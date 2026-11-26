//! Opt-in rewriting of AMD-only instruction sites in title code.
//!
//! On an Intel host every `monitorx`, `mwaitx` and `clzero` the title runs
//! raises #UD, and [`crate::crash`] does its job instead: an exception, a
//! kernel round trip of microseconds, each time. Spin-waits run these tens
//! of thousands of times a second. With a switch on, a site is rewritten the
//! first time it is emulated, so later passes run without faulting:
//!
//! - `VICTORY_PATCH_WAITS=1`: `monitorx` (`0f 01 fa`) becomes the three-byte
//!   NOP `0f 1f 00` (its emulation does nothing), and `mwaitx` (`0f 01 fb`)
//!   becomes `pause; nop` (`f3 90 90`). Its emulation yields the core
//!   (`SwitchToThread`); `pause` only eases the spin. The loops around
//!   `mwaitx` re-check their condition, so they stay correct, but a waiter
//!   no longer hands its core to another ready thread: fine with free cores,
//!   worse if the title runs more busy threads than the host has.
//! - `VICTORY_PATCH_CLZERO=1`: `clzero` (zero the 64-byte line at `rax`)
//!   does not fit in three bytes, and a jump needs five, so only loops the
//!   emulation already recognises are rewritten: with `=1` the first form
//!   alone (`clzero; add rax, 0x40; cmp; jb`: the `add` is moved and the
//!   loop's condition stays where it was); with `=2` also the second (`mov
//!   rax, a; add a, 0x40; clzero; cmp a, b; jb`), whose moved instruction
//!   is the loop's CONDITION: if any code jumps straight to that `cmp` (a
//!   `while` loop entered at its test), after the rewrite it lands in the
//!   middle of the `jmp rel32` and runs garbage. Nothing here checks jump
//!   targets, so `=2` is a risk taken knowingly. In both, where the
//!   instruction after `clzero` is a known register-only one (`add rax,
//!   0x40` or `cmp r64, r64`): those `clzero` + next instruction bytes
//!   become `jmp` to a trampoline that zeroes the line with plain stores,
//!   runs the moved instruction and jumps back. Every other `clzero` stays
//!   emulated (no instruction-length decoder here, on purpose).
//! - `VICTORY_PATCH_CLWB=1`: `clwb m8` (`66 [rex] 0f ae /6`), a write-back
//!   hint whose emulation does nothing on the PC, becomes the multi-byte NOP
//!   Intel recommends for its exact length ([`multi_byte_nop`]), the length
//!   the emulation already decodes. That includes the `clwb` of the
//!   write-back loop the emulation does whole (`clwb [base]; add base, 0x40;
//!   cmp base, limit; jbe`): with a NOP there, the loop runs natively, `add`,
//!   `cmp` and `jbe` a line, ending with the registers and flags the
//!   emulation would leave. SSE4a stays emulated.
//!
//! All switches are off by default: unset, nothing here runs.
//!
//! Safety of the rewrite, which other threads may be executing meanwhile:
//! the bytes replaced must lie in one aligned 8-byte word, written with a
//! single `lock cmpxchg` that also checks they are still the expected ones,
//! so another core fetches either the old instruction or the new one, never
//! a mix (sites that cross a word boundary stay emulated). The handler
//! reads a site's bytes with one load of that word ([`site_bytes`]), so it
//! too sees them whole. A thread that faulted on the old bytes and reaches
//! the handler after the write finds an instruction that cannot fault
//! there, and is resumed to run it ([`rewritten_under_fault`]). For
//! `clzero`, no thread is ever resumed inside the replaced bytes: the
//! emulation of those two loops resumes past the whole loop, and if the
//! loop's bytes changed while it read them (so it zeroed one line and would
//! resume after `clzero`), it resumes at the rewritten site instead, whose
//! trampoline zeroes that line again (same result) and goes on. The page is
//! made writable only around the write (`VirtualProtect`, restored, then
//! `FlushInstructionCache`), under a lock. Only sites inside an executable
//! section of a mapped title image are touched; each site is decided once
//! (patched or refused), logged once, and counted.
//!
//! Known shortcut: trampolines have no unwind data, so a fault in one (a
//! line the title had no right to clear, which `clzero` would fault on too)
//! or a stack walk while a thread is in one sees a frame it cannot unwind.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use victory_pe::ImageInfo;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect, VirtualQuery,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Turns the `monitorx`/`mwaitx` rewrite on.
pub(crate) const WAITS_ENV: &str = "VICTORY_PATCH_WAITS";
/// Turns the `clzero` loop rewrite on: `1` the first loop form only, `2`
/// both (see the module's notes on the second form's risk).
pub(crate) const CLZERO_ENV: &str = "VICTORY_PATCH_CLZERO";
/// Turns the `clwb` rewrite on.
pub(crate) const CLWB_ENV: &str = "VICTORY_PATCH_CLWB";

static WAITS: AtomicBool = AtomicBool::new(false);
/// 0 off; 1 the first `clzero` loop form; 2 both forms.
static CLZERO: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static CLWB: AtomicBool = AtomicBool::new(false);
/// Sites rewritten so far.
static PATCHED: AtomicUsize = AtomicUsize::new(0);

const MONITORX: [u8; 3] = [0x0f, 0x01, 0xfa];
const MWAITX: [u8; 3] = [0x0f, 0x01, 0xfb];
const CLZERO_OP: [u8; 3] = [0x0f, 0x01, 0xfc];
/// `nop dword [rax]`.
const NOP3: [u8; 3] = [0x0f, 0x1f, 0x00];
/// `pause; nop`.
const PAUSE_NOP: [u8; 3] = [0xf3, 0x90, 0x90];
/// `jmp rel32`.
const JMP: u8 = 0xe9;
/// `add rax, 0x40`.
const ADD_RAX_40: [u8; 4] = [0x48, 0x83, 0xc0, 0x40];

/// Reads the switches; call before title code runs.
pub(crate) fn read_switches() {
    let on = |name| std::env::var_os(name).is_some_and(|value| value == "1");
    WAITS.store(on(WAITS_ENV), Ordering::Relaxed);
    let clzero = match std::env::var(CLZERO_ENV).as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => 0,
    };
    CLZERO.store(clzero, Ordering::Relaxed);
    CLWB.store(on(CLWB_ENV), Ordering::Relaxed);
}

/// Some rewrite is on: sites may change under a fault.
fn any_switch() -> bool {
    WAITS.load(Ordering::Relaxed) || CLZERO.load(Ordering::Relaxed) != 0 || CLWB.load(Ordering::Relaxed)
}

/// The multi-byte NOPs the Intel SDM recommends (`NOP` instruction), by
/// length: `nop`, `66 nop`, then `nop dword/word` with a memory operand
/// (`0f 1f /0`) whose ModRM, SIB and zero displacement set the length.
const NOPS: [&[u8]; 9] = [
    &[0x90],
    &[0x66, 0x90],
    &[0x0f, 0x1f, 0x00],
    &[0x0f, 0x1f, 0x40, 0x00],
    &[0x0f, 0x1f, 0x44, 0x00, 0x00],
    &[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00],
    &[0x0f, 0x1f, 0x80, 0x00, 0x00, 0x00, 0x00],
    &[0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
    &[0x66, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
];

/// One NOP instruction of exactly `len` bytes (1 to 9), or `None`.
fn multi_byte_nop(len: usize) -> Option<&'static [u8]> {
    NOPS.get(len.checked_sub(1)?).copied()
}

/// What replaces a `clwb` of `len` bytes at `site`: a NOP of that length,
/// if the bytes lie in one aligned word (the only atomic write there is).
fn clwb_replacement(site: usize, len: usize) -> Result<&'static [u8], &'static str> {
    if !fits_one_word(site, len) {
        return Err("crosses an 8-byte boundary, cannot be written atomically");
    }
    multi_byte_nop(len).ok_or("no NOP of that length")
}

/// What replaces a `monitorx`/`mwaitx`, or `None` for other bytes.
fn wait_replacement(code: [u8; 3]) -> Option<[u8; 3]> {
    match code {
        MONITORX => Some(NOP3),
        MWAITX => Some(PAUSE_NOP),
        _ => None,
    }
}

/// Bytes of `clzero` plus the instruction after it that a trampoline takes
/// over, or `None` to leave the site emulated. `code` starts at the
/// `clzero`; `skipped` is how far the emulation moved `rip`, which says
/// which loop it recognised: 12 for `clzero; add rax, 0x40; cmp rax, b; jb`,
/// 8 for `mov rax, a; add a, 0x40; clzero; cmp a, b; jb`. The moved
/// instruction must use no memory and nothing relative to `rip`.
fn clzero_span(code: &[u8], skipped: u64) -> Option<usize> {
    if code.get(..3)? != CLZERO_OP {
        return None;
    }
    match skipped {
        12 => (code.get(3..7)? == ADD_RAX_40).then_some(7),
        8 => {
            // cmp r64, r64: REX.W 39 /r with a register operand.
            let &[0x48..=0x4f, 0x39, modrm] = code.get(3..6)? else {
                return None;
            };
            (modrm >> 6 == 3).then_some(6)
        }
        _ => None,
    }
}

/// `len` bytes at `site` lie in one aligned 8-byte word.
fn fits_one_word(site: usize, len: usize) -> bool {
    (site & 7) + len <= 8
}

/// `word` (little-endian code bytes) with `new` at byte `offset`, if the
/// bytes there are `expect`.
fn splice(word: u64, offset: usize, expect: &[u8], new: &[u8]) -> Option<u64> {
    let mut bytes = word.to_le_bytes();
    let at = bytes.get_mut(offset..offset + expect.len())?;
    if at != expect || new.len() != expect.len() {
        return None;
    }
    at.copy_from_slice(new);
    Some(u64::from_le_bytes(bytes))
}

/// `jmp rel32` placed at `from` to `to`, if in reach.
fn jump(from: usize, to: usize) -> Option<[u8; 5]> {
    let rel = (to as i64).wrapping_sub(from as i64 + 5);
    let rel = i32::try_from(rel).ok()?;
    let [a, b, c, d] = rel.to_le_bytes();
    Some([JMP, a, b, c, d])
}

/// A `clzero` trampoline placed at `at`: zeroes the line at `rax` aligned
/// down to 64 bytes, keeping `rax`, then runs `moved` and jumps to `back`.
/// Status flags are changed before `moved`, which sets all of them itself.
fn clzero_trampoline(at: usize, moved: &[u8], back: usize) -> Option<Vec<u8>> {
    // push rax; and rax, -64
    let mut code = vec![0x50, 0x48, 0x83, 0xe0, 0xc0];
    // mov qword [rax], 0
    code.extend([0x48, 0xc7, 0x00, 0, 0, 0, 0]);
    for disp in (8..64).step_by(8) {
        // mov qword [rax + disp8], 0
        code.extend([0x48, 0xc7, 0x40, disp as u8, 0, 0, 0, 0]);
    }
    // pop rax
    code.push(0x58);
    code.extend_from_slice(moved);
    code.extend(jump(at + code.len(), back)?);
    Some(code)
}

/// The site's bytes are an instruction that cannot raise #UD and that a
/// rewrite here writes: one that changed under a fault. `0f 1f` covers
/// [`NOP3`] and the `clwb` NOPs of 3 to 8 bytes, `66 90` and `66 0f 1f` the
/// others.
fn is_rewritten(code: [u8; 3]) -> bool {
    code == PAUSE_NOP
        || code[0] == JMP
        || code[..2] == [0x0f, 0x1f]
        || code[..2] == [0x66, 0x90]
        || code == [0x66, 0x0f, 0x1f]
}

/// `[site, site + len)` lies inside one of `ranges` (`[start, end)`).
fn inside(ranges: &[(usize, usize)], site: usize, len: usize) -> bool {
    let Some(end) = site.checked_add(len) else {
        return false;
    };
    ranges.iter().any(|&(start, stop)| start <= site && end <= stop)
}

/// The executable sections of an image mapped at `base`, as `[start, end)`.
fn executable_ranges(base: usize, info: &ImageInfo) -> Vec<(usize, usize)> {
    const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
    info.sections
        .iter()
        .filter(|s| s.characteristics & IMAGE_SCN_MEM_EXECUTE != 0 && s.virtual_size != 0)
        .map(|s| {
            let start = base + s.rva as usize;
            (start, start + s.virtual_size as usize)
        })
        .collect()
}

/// Trampoline memory: chunks of [`CHUNK`] bytes cut into [`SLOT`]s.
const CHUNK: usize = 0x1_0000;
const SLOT: usize = 128;
/// How far from a site a trampoline may be: under 2 GiB either way.
const REACH: usize = 0x7fff_0000 - CHUNK;

struct Chunk {
    base: usize,
    used: usize,
}

struct State {
    /// Executable ranges of the mapped title images.
    code: Vec<(usize, usize)>,
    /// Sites decided, patched or refused: each is decided once.
    seen: BTreeSet<usize>,
    chunks: Vec<Chunk>,
}

static STATE: Mutex<State> = Mutex::new(State {
    code: Vec::new(),
    seen: BTreeSet::new(),
    chunks: Vec::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A title image was mapped at `base`: its code may be rewritten.
pub(crate) fn note_title_code(base: usize, info: &ImageInfo) {
    let ranges = executable_ranges(base, info);
    let mut state = state();
    state.code.retain(|r| !ranges.contains(r));
    state.code.extend(ranges);
}

/// The title image at `base`, `size_of_image` bytes, is about to be
/// unmapped: forget its code and its sites, as another image may come there.
pub(crate) fn forget_title_code(base: usize, size_of_image: usize) {
    let end = base + size_of_image;
    let mut state = state();
    state.code.retain(|&(start, _)| !(base..end).contains(&start));
    state.seen.retain(|site| !(base..end).contains(site));
}

/// A `monitorx` or `mwaitx` with bytes `code` was just emulated at `site`.
pub(crate) fn after_wait(site: usize, code: [u8; 3]) {
    if !WAITS.load(Ordering::Relaxed) {
        return;
    }
    let Some(new) = wait_replacement(code) else {
        return;
    };
    let what = if code == MONITORX { "monitorx" } else { "mwaitx" };
    decide(site, what, |state| {
        if !inside(&state.code, site, 3) {
            return Err("outside the title's code");
        }
        if !fits_one_word(site, 3) {
            return Err("crosses an 8-byte boundary, cannot be written atomically");
        }
        // SAFETY: title code, the bytes checked as they are written.
        unsafe { write_code(site, &code, &new) }
    });
}

/// A `clzero` at `site` was just emulated, moving `rip` by `skipped`.
pub(crate) fn after_clzero(site: usize, skipped: u64) {
    let level = CLZERO.load(Ordering::Relaxed);
    if level == 0 {
        return;
    }
    let what = match skipped {
        12 => "clzero, first loop form (clzero; add rax, 0x40; cmp; jb)",
        8 => "clzero, second loop form (mov rax, a; add a, 0x40; clzero; cmp a, b; jb)",
        _ => "clzero",
    };
    decide(site, what, |state| {
        if skipped == 8 && level < 2 {
            return Err("second loop form (its moved instruction is the loop's condition): only with VICTORY_PATCH_CLZERO=2");
        }
        // Both loops were matched with their bytes inside the page (8 and
        // 12 bytes from `site`), so 8 are readable.
        let code = if matches!(skipped, 8 | 12) {
            // SAFETY: as above.
            unsafe { (site as *const [u8; 8]).read_unaligned() }
        } else {
            return Err("not one of the two recognised loops, left emulated");
        };
        let span = clzero_span(&code, skipped).ok_or("unexpected bytes after clzero")?;
        if !inside(&state.code, site, span) {
            return Err("outside the title's code");
        }
        if !fits_one_word(site, span) {
            return Err("crosses an 8-byte boundary, cannot be written atomically");
        }
        let slot = state.slot_near(site, site + span)?;
        let trampoline = clzero_trampoline(slot, &code[3..span], site + span)
            .ok_or("trampoline out of reach")?;
        let mut new = [0x90; 8];
        new[..5].copy_from_slice(&jump(site, slot).ok_or("trampoline out of reach")?);
        // SAFETY: a fresh slot no code jumps to yet.
        unsafe { write_trampoline(slot, &trampoline) }?;
        // SAFETY: title code, the bytes checked as they are written.
        unsafe { write_code(site, &code[..span], &new[..span]) }?;
        state.take_slot(slot);
        Ok(())
    });
}

/// A `clwb` of bytes `code` (its whole length, from [`crate::crash`]'s
/// decoder) was just emulated at `site`, alone or as the head of the
/// write-back loop the emulation did whole (`in_loop`).
pub(crate) fn after_clwb(site: usize, code: &[u8], in_loop: bool) {
    if !CLWB.load(Ordering::Relaxed) {
        return;
    }
    let what = if in_loop { "clwb in a write-back loop" } else { "clwb" };
    decide(site, what, |state| {
        if !inside(&state.code, site, code.len()) {
            return Err("outside the title's code");
        }
        let nop = clwb_replacement(site, code.len())?;
        // SAFETY: title code, the bytes checked as they are written.
        unsafe { write_code(site, code, nop) }
    });
}

/// An illegal-instruction fault at a site whose bytes are now `code`: true
/// if a rewrite there replaced the instruction after the CPU fetched it, so
/// the thread is to run the new one, at the same `rip`. Bytes that cannot
/// raise #UD can only be there for that reason; this does not wait for the
/// rewrite to be counted, as the faulting thread may see it first.
pub(crate) fn rewritten_under_fault(code: [u8; 3]) -> bool {
    any_switch() && is_rewritten(code)
}

/// The three bytes at a faulting `rip`. With a switch on, read with one
/// load of the aligned word holding them when they fit in one, the only
/// sites ever rewritten, so a rewrite in progress is seen whole or not at
/// all.
///
/// # Safety
/// `site` is a faulting instruction's address: its bytes are readable.
pub(crate) unsafe fn site_bytes(site: usize) -> [u8; 3] {
    if any_switch() && fits_one_word(site, 3) {
        // SAFETY: an aligned word never leaves its page, the site's.
        let word = unsafe { AtomicU64::from_ptr((site & !7) as *mut u64) }.load(Ordering::Acquire);
        let (bytes, at) = (word.to_le_bytes(), site & 7);
        return [bytes[at], bytes[at + 1], bytes[at + 2]];
    }
    // SAFETY: forwarded from the caller.
    unsafe { (site as *const [u8; 3]).read_unaligned() }
}

/// `code`, bytes copied one by one from a faulting `site`, with those that
/// lie in the site's aligned word read again with one load of it, when a
/// switch is on: a rewrite (always inside that word) in progress during the
/// copy is then seen whole or not at all, never as a mix to decode.
///
/// # Safety
/// `site` is a faulting instruction's address: its bytes are readable.
pub(crate) unsafe fn reread_site_word(site: usize, code: &mut [u8]) {
    if !any_switch() {
        return;
    }
    // SAFETY: an aligned word never leaves its page, the site's.
    let word = unsafe { AtomicU64::from_ptr((site & !7) as *mut u64) }.load(Ordering::Acquire);
    let at = site & 7;
    let n = code.len().min(8 - at);
    code[..n].copy_from_slice(&word.to_le_bytes()[at..at + n]);
}

/// Runs `patch` for `site` once, logging the outcome once. Skipped (to be
/// tried at the site's next fault) if another thread holds the state: an
/// exception handler should not wait.
fn decide(
    site: usize,
    what: &'static str,
    patch: impl FnOnce(&mut State) -> Result<(), &'static str>,
) {
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    if !state.seen.insert(site) {
        return;
    }
    let at = crate::crash::locate(site);
    match patch(&mut state) {
        Ok(()) => {
            let total = PATCHED.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(what, site = %at, total, "AMD-only instruction site rewritten");
        }
        Err(why) => {
            tracing::info!(what, site = %at, why, "AMD-only instruction site kept emulated");
        }
    }
}

impl State {
    /// A free trampoline slot within reach of `site` and of `back`.
    fn slot_near(&mut self, site: usize, back: usize) -> Result<usize, &'static str> {
        let near = |slot: usize| slot.abs_diff(site) < REACH && slot.abs_diff(back) < REACH;
        if let Some(c) = self.chunks.iter().find(|c| c.used + SLOT <= CHUNK && near(c.base)) {
            return Ok(c.base + c.used);
        }
        // SAFETY: allocates fresh memory only.
        let base = unsafe { alloc_near(site) }.ok_or("no memory within 2 GiB of the site")?;
        self.chunks.push(Chunk { base, used: 0 });
        Ok(base)
    }

    /// The slot `slot_near` gave is in use.
    fn take_slot(&mut self, slot: usize) {
        if let Some(c) = self.chunks.iter_mut().find(|c| c.base + c.used == slot) {
            c.used += SLOT;
        }
    }
}

/// Commits [`CHUNK`] bytes of executable memory within [`REACH`] of
/// `site`, searching down from it, then up.
///
/// # Safety
/// Only reserves free address space.
unsafe fn alloc_near(site: usize) -> Option<usize> {
    let granule = CHUNK;
    let low = site.saturating_sub(REACH).max(granule);
    let high = site.saturating_add(REACH);
    let query = |at: usize| {
        // SAFETY: a zeroed MEMORY_BASIC_INFORMATION is valid; only the
        // address space's bookkeeping is read.
        unsafe {
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let n = VirtualQuery(at as *const _, &mut info, size_of_val(&info));
            (n != 0).then_some(info)
        }
    };
    let try_at = |at: usize| {
        // SAFETY: `at` is in a free region; nothing is overwritten.
        let p = unsafe {
            VirtualAlloc(at as *const _, CHUNK, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READ)
        };
        (!p.is_null()).then_some(p as usize)
    };
    // Down.
    let mut at = (site & !(granule - 1)).checked_sub(granule)?;
    while at >= low {
        let info = query(at)?;
        let region = info.BaseAddress as usize;
        if info.State == MEM_FREE && at + CHUNK <= region + info.RegionSize {
            if let Some(p) = try_at(at) {
                return Some(p);
            }
        }
        let next = (if info.State == MEM_FREE { at } else { region }) & !(granule - 1);
        let Some(next) = next.checked_sub(granule) else { break };
        at = next.min(at - granule);
    }
    // Up.
    let mut at = (site & !(granule - 1)) + granule;
    while at + CHUNK <= high {
        let info = query(at)?;
        let region = info.BaseAddress as usize;
        let end = region + info.RegionSize;
        if info.State == MEM_FREE && at + CHUNK <= end {
            if let Some(p) = try_at(at) {
                return Some(p);
            }
        }
        at = (end.checked_add(granule - 1)? & !(granule - 1)).max(at + granule);
    }
    None
}

/// Writes `code` into a trampoline slot of ours.
///
/// # Safety
/// `slot` is a slot `State::slot_near` gave, which no code jumps to yet.
unsafe fn write_trampoline(slot: usize, code: &[u8]) -> Result<(), &'static str> {
    if code.len() > SLOT {
        return Err("trampoline too long");
    }
    let mut old = 0;
    // SAFETY: our chunk; other slots in it keep executable rights.
    if unsafe { VirtualProtect(slot as *const _, SLOT, PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        return Err("VirtualProtect failed on the trampoline");
    }
    // SAFETY: writable now, and ours.
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), slot as *mut u8, code.len());
        VirtualProtect(slot as *const _, SLOT, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), slot as *const _, SLOT);
    }
    Ok(())
}

/// Replaces `expect` at `site` with `new` in one atomic write of the
/// aligned 8-byte word holding them, if they are still `expect`.
///
/// # Safety
/// `site` is title code inside an executable section, and the bytes fit in
/// one aligned word ([`fits_one_word`]).
unsafe fn write_code(site: usize, expect: &[u8], new: &[u8]) -> Result<(), &'static str> {
    if !fits_one_word(site, expect.len()) || new.len() != expect.len() {
        return Err("not one aligned word");
    }
    let word_at = site & !7;
    let mut old = 0;
    // SAFETY: the word is inside the title's code; protection restored below.
    if unsafe { VirtualProtect(word_at as *const _, 8, PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        return Err("VirtualProtect failed");
    }
    // SAFETY: aligned, writable now, and title code stays mapped.
    let word = unsafe { AtomicU64::from_ptr(word_at as *mut u64) };
    let now = word.load(Ordering::SeqCst);
    let written = match splice(now, site & 7, expect, new) {
        None => Err("the bytes are not the expected ones"),
        Some(next) => word
            .compare_exchange(now, next, Ordering::SeqCst, Ordering::SeqCst)
            .map(drop)
            .map_err(|_| "the bytes changed while being written"),
    };
    // SAFETY: restores what was there; the bytes are written.
    unsafe {
        let mut ignored = 0;
        VirtualProtect(word_at as *const _, 8, old, &mut ignored);
        FlushInstructionCache(GetCurrentProcess(), word_at as *const _, 8);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_get_their_replacements_and_nothing_else_does() {
        assert_eq!(wait_replacement(MONITORX), Some(NOP3));
        assert_eq!(wait_replacement(MWAITX), Some(PAUSE_NOP));
        assert_eq!(wait_replacement(CLZERO_OP), None);
        assert_eq!(wait_replacement([0x0f, 0x01, 0xfd]), None);
        assert_eq!(wait_replacement([0x90, 0x90, 0x90]), None);
    }

    #[test]
    fn only_the_two_recognised_clzero_loops_get_a_span() {
        // clzero; add rax, 0x40; cmp rax, r10; jb
        let first = [0x0f, 0x01, 0xfc, 0x48, 0x83, 0xc0, 0x40, 0x4c];
        assert_eq!(clzero_span(&first, 12), Some(7));
        // mov rax, rdx; add rdx, 0x40; [clzero; cmp rdx, r8; jb]
        let second = [0x0f, 0x01, 0xfc, 0x4c, 0x39, 0xc2, 0x72, 0xf1];
        assert_eq!(clzero_span(&second, 8), Some(6));
        // A lone clzero (the emulation moved 3), or bytes that do not match.
        assert_eq!(clzero_span(&first, 3), None);
        assert_eq!(clzero_span(&second, 12), None);
        assert_eq!(clzero_span(&first, 8), None);
        // cmp with a memory operand is not moved.
        let memory = [0x0f, 0x01, 0xfc, 0x48, 0x39, 0x02, 0x72, 0xf1];
        assert_eq!(clzero_span(&memory, 8), None);
        // Not a clzero at all.
        let mut other = first;
        other[2] = 0xfa;
        assert_eq!(clzero_span(&other, 12), None);
        assert_eq!(clzero_span(&first[..5], 12), None);
    }

    #[test]
    fn a_site_is_written_only_inside_one_aligned_word() {
        assert!(fits_one_word(0x1000, 3));
        assert!(fits_one_word(0x1005, 3));
        assert!(!fits_one_word(0x1006, 3));
        assert!(!fits_one_word(0x1007, 3));
        assert!(fits_one_word(0x1001, 7));
        assert!(!fits_one_word(0x1002, 7));
        assert!(fits_one_word(0x1002, 6));
        assert!(!fits_one_word(0x1003, 6));
    }

    #[test]
    fn splice_checks_the_bytes_it_replaces() {
        let word = u64::from_le_bytes([0xaa, 0xbb, 0x0f, 0x01, 0xfa, 0xcc, 0xdd, 0xee]);
        let next = splice(word, 2, &MONITORX, &NOP3).unwrap();
        assert_eq!(next.to_le_bytes(), [0xaa, 0xbb, 0x0f, 0x1f, 0x00, 0xcc, 0xdd, 0xee]);
        assert_eq!(splice(word, 2, &MWAITX, &PAUSE_NOP), None);
        assert_eq!(splice(word, 1, &MONITORX, &NOP3), None);
        assert_eq!(splice(word, 6, &MONITORX, &NOP3), None);
        assert_eq!(splice(word, 2, &MONITORX, &NOP3[..2]), None);
    }

    #[test]
    fn a_word_in_a_buffer_of_ours_is_rewritten_in_place() {
        // The atomic exchange `write_code` does, on our own memory.
        let buffer = AtomicU64::new(u64::from_le_bytes([0x90, 0x0f, 0x01, 0xfb, 0xc3, 0, 0, 0]));
        let now = buffer.load(Ordering::SeqCst);
        let next = splice(now, 1, &MWAITX, &PAUSE_NOP).unwrap();
        assert!(buffer.compare_exchange(now, next, Ordering::SeqCst, Ordering::SeqCst).is_ok());
        let bytes = buffer.load(Ordering::SeqCst).to_le_bytes();
        assert_eq!(bytes, [0x90, 0xf3, 0x90, 0x90, 0xc3, 0, 0, 0]);
        // A second pass finds no mwaitx left.
        assert_eq!(splice(buffer.load(Ordering::SeqCst), 1, &MWAITX, &PAUSE_NOP), None);
    }

    #[test]
    fn jumps_reach_both_ways_within_2_gib_only() {
        assert_eq!(jump(0x1000, 0x1005), Some([JMP, 0, 0, 0, 0]));
        assert_eq!(jump(0x1000, 0x1000), Some([JMP, 0xfb, 0xff, 0xff, 0xff]));
        assert_eq!(jump(0x1_0000_0000, 0x1_7fff_0000).map(|j| j[0]), Some(JMP));
        assert_eq!(jump(0x1_0000_0000, 0x1_8000_0005), None);
        assert_eq!(jump(0x1_8000_0000, 0x1000), None);
    }

    #[test]
    fn the_trampoline_zeroes_a_line_runs_the_moved_instruction_and_returns() {
        let at = 0x7ff0_0000_0000;
        let back = at - 0x1234;
        let code = clzero_trampoline(at, &ADD_RAX_40, back).unwrap();
        assert!(code.len() <= SLOT);
        assert_eq!(code[..5], [0x50, 0x48, 0x83, 0xe0, 0xc0]);
        assert_eq!(code[5..12], [0x48, 0xc7, 0x00, 0, 0, 0, 0]);
        for (i, store) in code[12..68].chunks(8).enumerate() {
            assert_eq!(store, [0x48, 0xc7, 0x40, 8 * (i as u8 + 1), 0, 0, 0, 0]);
        }
        assert_eq!(code[68], 0x58);
        assert_eq!(code[69..73], ADD_RAX_40);
        let jmp = &code[73..];
        assert_eq!(jmp.len(), 5);
        let rel = i32::from_le_bytes(jmp[1..5].try_into().unwrap());
        assert_eq!(jmp[0], JMP);
        assert_eq!((at + code.len()) as i64 + i64::from(rel), back as i64);
        assert_eq!(clzero_trampoline(at, &ADD_RAX_40, 0x1000), None);
    }

    #[test]
    fn rewritten_bytes_are_recognised_and_the_originals_are_not() {
        assert!(is_rewritten(NOP3));
        assert!(is_rewritten(PAUSE_NOP));
        assert!(is_rewritten([JMP, 0x12, 0x34]));
        for len in 2..=9 {
            let nop = multi_byte_nop(len).unwrap();
            assert!(is_rewritten([nop[0], nop[1], nop.get(2).copied().unwrap_or(0xcc)]));
        }
        assert!(!is_rewritten(MONITORX));
        assert!(!is_rewritten(MWAITX));
        assert!(!is_rewritten(CLZERO_OP));
        assert!(!is_rewritten([0x0f, 0x0b, 0x90]));
        // clwb, with and without REX.
        assert!(!is_rewritten([0x66, 0x0f, 0xae]));
        assert!(!is_rewritten([0x66, 0x41, 0x0f]));
        // SSE4a extrq/insertq.
        assert!(!is_rewritten([0x66, 0x0f, 0x78]));
        assert!(!is_rewritten([0xf2, 0x0f, 0x79]));
    }

    /// The length of a NOP as the CPU decodes it, checking it is one:
    /// `90`, `66 90`, or `[66] 0f 1f /0` with a memory operand whose SIB
    /// and displacement bytes are zero.
    fn decoded_nop_len(code: &[u8]) -> usize {
        match code {
            [0x90, ..] => return 1,
            [0x66, 0x90, ..] => return 2,
            _ => {}
        }
        let at = usize::from(code[0] == 0x66);
        assert_eq!(code[at..at + 2], [0x0f, 0x1f], "{code:02x?}");
        let modrm = code[at + 2];
        let (mode, reg, rm) = (modrm >> 6, (modrm >> 3) & 7, modrm & 7);
        assert_eq!(reg, 0, "0f 1f /0 is the NOP");
        assert_ne!(mode, 3, "a memory operand, as Intel's forms have");
        assert!(mode != 0 || rm != 5, "no rip-relative form");
        let mut len = at + 3;
        if rm == 4 {
            len += 1; // SIB
        }
        len += match mode {
            1 => 1,
            2 => 4,
            _ => 0,
        };
        assert!(code[at + 3..len].iter().all(|&b| b == 0), "{code:02x?}");
        len
    }

    #[test]
    fn every_nop_has_its_length_and_is_one_instruction() {
        assert_eq!(multi_byte_nop(0), None);
        assert_eq!(multi_byte_nop(10), None);
        assert_eq!(multi_byte_nop(3), Some(&NOP3[..]));
        for len in 1..=9 {
            let nop = multi_byte_nop(len).unwrap();
            assert_eq!(nop.len(), len);
            assert_eq!(decoded_nop_len(nop), len);
        }
        assert_eq!(multi_byte_nop(2), Some(&[0x66, 0x90][..]));
        assert_eq!(multi_byte_nop(4), Some(&[0x0f, 0x1f, 0x40, 0x00][..]));
        assert_eq!(multi_byte_nop(5), Some(&[0x0f, 0x1f, 0x44, 0x00, 0x00][..]));
        assert_eq!(multi_byte_nop(6), Some(&[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00][..]));
        assert_eq!(multi_byte_nop(7), Some(&[0x0f, 0x1f, 0x80, 0, 0, 0, 0][..]));
        assert_eq!(multi_byte_nop(8), Some(&[0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0][..]));
        assert_eq!(multi_byte_nop(9), Some(&[0x66, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0][..]));
    }

    #[test]
    fn a_clwb_is_replaced_only_when_it_fits_one_aligned_word() {
        // clwb [rax] (4 bytes), [r9] (5), [rax+0x40] (5), [rax+disp32] (8).
        assert_eq!(clwb_replacement(0x1000, 4), Ok(multi_byte_nop(4).unwrap()));
        assert_eq!(clwb_replacement(0x1004, 4), Ok(multi_byte_nop(4).unwrap()));
        assert!(clwb_replacement(0x1005, 4).is_err());
        assert_eq!(clwb_replacement(0x1003, 5), Ok(multi_byte_nop(5).unwrap()));
        assert!(clwb_replacement(0x1004, 5).is_err());
        assert_eq!(clwb_replacement(0x1000, 8), Ok(multi_byte_nop(8).unwrap()));
        assert!(clwb_replacement(0x1001, 8).is_err());
        // A 10-byte clwb (REX, SIB, disp32) never fits.
        assert!(clwb_replacement(0x1000, 10).is_err());
    }

    #[test]
    fn a_clwb_in_a_buffer_of_ours_becomes_a_nop_of_its_length() {
        // clwb [rcx]; add rcx, 0x40 (a write-back loop's head), at offset 1.
        let code = [0x66, 0x0f, 0xae, 0x31];
        let word = u64::from_le_bytes([0x90, 0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1]);
        let nop = clwb_replacement(0x1001, code.len()).unwrap();
        let next = splice(word, 1, &code, nop).unwrap();
        assert_eq!(next.to_le_bytes(), [0x90, 0x0f, 0x1f, 0x40, 0x00, 0x48, 0x83, 0xc1]);
        // Done once: the clwb is gone.
        assert_eq!(splice(next, 1, &code, nop), None);
    }

    #[test]
    fn only_executable_sections_of_title_images_count() {
        let ranges = [(0x1000, 0x2000), (0x5000, 0x5100)];
        assert!(inside(&ranges, 0x1000, 3));
        assert!(inside(&ranges, 0x1ffd, 3));
        assert!(!inside(&ranges, 0x1ffe, 3));
        assert!(!inside(&ranges, 0xfff, 3));
        assert!(inside(&ranges, 0x50f9, 7));
        assert!(!inside(&ranges, 0x3000, 3));
        assert!(!inside(&ranges, usize::MAX - 1, 3));
        assert!(!inside(&[], 0x1000, 3));
    }
}
