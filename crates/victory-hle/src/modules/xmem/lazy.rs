//! Page mappings recorded without the system (`VICTORY_LAZY_REMAP=1`).
//!
//! While the world streams in, the GPU queue's page-mapping copies
//! ([`super::map_alias`]) come thousands of times a frame, most for one
//! page. The eager path gives each one several system calls under the
//! section lock (a view taken away, a placeholder split, a view mapped and
//! committed), each flushing every processor's TLB, and most of the pages
//! it maps are replaced again before the processor ever touches them.
//!
//! Here a page-mapping copy into one of the title's placeholder
//! reservations makes no system call: it notes which section page each
//! 64 KiB title page shows now. A view is mapped only when the processor
//! touches the page (an access fault, served by [`on_fault`]), for that
//! page alone, or when a system call is about to write it ([`materialize`]).
//! Our own reads and writes reach a recorded page through its section's one
//! stable view ([`super::Section::persistent_ptr`], `victory_gpu::recorded`).
//!
//! Locks: the record has its own ([`RECORD`]), taken after the page table's
//! ([`super::physical`]) and never before it; the fault handler and the
//! lookups for our own reads take the record's alone, so they never wait on
//! the page table (a GPU reader may hold the remapping lock while a
//! remapper holding the page table waits for it). Nothing reads or writes
//! title memory, and nothing calls into the GPU side, while the record's
//! lock is held (a title's query buffer is filled in after it is let go).
//! Should a thread fault on a recorded page all the same while it holds
//! the lock, the handler does not wait on itself: it knows ([`holding`])
//! and leaves the fault to the other handlers, said once.
//!
//! Off by default: unset, or anything but `1`, keeps every request on the
//! eager path, and none of this is reached.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Once, OnceLock};
use std::time::Instant;

use victory_gpu::recorded::{C, count};
use windows_sys::Win32::Foundation::{ERROR_INVALID_ADDRESS, SetLastError};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_MAPPED, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER,
    MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY,
    UnmapViewOfFile2,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

use super::{Meter, PAGE, Physical, Section, Slice};
use crate::{HleModule, hle_export};

/// Parses `VICTORY_LAZY_REMAP`: on for `1` alone.
pub(crate) fn parse(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Whether page mappings are recorded (`VICTORY_LAZY_REMAP=1`). The first
/// asking installs the fault handler and hands the GPU side its lookup; a
/// handler the system will not take leaves the switch off, said once.
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if !parse(std::env::var_os("VICTORY_LAZY_REMAP").as_deref()) {
            return false;
        }
        if !install() {
            tracing::warn!(
                "VICTORY_LAZY_REMAP=1: the access-fault handler could not be installed; page mappings stay eager"
            );
            return false;
        }
        victory_gpu::use_recorded_pages(host_pages);
        tracing::info!(
            "page-mapping copies into the title's placeholder reservations are recorded, and a page is mapped only \
             when the processor touches it (VICTORY_LAZY_REMAP=1)"
        );
        true
    })
}

/// A recorded title page.
struct Entry {
    section: Arc<Section>,
    index: usize,
    /// Whether a view of the page stands at its address (the processor
    /// touched it, or a system call was to write it).
    viewed: bool,
}

impl Entry {
    fn key(&self) -> (usize, usize) {
        (Arc::as_ptr(&self.section) as usize, self.index)
    }
}

#[derive(Default)]
struct Record {
    /// By title page address.
    pages: BTreeMap<usize, Entry>,
    /// By section page ([`Slice::key`]), the title pages recorded as
    /// showing it: how many recorded aliases point at it, and where.
    shown: HashMap<(usize, usize), Vec<usize>>,
    /// Pages the eager path would have left as one-page placeholders (a
    /// page it mapped and unmapped again) but which were never split here,
    /// since nothing touched them: split before any native operation on
    /// their range, which must find the boundaries the eager path makes.
    owed: BTreeSet<usize>,
    /// Pages split into placeholders of their own by our hand.
    alone: BTreeSet<usize>,
    /// The title's placeholder reservations, start to end: the only
    /// addresses recorded. A release or reuse of part of one ends that
    /// part.
    extents: BTreeMap<usize, usize>,
}

static RECORD: LazyLock<Mutex<Record>> = LazyLock::new(Mutex::default);

thread_local! {
    /// Whether this thread holds the record's lock ([`Held`]). A fault it
    /// takes on a recorded page then cannot be served, as the handler would
    /// wait on the lock this thread holds; [`on_fault`] leaves it to the
    /// other handlers.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

/// The record's lock, held: [`holding`] says so on this thread until the
/// guard goes.
struct Held {
    guard: ManuallyDrop<MutexGuard<'static, Record>>,
    /// What [`HOLDING`] said before, put back once the lock is let go.
    was: bool,
}

impl Deref for Held {
    type Target = Record;
    fn deref(&self) -> &Record {
        &self.guard
    }
}

impl DerefMut for Held {
    fn deref_mut(&mut self) -> &mut Record {
        &mut self.guard
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: dropped here alone, and not used after.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        HOLDING.set(self.was);
    }
}

fn record() -> Held {
    let guard = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    let was = HOLDING.replace(true);
    Held { guard: ManuallyDrop::new(guard), was }
}

/// Whether this thread holds the record's lock.
fn holding() -> bool {
    HOLDING.get()
}

/// Pages recorded now, and the lowest and highest address of any extent:
/// a fault or a lookup outside them is answered without the lock.
static RECORDED: AtomicUsize = AtomicUsize::new(0);
static LOW: AtomicUsize = AtomicUsize::new(usize::MAX);
static HIGH: AtomicUsize = AtomicUsize::new(0);

/// The system's mapping calls this path made, since the start (for the
/// tests; the frame log has its own).
static KERNEL_OPS: AtomicU64 = AtomicU64::new(0);

fn kernel_op() {
    KERNEL_OPS.fetch_add(1, Relaxed);
    count(C::KernelOps, 1);
}

/// Whether `[at, at + len)` may meet an extent.
fn near(at: usize, len: usize) -> bool {
    at < HIGH.load(Relaxed) && at.saturating_add(len) > LOW.load(Relaxed)
}

/// Whether a recorded page may lie under `[at, at + len)`.
fn maybe_recorded(at: usize, len: usize) -> bool {
    RECORDED.load(Relaxed) != 0 && near(at, len)
}

/// The title reserved a placeholder `[at, at + len)` for physical pages.
pub(super) fn note_reservation(at: usize, len: usize) {
    if !enabled() || at == 0 || len == 0 {
        return;
    }
    let end = at.saturating_add(len);
    record().extents.insert(at, end);
    LOW.fetch_min(at, Relaxed);
    HIGH.fetch_max(end, Relaxed);
}

/// The extent holding all of `[at, end)`, if one does.
fn extent_of(r: &Record, at: usize, end: usize) -> Option<(usize, usize)> {
    r.extents
        .range(..=at)
        .next_back()
        .filter(|&(_, &e)| end <= e)
        .map(|(&s, &e)| (s, e))
}

/// The extents meeting `[at, end)`, whole.
fn extents_meeting(r: &Record, at: usize, end: usize) -> Vec<(usize, usize)> {
    r.extents
        .range(..end)
        .filter(|&(_, &e)| e > at)
        .map(|(&s, &e)| (s, e))
        .collect()
}

/// `[at, end)` is no longer eligible: what was released or reused.
fn end_eligibility(r: &mut Record, at: usize, end: usize) {
    for (s, e) in extents_meeting(r, at, end) {
        r.extents.remove(&s);
        if s < at {
            r.extents.insert(s, at);
        }
        if end < e {
            r.extents.insert(end, e);
        }
    }
    let gone: Vec<usize> = r.owed.range(at..end).copied().collect();
    for page in gone {
        r.owed.remove(&page);
    }
    let gone: Vec<usize> = r.alone.range(at..end).copied().collect();
    for page in gone {
        r.alone.remove(&page);
    }
}

/// Removes `page` from the record and from its section page's list.
fn take(r: &mut Record, page: usize) -> Option<Entry> {
    let entry = r.pages.remove(&page)?;
    RECORDED.fetch_sub(1, Relaxed);
    let key = entry.key();
    if let Some(at) = r.shown.get_mut(&key) {
        at.retain(|&a| a != page);
        if at.is_empty() {
            r.shown.remove(&key);
        }
    }
    Some(entry)
}

/// Records `page` as showing `index` of `section`, no view at it yet.
fn put(r: &mut Record, page: usize, section: Arc<Section>, index: usize) {
    let entry = Entry { section, index, viewed: false };
    r.shown.entry(entry.key()).or_default().push(page);
    r.pages.insert(page, entry);
    RECORDED.fetch_add(1, Relaxed);
    if !r.alone.contains(&page) {
        r.owed.insert(page);
    }
}

/// Splits `page` out of the placeholder holding it (failing harmlessly
/// when it is one already).
fn split(r: &mut Record, page: usize) {
    // SAFETY: acts only on a placeholder.
    unsafe {
        windows_sys::Win32::System::Memory::VirtualFree(
            page as *mut c_void,
            PAGE,
            MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
        )
    };
    kernel_op();
    r.owed.remove(&page);
    r.alone.insert(page);
}

/// Takes the view at `page` away, leaving its placeholder. False if the
/// system refused.
fn unmap_view(page: usize) -> bool {
    let meter = Meter::begin();
    // SAFETY: a one-page view of ours; it becomes a placeholder again.
    let done = unsafe {
        UnmapViewOfFile2(
            GetCurrentProcess(),
            MEMORY_MAPPED_VIEW_ADDRESS { Value: page as *mut c_void },
            MEM_PRESERVE_PLACEHOLDER,
        )
    } != 0;
    meter.end(false);
    kernel_op();
    if !done {
        tracing::warn!(
            address = format_args!("{page:#x}"),
            error = %std::io::Error::last_os_error(),
            "VICTORY_LAZY_REMAP: taking a touched page's view away failed; the request fails"
        );
    }
    done
}

/// Maps a view of `page`'s section page at `page`, that page alone,
/// committed read-write.
fn map_one(r: &mut Record, page: usize) -> bool {
    let started = std::time::Instant::now();
    let Some((handle, offset)) = r.pages.get(&page).map(|e| (e.section.handle, (e.index * PAGE) as u64)) else {
        return false;
    };
    let map = || {
        let meter = Meter::begin();
        // SAFETY: replaces a one-page placeholder with a view of our
        // section.
        let view = unsafe {
            MapViewOfFile3(
                handle,
                GetCurrentProcess(),
                page as *const c_void,
                offset,
                PAGE,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_READWRITE,
                std::ptr::null_mut(),
                0,
            )
        };
        meter.end(true);
        kernel_op();
        view.Value as usize == page
    };
    if !r.alone.contains(&page) {
        split(r, page);
    }
    // A page we took to be alone may have been part of a larger
    // placeholder again (an eager view over it, unmapped whole): split and
    // tried once more.
    let mapped = map() || {
        split(r, page);
        map()
    };
    if !mapped {
        return false;
    }
    if !super::on_wine() {
        kernel_op();
    }
    if !super::commit(page, PAGE) {
        // SAFETY: the view just mapped, back to a placeholder.
        unsafe {
            UnmapViewOfFile2(
                GetCurrentProcess(),
                MEMORY_MAPPED_VIEW_ADDRESS { Value: page as *mut c_void },
                MEM_PRESERVE_PLACEHOLDER,
            )
        };
        kernel_op();
        return false;
    }
    if let Some(entry) = r.pages.get_mut(&page) {
        entry.viewed = true;
    }
    count(C::TouchViews, 1);
    count(C::Us, started.elapsed().as_micros() as u64);
    true
}

/// What a touch of a recorded page came to.
enum Touch {
    /// A view was mapped for it.
    Mapped,
    /// A view stood there already, and allows the access.
    Allowed,
    /// Not ours, or not allowed: the next handler's.
    No,
}

/// The processor touched `at` (`write`: to store).
fn touch(at: usize, write: bool) -> Touch {
    let page = at & !(PAGE - 1);
    let mut r = record();
    let Some(entry) = r.pages.get(&page) else {
        return Touch::No;
    };
    if entry.viewed {
        // Another thread mapped it meanwhile; a store into a page a guard
        // made read-only is the guard's.
        return if allowed(at, write) { Touch::Allowed } else { Touch::No };
    }
    if map_one(&mut r, page) { Touch::Mapped } else { Touch::No }
}

/// Whether the memory at `at` allows the access now.
fn allowed(at: usize, write: bool) -> bool {
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe {
        windows_sys::Win32::System::Memory::VirtualQuery(at as *const c_void, &mut info, size_of_val(&info))
    };
    if n == 0 || info.State != MEM_COMMIT || info.Protect & PAGE_GUARD != 0 {
        return false;
    }
    let protect = info.Protect & 0xff;
    if write {
        matches!(protect, PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY)
    } else {
        protect != PAGE_NOACCESS
    }
}

/// What the handler does with a read or a write fault, decided before any
/// lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// No recorded page can be there: the next handler's.
    Pass,
    /// A recorded page may be there, but this thread holds the record's
    /// lock: taking it would wait on itself, so the next handler's.
    Holding,
    /// A recorded page may be there: looked up under the lock.
    Look,
}

/// [`Step`] for a fault that may (`near`) or cannot meet a recorded page,
/// on a thread that holds the record's lock or not.
fn step(near: bool, holding: bool) -> Step {
    match (near, holding) {
        (false, _) => Step::Pass,
        (true, true) => Step::Holding,
        (true, false) => Step::Look,
    }
}

/// Faults that may have met a recorded page, taken while their thread held
/// the record's lock (left to the other handlers).
static HOLDING_FAULTS: AtomicU64 = AtomicU64::new(0);

/// The handler's answer to a read or a write fault at `at`, after `step`:
/// the record's lock is taken for [`Step::Look`] alone.
fn respond(step: Step, at: usize, write: bool) -> i32 {
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH};
    match step {
        Step::Pass => EXCEPTION_CONTINUE_SEARCH,
        Step::Holding => {
            HOLDING_FAULTS.fetch_add(1, Relaxed);
            static SAID: Once = Once::new();
            SAID.call_once(|| {
                tracing::warn!(
                    address = format_args!("{at:#x}"),
                    "a fault on a recorded page while this thread holds the record table (VICTORY_LAZY_REMAP): left \
                     to the other handlers"
                );
            });
            EXCEPTION_CONTINUE_SEARCH
        }
        Step::Look => match touch(at, write) {
            Touch::Mapped => {
                count(C::Faults, 1);
                EXCEPTION_CONTINUE_EXECUTION
            }
            Touch::Allowed => EXCEPTION_CONTINUE_EXECUTION,
            Touch::No => EXCEPTION_CONTINUE_SEARCH,
        },
    }
}

/// The access-fault handler: a read or a write of a recorded page with no
/// view gets that page mapped, and the access resumes. Execute faults are
/// not ours. Each read or write fault is counted, served or passed on, with
/// the time it took (the frame-rate line).
unsafe extern "system" fn on_fault(
    info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH};
    // SAFETY: the system passes the exception being dispatched.
    let r = unsafe { &*(*info).ExceptionRecord };
    const READ: usize = 0;
    const WRITE: usize = 1;
    if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS || r.NumberParameters < 2 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let kind = r.ExceptionInformation[0];
    let at = r.ExceptionInformation[1];
    if !(kind == READ || kind == WRITE) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let started = Instant::now();
    let next = step(maybe_recorded(at, 1), holding());
    let answer = respond(next, at, kind == WRITE);
    let served = answer == EXCEPTION_CONTINUE_EXECUTION;
    count(C::HandlerCalls, 1);
    count(if served { C::HandlerServed } else { C::HandlerPassed }, 1);
    count(C::HandlerNs, started.elapsed().as_nanos() as u64);
    // In the tests, loudly: a path that touches title memory under the
    // record's lock.
    if cfg!(test) {
        debug_assert!(
            next != Step::Holding,
            "a fault on a recorded page at {at:#x} while this thread holds the record table"
        );
    }
    answer
}

fn install() -> bool {
    use windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;
    // SAFETY: a handler that touches the record and maps our own sections.
    !unsafe { AddVectoredExceptionHandler(1, Some(on_fault)) }.is_null()
}

/// Real views for the recorded pages under `[at, at + len)`, before a
/// system call writes there or changes their protection (it does not
/// fault into our handler). False if one could not be mapped.
pub(crate) fn materialize(at: usize, len: usize) -> bool {
    if !enabled() || len == 0 || !maybe_recorded(at, len) {
        return true;
    }
    let (first, end) = (at & !(PAGE - 1), at.saturating_add(len));
    let mut r = record();
    let due: Vec<usize> = r.pages.range(first..end).filter(|(_, e)| !e.viewed).map(|(&p, _)| p).collect();
    due.into_iter().all(|page| map_one(&mut r, page))
}

/// The section page `at` shows, eager view or recorded page.
fn source(physical: &Physical, r: &Record, at: usize) -> Option<(Arc<Section>, usize)> {
    if let Some((base, (size, slice))) = physical.views.range(..=at).next_back()
        && at < base + size
    {
        return Some((slice.section.clone(), slice.index + (at - base) / PAGE));
    }
    r.pages.get(&(at & !(PAGE - 1))).map(|e| (e.section.clone(), e.index))
}

/// The section pages behind `n` pages from `from` on, up to the first that
/// shows none.
fn sources(physical: &Physical, from: usize, n: usize) -> Vec<(Arc<Section>, usize)> {
    let r = record();
    (0..n).map_while(|k| source(physical, &r, from + k * PAGE)).collect()
}

/// Whether each section has its stable view (needed for our own reads of
/// its recorded pages); one that refuses says so once and stays eager.
fn stable(sources: &[(Arc<Section>, usize)]) -> bool {
    let mut seen: Vec<usize> = Vec::new();
    for (section, _) in sources {
        let id = Arc::as_ptr(section) as usize;
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        if section.persistent_ptr().is_none() {
            if section.lazy_refused.set(()).is_ok() {
                tracing::warn!(
                    size = section.size,
                    "VICTORY_LAZY_REMAP: a section's stable view was refused; mappings of its pages stay eager"
                );
            }
            return false;
        }
    }
    true
}

/// What changed at a page, for the GPU side once the record's lock is let
/// go: a page whose contents changed, and what it shows now if anything.
struct Changed {
    page: usize,
    now: Option<(usize, u64)>,
}

fn tell(changed: &[Changed]) {
    for c in changed {
        // New contents, read-write: the GPU side's copies no longer guard
        // it (as the eager path's `map` and `unmap` say, page by page).
        victory_gpu::title_memory_remapped(c.page, PAGE);
        if let Some((section, offset)) = c.now {
            victory_gpu::title_memory_view(c.page, PAGE, section, offset);
        }
    }
}

/// Drops the recorded pages of `[at, end)`: a view at one is taken away
/// first, and a refusal ends the drop there (that page keeps its record)
/// and answers false. With `split`, the owed pages of every extent the
/// range meets are split as well, as a native or eager operation there
/// needs them. Caller holds the record's lock; the GPU side is told
/// through `changed`, the entries dropped through `dropped`, after.
fn drop_range(
    r: &mut Record,
    at: usize,
    end: usize,
    split_owed: bool,
    changed: &mut Vec<Changed>,
    dropped: &mut Vec<Entry>,
) -> bool {
    let pages: Vec<usize> = r.pages.range(at..end).map(|(&p, _)| p).collect();
    for page in pages {
        if r.pages.get(&page).is_some_and(|e| e.viewed) && !unmap_view(page) {
            return false;
        }
        if let Some(entry) = take(r, page) {
            count(C::EagerOps, 1);
            if !r.alone.contains(&page) {
                r.owed.insert(page);
            }
            changed.push(Changed { page, now: None });
            dropped.push(entry);
        }
    }
    if split_owed {
        for (s, e) in extents_meeting(r, at, end) {
            let owed: Vec<usize> = r.owed.range(s..e).copied().collect();
            for page in owed {
                split(r, page);
            }
        }
        // What an eager view over them leaves when it goes is one
        // placeholder, not pages alone.
        let gone: Vec<usize> = r.alone.range(at..end).copied().collect();
        for page in gone {
            r.alone.remove(&page);
        }
    }
    true
}

/// Before the eager path, or a native operation, works on `[at, at +
/// len)`: its recorded pages go (views taken away), and the pages the eager
/// path would have split are split. False if a view could not be taken
/// away (the caller fails).
pub(super) fn clear(at: usize, len: usize) -> bool {
    if !enabled() || len == 0 || !near(at, len) {
        return true;
    }
    // Whole pages, as the eager path's unmap takes whole views.
    let (mut changed, mut dropped) = (Vec::new(), Vec::new());
    let done = drop_range(&mut record(), at & !(PAGE - 1), at.saturating_add(len), true, &mut changed, &mut dropped);
    tell(&changed);
    drop(dropped);
    done
}

/// [`super::map_alias`] on this path: `None` when the request is not for
/// it (the switch off, a destination outside the title's placeholder
/// reservations, unaligned or overflowing ranges, a source section
/// without its stable view), and the eager path takes it.
///
/// Semantics are the eager path's. A null source unmaps; a source page may
/// be a recorded page or an eager view; a source overlapping its
/// destination clears the destination first, as the eager path's unmap
/// does (so a page aliased onto itself fails); a source page that shows
/// nothing fails the request, leaving the pages before it mapped and the
/// rest unmapped. When the two do not overlap each page is replaced alone
/// under the record's lock, so a reader sees the old page or the new one
/// and never a gap.
pub(super) fn alias(physical: &mut Physical, to: usize, from: usize, len: usize) -> Option<bool> {
    if !enabled() || len == 0 {
        return None;
    }
    // A null destination with a source fails. (The eager path's `map`
    // takes the null its mapping call returns there for the address asked
    // for, and answers true; left as it is without the switch.)
    if to == 0 && from != 0 {
        return Some(false);
    }
    if !to.is_multiple_of(PAGE) || !len.is_multiple_of(PAGE) || !from.is_multiple_of(PAGE) {
        return None;
    }
    let end = to.checked_add(len)?;
    if from != 0 && from.checked_add(len).is_none() {
        return None;
    }
    if !maybe_extent(to, end) {
        return None;
    }
    let started = std::time::Instant::now();
    let n = len / PAGE;
    let overlap = from != 0 && from < end && to < from + len;
    let (mut changed, mut dropped) = (Vec::new(), Vec::new());
    let mut ok = true;
    if from == 0 || overlap {
        // As the eager path: the destination is cleared first.
        super::unmap(physical, to, len);
        ok = drop_range(&mut record(), to, end, false, &mut changed, &mut dropped);
    }
    if ok && overlap {
        // As the eager path, each source page is looked up once the pages
        // before it are mapped: a source page in the destination shows
        // what was just put there, or nothing.
        for k in 0..n {
            let page = to + k * PAGE;
            let Some((section, index)) = source(physical, &record(), from + k * PAGE) else {
                ok = false;
                break;
            };
            count(C::EagerOps, 1);
            if !stable(&[(section.clone(), index)]) {
                // This page eagerly, as the eager path would.
                if !super::map(physical, page, PAGE, Slice { section, index }) {
                    ok = false;
                    break;
                }
                continue;
            }
            let now = Some((section.handle as usize, (index * PAGE) as u64));
            put(&mut record(), page, section, index);
            count(C::Pages, 1);
            changed.push(Changed { page, now });
        }
    }
    if ok && from != 0 && !overlap {
        let found = sources(physical, from, n);
        if !stable(&found) {
            // Eager after all, from a destination without records.
            return if clear(to, len) { None } else { Some(false) };
        }
        super::unmap(physical, to, len);
        let mut r = record();
        for k in 0..n {
            let page = to + k * PAGE;
            if r.pages.get(&page).is_some_and(|e| e.viewed) && !unmap_view(page) {
                // The bookkeeping stays as it was for this page and those
                // after it.
                ok = false;
                break;
            }
            let old = take(&mut r, page);
            let new = found.get(k).cloned();
            count(C::EagerOps, u64::from(old.is_some()) + u64::from(new.is_some()));
            match new {
                Some((section, index)) => {
                    let now = Some((section.handle as usize, (index * PAGE) as u64));
                    put(&mut r, page, section, index);
                    count(C::Pages, 1);
                    changed.push(Changed { page, now });
                }
                None => {
                    if old.is_some() {
                        if !r.alone.contains(&page) {
                            r.owed.insert(page);
                        }
                        changed.push(Changed { page, now: None });
                    }
                    ok = false;
                }
            }
            dropped.extend(old);
        }
    }
    tell(&changed);
    drop(dropped);
    count(C::Us, started.elapsed().as_micros() as u64);
    Some(ok)
}

/// Whether all of `[at, end)` lies in one extent.
fn maybe_extent(at: usize, end: usize) -> bool {
    near(at, end - at) && extent_of(&record(), at, end).is_some()
}

/// The title addresses recorded as showing section page `key`.
pub(super) fn shown(key: (usize, usize)) -> Vec<usize> {
    if RECORDED.load(Relaxed) == 0 {
        return Vec::new();
    }
    record().shown.get(&key).cloned().unwrap_or_default()
}

/// For the GPU side (`victory_gpu::recorded`): the recorded pages under
/// `[at, at + len)` and their bytes in their section's stable view,
/// committed.
fn host_pages(at: usize, len: usize) -> Vec<victory_gpu::HostPage> {
    if !maybe_recorded(at, len) {
        return Vec::new();
    }
    let (first, end) = (at & !(PAGE - 1), at.saturating_add(len));
    let found: Vec<(usize, Arc<Section>, usize)> = record()
        .pages
        .range(first..end)
        .map(|(&page, e)| (page, e.section.clone(), e.index))
        .collect();
    found
        .into_iter()
        .filter_map(|(page, section, index)| {
            let base = section.persistent_ptr()?;
            if !section.ensure_committed(base, index, 1) {
                return None;
            }
            // SAFETY: `ensure_committed` proved the page lies in the view.
            let ptr = unsafe { base.add(index * PAGE) };
            Some(victory_gpu::HostPage { at: page, ptr, keep: section })
        })
        .collect()
}

/// What a query of `address` would answer on the eager path, from the
/// system's answer `info`: a recorded page without a view is one committed
/// page (as a load would find it), and regions stop at recorded pages and
/// at the pages the eager path would have split.
pub(super) fn adjust_query(address: usize, info: &mut MEMORY_BASIC_INFORMATION) {
    if !enabled() || !near(address, 1) || info.State == MEM_COMMIT {
        return;
    }
    let page = address & !(PAGE - 1);
    let base = info.BaseAddress as usize;
    let r = record();
    if r.pages.get(&page).is_some_and(|e| !e.viewed) {
        info.AllocationBase = page as *mut c_void;
        info.AllocationProtect = PAGE_READWRITE;
        info.RegionSize = page + PAGE - base;
        info.State = MEM_COMMIT;
        info.Protect = PAGE_READWRITE;
        info.Type = MEM_MAPPED;
        return;
    }
    if r.owed.contains(&page) {
        info.RegionSize = info.RegionSize.min(page + PAGE - base);
        return;
    }
    let region_end = base.saturating_add(info.RegionSize);
    let next = [
        r.pages.range(page + PAGE..region_end).next().map(|(&p, _)| p),
        r.owed.range(page + PAGE..region_end).next().copied(),
    ];
    if let Some(next) = next.into_iter().flatten().min() {
        info.RegionSize = next - base;
    }
}

/// Before the title releases `[at, end)` (or, with `size` 0, the region at
/// `at`), or replaces a placeholder there: false if a view of a touched
/// page stands there (the release fails, as it does on a view); else the
/// recorded pages there go, the owed pages are split, and the range is no
/// longer eligible.
fn release(at: usize, size: usize) -> bool {
    let end = if size == 0 {
        // The region the eager path would have there.
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        if unsafe {
            windows_sys::Win32::System::Memory::VirtualQuery(at as *const c_void, &mut info, size_of_val(&info))
        } == 0
        {
            return true;
        }
        adjust_query(at, &mut info);
        (info.BaseAddress as usize).saturating_add(info.RegionSize)
    } else {
        at.saturating_add(size).next_multiple_of(4096)
    };
    let at = at & !(4096 - 1);
    let (mut changed, mut dropped) = (Vec::new(), Vec::new());
    let done = {
        let mut r = record();
        if r.pages.range(at & !(PAGE - 1)..end).any(|(_, e)| e.viewed) {
            false
        } else {
            let done = drop_range(&mut r, at & !(PAGE - 1), end, true, &mut changed, &mut dropped);
            if done {
                end_eligibility(&mut r, at, end);
            }
            done
        }
    };
    tell(&changed);
    drop(dropped);
    done
}

/// [`release`] for `XMemVirtualAlloc`'s replacement of part of a
/// placeholder reservation with other memory.
pub(super) fn reuse(at: usize, size: usize) -> bool {
    if !enabled() || size == 0 || !near(at, size) {
        return true;
    }
    release(at, size)
}

/// The title's `VirtualFree`, with the switch on: a release in a
/// placeholder reservation sees the eager path's boundaries.
pub extern "system" fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> BOOL {
    let at = address as usize;
    if enabled() && kind & MEM_RELEASE != 0 && near(at, size.max(1)) && !release(at, size) {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_ADDRESS) };
        return 0;
    }
    // SAFETY: the title's own arguments.
    unsafe { windows_sys::Win32::System::Memory::VirtualFree(address, size, kind) }
}

/// The title's `VirtualQuery`, with the switch on: recorded pages read as
/// the eager path's views.
pub extern "system" fn VirtualQuery(
    address: *const c_void,
    buffer: *mut MEMORY_BASIC_INFORMATION,
    length: usize,
) -> usize {
    // SAFETY: the title's own arguments.
    let n = unsafe { windows_sys::Win32::System::Memory::VirtualQuery(address, buffer, length) };
    if n >= size_of::<MEMORY_BASIC_INFORMATION>() {
        // Adjusted in a copy: the title's buffer is not written under the
        // record's lock (a fault there could not be served).
        // SAFETY: the system filled a whole structure there.
        let mut info = unsafe { buffer.read_unaligned() };
        adjust_query(address as usize, &mut info);
        // SAFETY: as above.
        unsafe { buffer.write_unaligned(info) };
    }
    n
}

/// The title's `VirtualProtect`, with the switch on: recorded pages get
/// their views first (a protection needs a page to protect).
pub extern "system" fn VirtualProtect(address: *const c_void, size: usize, protect: u32, old: *mut u32) -> BOOL {
    if !materialize(address as usize, size.max(1)) {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_ADDRESS) };
        return 0;
    }
    // SAFETY: the title's own arguments.
    unsafe { windows_sys::Win32::System::Memory::VirtualProtect(address, size, protect, old) }
}

/// The title's memory calls that must see recorded pages as the eager
/// path's views; served only with the switch on.
pub(crate) fn module() -> HleModule {
    HleModule {
        name: "xmem-lazy",
        dlls: &["kernel32.dll", "kernelbase.dll", "api-ms-win-core-memory-*"],
        exports: vec![hle_export!(VirtualFree), hle_export!(VirtualQuery), hle_export!(VirtualProtect)],
    }
}

#[cfg(test)]
mod tests;
