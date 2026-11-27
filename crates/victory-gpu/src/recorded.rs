//! Title pages the runtime has recorded but not mapped
//! (`VICTORY_LAZY_REMAP=1`, `victory-hle` `xmem`): a page-mapping copy of
//! the queue notes which section page a 64 KiB title page shows, and the
//! system maps it only when the title's processor touches it. Until then
//! the address is a reservation, which the kernel's copies
//! (`ReadProcessMemory`, `NtWriteVirtualMemory`) and `VirtualQuery` report
//! as not there. Our own reads and writes of such a page go through the
//! section's one stable view instead ([`HostPage`]), and the checks of
//! what is mapped count it as there.
//!
//! The runtime hands us its lookup with [`use_recorded_pages`] when the
//! switch is on. Without it every function here answers "nothing
//! recorded" at once and the callers take their usual path unchanged.

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};

/// The runtime's page: 64 KiB.
pub const PAGE: usize = 64 << 10;

/// A recorded title page as our own code reaches it.
pub struct HostPage {
    /// The title address of the page (a multiple of [`PAGE`]).
    pub at: usize,
    /// The page's bytes in its section's stable view, committed: [`PAGE`]
    /// bytes, readable and writable for as long as `keep` lives.
    pub ptr: *mut u8,
    /// Keeps the section, and so the view, alive while the bytes are used,
    /// even if the title frees or remaps the page meanwhile.
    pub keep: Arc<dyn Any + Send + Sync>,
}

/// The recorded pages under `[at, at + len)`, in address order.
pub type Lookup = fn(usize, usize) -> Vec<HostPage>;

static LOOKUP: OnceLock<Lookup> = OnceLock::new();

/// The runtime records page mappings (`VICTORY_LAZY_REMAP=1`): `lookup`
/// says which title pages are recorded and where their bytes are.
pub fn use_recorded_pages(lookup: Lookup) {
    let _ = LOOKUP.set(lookup);
}

/// Whether the runtime records page mappings.
pub(crate) fn on() -> bool {
    LOOKUP.get().is_some()
}

/// The recorded pages under `[at, at + len)`; empty when the switch is off.
fn pages(at: usize, len: usize) -> Vec<HostPage> {
    match LOOKUP.get() {
        Some(lookup) if len > 0 => lookup(at, len),
        _ => Vec::new(),
    }
}

/// The piece of `[at, at + len)` from `from` to the next page boundary or
/// the end, and the recorded page holding it, if any.
fn piece(pages: &[HostPage], at: usize, len: usize, from: usize) -> (usize, Option<&HostPage>) {
    let page = from & !(PAGE - 1);
    let n = (page + PAGE).min(at + len) - from;
    (n, pages.binary_search_by_key(&page, |p| p.at).ok().map(|i| &pages[i]))
}

/// A copy of `len` bytes from `src` to `dst` when a recorded page lies
/// under either; `None` when none does (the caller copies as usual).
/// Recorded pieces go through the stable view, the rest through `plain`
/// (the kernel's copy, which fails on a page that is not there). All of the
/// source is read before any of the destination is written: the two may
/// show one section page at two addresses.
pub(crate) fn copy(
    src: *const u8,
    dst: *mut u8,
    len: usize,
    plain: impl Fn(*const u8, *mut u8, usize) -> bool,
) -> Option<bool> {
    if !on() || len == 0 {
        return None;
    }
    let (from, to) = (pages(src as usize, len), pages(dst as usize, len));
    if from.is_empty() && to.is_empty() {
        return None;
    }
    let mut bytes = vec![0u8; len];
    let mut done = 0;
    while done < len {
        let (n, page) = piece(&from, src as usize, len, src as usize + done);
        match page {
            // SAFETY: the page's view holds `PAGE` bytes, the piece lies in
            // them, and `keep` holds the view.
            Some(p) => unsafe {
                std::ptr::copy_nonoverlapping(p.ptr.add(src as usize + done - p.at), bytes.as_mut_ptr().add(done), n)
            },
            None => {
                if !plain(src.wrapping_add(done), bytes[done..].as_mut_ptr(), n) {
                    return Some(false);
                }
            }
        }
        done += n;
    }
    let mut whole = true;
    done = 0;
    while done < len {
        let (n, page) = piece(&to, dst as usize, len, dst as usize + done);
        match page {
            // SAFETY: as above, for the destination.
            Some(p) => unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(done), p.ptr.add(dst as usize + done - p.at), n)
            },
            None => whole &= plain(bytes[done..].as_ptr(), dst.wrapping_add(done), n),
        }
        done += n;
    }
    count(C::HostBytes, len as u64);
    Some(whole)
}

/// A write of `len` bytes from host memory at `src` into title memory at
/// `dst` when a recorded page lies under the destination: those pieces go
/// through the stable view, the rest to `plain` (at, bytes). False when
/// none does, and nothing was written.
pub(crate) fn write(dst: *mut u8, src: *const u8, len: usize, mut plain: impl FnMut(usize, usize)) -> bool {
    if !on() || len == 0 {
        return false;
    }
    let to = pages(dst as usize, len);
    if to.is_empty() {
        return false;
    }
    let mut done = 0;
    while done < len {
        let (n, page) = piece(&to, dst as usize, len, dst as usize + done);
        match page {
            // SAFETY: the page's view holds `PAGE` bytes, the piece lies in
            // them, and `keep` holds the view; `src` holds `len` bytes.
            Some(p) => unsafe { std::ptr::copy_nonoverlapping(src.add(done), p.ptr.add(dst as usize + done - p.at), n) },
            None => plain(dst as usize + done, n),
        }
        done += n;
    }
    count(C::HostBytes, len as u64);
    true
}

/// How many bytes of `[at, at + len)` lie on recorded pages (which the
/// system reports reserved, and our copies read through their view).
pub(crate) fn present_bytes(at: usize, len: usize) -> u64 {
    if !on() || len == 0 {
        return 0;
    }
    let end = at.saturating_add(len);
    pages(at, len)
        .iter()
        .map(|p| (p.at + PAGE).min(end).saturating_sub(p.at.max(at)) as u64)
        .sum()
}

/// What the runtime counts of its recording, for the frame-rate line (per
/// second) and the frame log (`crate::framelog`, beside the remapping
/// meter).
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum C {
    /// Pages a page-mapping copy asked for and that were recorded.
    Pages,
    /// The system's mapping calls made all the same: views of touched
    /// pages taken away, one-page views made, placeholders split.
    KernelOps,
    /// What the eager path would have made for the same pages: a view
    /// taken away for each page that showed something, one made for each
    /// page that shows something new.
    EagerOps,
    /// One-page views made because the processor touched a recorded page
    /// (or a system call was about to write it).
    TouchViews,
    /// Access faults served by mapping a recorded page.
    Faults,
    /// Microseconds the runtime spent recording (the page table's lock
    /// held, the wait for it left out).
    Us,
    /// Bytes our own copies moved through stable views.
    HostBytes,
    /// Calls of the access-fault handler that reached its range check (a
    /// read or a write fault), those of them served (a recorded page
    /// mapped, or found mapped), those passed on to the other handlers, and
    /// the nanoseconds spent in it. For the frame-rate line alone.
    HandlerCalls,
    HandlerServed,
    HandlerPassed,
    HandlerNs,
    Count,
}

const N: usize = C::Count as usize;
static SINCE_LINE: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

/// Counts for the frame-rate line and the frame log.
pub fn count(c: C, n: u64) {
    use crate::framelog::{T, add};
    SINCE_LINE[c as usize].fetch_add(n, Relaxed);
    let t = match c {
        C::Pages => T::LazyPages,
        C::KernelOps => T::LazyKernelOps,
        C::EagerOps => T::LazyEagerOps,
        C::TouchViews => T::LazyTouchViews,
        C::Faults => T::LazyFaults,
        C::Us => T::LazyUs,
        C::HostBytes => T::LazyHostBytes,
        C::HandlerCalls | C::HandlerServed | C::HandlerPassed | C::HandlerNs | C::Count => return,
    };
    add(t, n);
}

/// With the frame-rate line, when the switch is on: what was recorded per
/// second since the line before.
pub(crate) fn line() {
    if !on() {
        return;
    }
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let now = std::time::Instant::now();
    let seconds = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let seconds = last.map_or(1.0, |t| now.duration_since(t).as_secs_f64().max(1e-3));
        *last = Some(now);
        seconds
    };
    let [pages, ops, eager, views, faults, us, bytes, calls, served, passed, handler_ns] =
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(|i| SINCE_LINE[i].swap(0, Relaxed) as f64 / seconds);
    tracing::info!(
        "page mappings recorded without the system (VICTORY_LAZY_REMAP=1), per second: {pages:.0} pages asked for, \
         {ops:.0} system mapping calls made ({:.0} avoided of the {eager:.0} the eager path would make), {views:.0} \
         one-page views made on a first touch, {faults:.0} faults served, {:.1} ms recording, {:.1} MB through stable \
         views, handler calls {calls:.0} (served {served:.0}, passed on {passed:.0}), handler time {:.2} ms",
        (eager - ops).max(0.0),
        us / 1e3,
        bytes / 1e6,
        handler_ns / 1e6,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_stop_at_page_boundaries_and_find_their_page() {
        let keep: Arc<dyn Any + Send + Sync> = Arc::new(());
        let pages = [HostPage { at: 2 * PAGE, ptr: std::ptr::null_mut(), keep }];
        // From inside the page before to the next boundary: no page.
        assert!(matches!(piece(&pages, PAGE + 10, 3 * PAGE, PAGE + 10), (n, None) if n == PAGE - 10));
        // The recorded page, whole.
        assert!(matches!(piece(&pages, PAGE + 10, 3 * PAGE, 2 * PAGE), (n, Some(p)) if n == PAGE && p.at == 2 * PAGE));
        // Cut at the end of the range.
        assert!(matches!(piece(&pages, 2 * PAGE, 5, 2 * PAGE), (5, Some(_))));
    }

    /// The test's recorded pages: title address and the bytes behind it.
    static FAKE: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

    fn fake(at: usize, len: usize) -> Vec<HostPage> {
        let keep: Arc<dyn Any + Send + Sync> = Arc::new(());
        let (first, end) = (at & !(PAGE - 1), at + len);
        let mut found: Vec<HostPage> = FAKE
            .lock()
            .unwrap()
            .iter()
            .filter(|&&(page, _)| page >= first && page < end)
            .map(|&(page, ptr)| HostPage { at: page, ptr: ptr as *mut u8, keep: keep.clone() })
            .collect();
        found.sort_by_key(|p| p.at);
        found
    }

    /// Native, recorded and missing pages in one range, and two addresses
    /// showing one section page: the checks of what is mapped count the
    /// recorded ones, copies read them through their view, write-back
    /// writes them there, and a copy between the two addresses reads all
    /// of its source first.
    #[test]
    fn copies_and_checks_over_native_recorded_and_missing_pages() {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc};
        // Kept for the life of the process: the lookup is the process's.
        // SAFETY: a fresh reservation of our own.
        let region = unsafe { VirtualAlloc(std::ptr::null(), 4 * PAGE, MEM_RESERVE, PAGE_NOACCESS) } as usize;
        assert_ne!(region, 0);
        // Page 0 native, 1 and 3 recorded on one section page, 2 missing.
        // SAFETY: the reservation's first page.
        assert!(!unsafe { VirtualAlloc(region as *const _, PAGE, MEM_COMMIT, PAGE_READWRITE) }.is_null());
        // SAFETY: committed read-write just now.
        unsafe { (region as *mut u8).write_bytes(0x10, PAGE) };
        let pattern: Vec<u8> = (0..PAGE).map(|i| (i % 251) as u8).collect();
        let backing: &'static mut [u8] = Box::leak(pattern.clone().into_boxed_slice());
        let shared = backing.as_mut_ptr() as usize;
        *FAKE.lock().unwrap() = vec![(region + PAGE, shared), (region + 3 * PAGE, shared)];
        use_recorded_pages(fake);
        assert!(on());

        assert!(crate::texture::committed(region as *const u8, 2 * PAGE));
        assert!(crate::texture::committed((region + 3 * PAGE) as *const u8, PAGE));
        assert!(!crate::texture::committed((region + 2 * PAGE) as *const u8, 1));
        assert_eq!(crate::texture::mapped_bytes(region as *const u8, 4 * PAGE), 3 * PAGE as u64);
        assert_eq!(present_bytes(region + PAGE - 8, 16), 8);

        let mut out = vec![0xffu8; 4 * PAGE];
        crate::host::copy_mapped(out.as_mut_ptr(), region as *const u8, 4 * PAGE, false);
        assert!(out[..PAGE].iter().all(|&b| b == 0x10), "native");
        assert_eq!(out[PAGE..2 * PAGE], pattern[..], "recorded");
        assert!(out[2 * PAGE..3 * PAGE].iter().all(|&b| b == 0), "missing reads as zeros");
        assert_eq!(out[3 * PAGE..], pattern[..], "recorded, the same section page");

        let src: Vec<u8> = (0..4 * PAGE).map(|i| (i / 7 % 256) as u8).collect();
        crate::host::write_mapped(region as *mut u8, src.as_ptr(), 4 * PAGE);
        // SAFETY: committed read-write.
        let native = unsafe { std::slice::from_raw_parts(region as *const u8, PAGE) };
        assert_eq!(native, &src[..PAGE]);
        // SAFETY: the leaked backing, alive for the process.
        let shown = || unsafe { std::slice::from_raw_parts(shared as *const u8, PAGE) }.to_vec();
        assert_eq!(shown(), src[3 * PAGE..], "the later address's bytes are the page's");

        // From one address of the page to the other, shifted: the whole
        // source is read before the destination is written.
        // SAFETY: the leaked backing, written by this test alone.
        unsafe { std::ptr::copy_nonoverlapping(pattern.as_ptr(), shared as *mut u8, PAGE) };
        crate::host::copy_mapped((region + 3 * PAGE) as *mut u8, (region + PAGE + 16) as *const u8, PAGE - 16, true);
        assert_eq!(shown()[..PAGE - 16], pattern[16..]);
        assert_eq!(shown()[PAGE - 16..], pattern[PAGE - 16..]);
        FAKE.lock().unwrap().clear();
    }
}
