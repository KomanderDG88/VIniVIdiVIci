//! The switch is read once per process, so each test of the recording runs
//! in a child process of this test binary with `VICTORY_LAZY_REMAP=1` set
//! ([`in_child`]); the parent only starts it and checks it passed. The
//! eager path's own tests (`xmem::tests`) run in a child too, once with the
//! switch unset (nothing changes without it) and once with it on (the
//! requests they make answer the same recorded).

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::Memory::{MEM_FREE, MEM_RESERVE};

use super::super::{
    BasicInformation, PAGES_64K, XMemAllocatePhysicalPages, XMemFreePhysicalPages, XMemMapPhysicalPages,
    XMemVirtualAlloc, XMemVirtualQuery, map_alias, shown_at, write_physical,
};
use super::*;

/// Set in the child processes, so that they run the test's body.
const CHILD: &str = "VICTORY_LAZY_REMAP_TEST_CHILD";

/// The test binary run again for the tests `filter` names (`exact`: that
/// one test), with the switch on or unset; panics unless they all pass and
/// at least one ran.
fn run_child(filter: &str, exact: bool, on: bool) {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command.arg(filter);
    if exact {
        command.arg("--exact");
    }
    command.args(["--nocapture"]).env(CHILD, "1");
    if on {
        command.env("VICTORY_LAZY_REMAP", "1");
    } else {
        command.env_remove("VICTORY_LAZY_REMAP");
    }
    let out = command.output().expect("the test binary runs");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "child `{filter}` (switch {on}) failed:\n{text}");
    assert!(!text.contains("running 0 tests"), "child `{filter}` ran nothing:\n{text}");
}

/// Runs `body` with the switch on: in a child process started for this
/// test alone, which runs it.
fn in_child(test: &str, body: impl FnOnce()) {
    if std::env::var_os(CHILD).is_some() {
        assert!(enabled(), "the child has the switch on");
        body();
        return;
    }
    // Test names leave the crate out of the module path.
    let module = module_path!().split_once("::").map_or(module_path!(), |(_, rest)| rest);
    run_child(&format!("{module}::{test}"), true, true);
}

/// `n` pages of one new allocation.
fn allocate(n: usize) -> Vec<usize> {
    let (mut pages, mut given) = (vec![0; n], n);
    assert_eq!(XMemAllocatePhysicalPages(0, &mut given, pages.as_mut_ptr()), 1);
    assert_eq!(given, n);
    pages
}

/// A placeholder reservation of `n` pages, as the title makes them.
fn reserve(n: usize) -> usize {
    let at = XMemVirtualAlloc(std::ptr::null_mut(), n * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE);
    assert!(!at.is_null());
    at as usize
}

/// `pages` mapped eagerly at a reservation of their own: the title's pool
/// view, the source of its page-mapping copies.
fn pool(pages: &[usize]) -> usize {
    let at = reserve(pages.len());
    assert_eq!(XMemMapPhysicalPages(at as *mut c_void, pages.len(), pages.as_ptr()), at as *mut c_void);
    at
}

/// What the system says of `at`.
fn system_state(at: usize) -> u32 {
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe {
        windows_sys::Win32::System::Memory::VirtualQuery(at as *const c_void, &mut info, size_of_val(&info))
    };
    assert_ne!(n, 0);
    info.State
}

/// What the title's `XMemVirtualQuery` says of `at`.
fn query(at: usize) -> BasicInformation {
    // SAFETY: plain data; zero is a valid value.
    let mut info: BasicInformation = unsafe { std::mem::zeroed() };
    // SAFETY: a pseudo handle needs no closing.
    assert_eq!(XMemVirtualQuery(unsafe { GetCurrentProcess() }, at as *const c_void, &mut info), 1);
    info
}

/// The bytes of the recorded page at `at` as our own reads find them
/// (`victory_gpu::recorded`), or `None` if it is not recorded.
fn host_bytes(at: usize) -> Option<Vec<u8>> {
    let pages = host_pages(at, PAGE);
    let page = pages.iter().find(|p| p.at == at)?;
    // SAFETY: a committed page of the stable view, held by `keep`.
    Some(unsafe { std::slice::from_raw_parts(page.ptr, PAGE) }.to_vec())
}

/// The bytes at title address `at`, read in place (a recorded page faults
/// into the handler and is mapped).
fn guest_bytes(at: usize, len: usize) -> Vec<u8> {
    // SAFETY: the callers read pages that are mapped or recorded.
    (0..len).map(|i| unsafe { ((at + i) as *const u8).read_volatile() }).collect()
}

/// Page `k` of a test pool: each byte says its page and its place.
fn pattern(k: usize) -> Vec<u8> {
    (0..PAGE).map(|i| ((i * 7 + k * 53) % 251) as u8).collect()
}

fn fill(pages: &[usize]) {
    for (k, &page) in pages.iter().enumerate() {
        assert!(write_physical(&[page], 0, &pattern(k)));
    }
}

#[test]
fn parse_accepts_only_one() {
    assert!(parse(Some("1".as_ref())));
    for no in ["0", "true", "yes", " 1", ""] {
        assert!(!parse(Some(no.as_ref())), "{no:?}");
    }
    assert!(!parse(None));
}

/// Without the switch, nothing changes: the eager path's tests, run in a
/// child with `VICTORY_LAZY_REMAP` unset, where the first asking turns
/// nothing on and every request is the eager one.
#[test]
fn the_eager_tests_pass_with_the_switch_unset() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    run_child("modules::xmem::tests::", false, false);
}

/// The same tests with the switch on: page-mapping copies into their
/// reservations are recorded, and every answer they check is the eager
/// path's.
#[test]
fn the_eager_tests_pass_with_the_switch_on() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    run_child("modules::xmem::tests::", false, true);
}

/// The switch unset: a page-mapping copy maps a view at once.
#[test]
fn without_the_switch_a_copy_maps_at_once() {
    if std::env::var_os(CHILD).is_none() {
        let module = module_path!().split_once("::").map_or(module_path!(), |(_, rest)| rest);
        run_child(&format!("{module}::without_the_switch_a_copy_maps_at_once"), true, false);
        return;
    }
    assert!(!enabled());
    let pages = allocate(1);
    fill(&pages);
    let (from, to) = (pool(&pages), reserve(1));
    assert!(map_alias(to as u64, from as u64, PAGE as u64));
    assert_eq!(system_state(to), MEM_COMMIT);
    assert!(host_pages(to, PAGE).is_empty());
    assert!(map_alias(to as u64, 0, PAGE as u64));
    assert_eq!(system_state(to), MEM_RESERVE);
}

#[test]
fn repeated_replacement_makes_no_system_call() {
    in_child("repeated_replacement_makes_no_system_call", || {
        let pages = allocate(4);
        fill(&pages);
        let (from, to) = (pool(&pages), reserve(2));
        let before = KERNEL_OPS.load(Relaxed);
        for round in 0..400 {
            let (k, j) = (round % 2, round * 7 % 4);
            assert!(map_alias((to + k * PAGE) as u64, (from + j * PAGE) as u64, PAGE as u64));
            assert_eq!(host_bytes(to + k * PAGE), Some(pattern(j)), "round {round}");
        }
        assert_eq!(KERNEL_OPS.load(Relaxed), before, "no system mapping call");
        for k in 0..2 {
            assert_eq!(system_state(to + k * PAGE), MEM_RESERVE, "still reserved to the system");
            // The title sees a committed page, as a load would.
            let info = query(to + k * PAGE);
            assert_eq!(info.host.State, MEM_COMMIT);
            assert_eq!(info.host.RegionSize, PAGE);
        }
        assert!(map_alias(to as u64, 0, 2 * PAGE as u64));
        assert_eq!(KERNEL_OPS.load(Relaxed), before, "nor for unmapping untouched pages");
        assert!(host_pages(to, 2 * PAGE).is_empty());
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 4, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(4, pages.as_ptr()), 1);
    });
}

/// A texture's tiles land on pages in an order of their own: written by
/// page number, read back through the recorded addresses.
#[test]
fn a_tiled_texture_written_by_page_number_reads_back() {
    in_child("a_tiled_texture_written_by_page_number_reads_back", || {
        let pages = allocate(4);
        let (from, to) = (pool(&pages), reserve(4));
        // Tiled: title page k shows pool page `order[k]`.
        let order = [2, 0, 3, 1];
        for (k, &j) in order.iter().enumerate() {
            assert!(map_alias((to + k * PAGE) as u64, (from + j * PAGE) as u64, PAGE as u64));
        }
        // Written after the mapping, by page number, across page seams.
        let data: Vec<u8> = (0..4 * PAGE - 300).map(|i| (i * 31 % 253) as u8).collect();
        assert!(write_physical(&pages, 100, &data));
        let mut physical = vec![0u8; 100];
        physical.extend(&data);
        physical.resize(4 * PAGE, 0);
        for (k, &j) in order.iter().enumerate() {
            let want = &physical[j * PAGE..(j + 1) * PAGE];
            assert_eq!(host_bytes(to + k * PAGE).as_deref(), Some(want), "page {k} through the stable view");
            assert_eq!(guest_bytes(to + k * PAGE, PAGE), want, "page {k} in place");
        }
        // Where the pages show, the recorded addresses included.
        assert!(shown_at(&pages, 0, PAGE).contains(&(to + PAGE, PAGE)));
        assert!(shown_at(&pages, 0, PAGE).contains(&(from, PAGE)));
        assert!(map_alias(to as u64, 0, 4 * PAGE as u64));
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 4, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(4, pages.as_ptr()), 1);
    });
}

#[test]
fn simultaneous_first_touches_all_succeed() {
    in_child("simultaneous_first_touches_all_succeed", || {
        let pages = allocate(2);
        fill(&pages);
        let (from, to) = (pool(&pages), reserve(2));
        assert!(map_alias(to as u64, from as u64, 2 * PAGE as u64));
        let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let gate = gate.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    let at = to + t * 997;
                    // SAFETY: a recorded page.
                    unsafe { (at as *const u8).read_volatile() }
                })
            })
            .collect();
        let want = pattern(0);
        for (t, thread) in threads.into_iter().enumerate() {
            assert_eq!(thread.join().expect("no thread faulted"), want[t * 997]);
        }
        assert_eq!(system_state(to), MEM_COMMIT, "the touched page has its view");
        assert_eq!(system_state(to + PAGE), MEM_RESERVE, "its neighbour does not");
        // A store from a thread that arrives after the view exists.
        // SAFETY: mapped read-write now.
        unsafe { ((to + 5) as *mut u8).write_volatile(0xee) };
        assert_eq!(host_bytes(to).expect("recorded")[5], 0xee);
        assert!(map_alias(to as u64, 0, 2 * PAGE as u64));
        assert_eq!(system_state(to), MEM_RESERVE);
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 2, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(2, pages.as_ptr()), 1);
    });
}

#[test]
fn queries_and_releases_see_the_eager_boundaries() {
    in_child("queries_and_releases_see_the_eager_boundaries", || {
        let pages = allocate(2);
        fill(&pages);
        let (from, to) = (pool(&pages), reserve(4));
        // Pages 1 and 2 of four recorded.
        assert!(map_alias((to + PAGE) as u64, from as u64, 2 * PAGE as u64));
        let info = query(to);
        assert_eq!(info.host.State, MEM_RESERVE);
        assert_eq!(info.host.RegionSize, PAGE, "the region stops at the recorded page");
        let info = query(to + PAGE + 0x1234);
        assert_eq!(info.host.State, MEM_COMMIT);
        assert_eq!(info.host.Type, MEM_MAPPED);
        assert_eq!(info.host.BaseAddress as usize, to + PAGE + 0x1000);
        assert_eq!(info.host.BaseAddress as usize + info.host.RegionSize, to + 2 * PAGE);
        assert_eq!(info.flags & super::super::GRAPHICS, super::super::GRAPHICS);
        // The title's own `VirtualQuery` says the same.
        // SAFETY: plain data; zero is a valid value.
        let mut plain: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(VirtualQuery((to + 2 * PAGE) as *const c_void, &mut plain, size_of_val(&plain)), 0);
        assert_eq!(plain.State, MEM_COMMIT);
        assert_eq!(plain.RegionSize, PAGE);
        // Page 2 touched: a live view, which a release fails on.
        assert_eq!(guest_bytes(to + 2 * PAGE, 1)[0], pattern(1)[0]);
        assert_eq!(VirtualFree((to + 2 * PAGE) as *mut c_void, 0, MEM_RELEASE), 0);
        assert!(host_bytes(to + 2 * PAGE).is_some(), "a failed release changes nothing");
        // Page 1 was never touched: it can be released, and is gone.
        assert_ne!(VirtualFree((to + PAGE) as *mut c_void, 0, MEM_RELEASE), 0);
        assert_eq!(system_state(to + PAGE), MEM_FREE);
        assert!(host_bytes(to + PAGE).is_none());
        // No longer eligible: a copy there is the eager path's, and fails
        // on memory that is not a placeholder.
        assert!(!map_alias((to + PAGE) as u64, from as u64, PAGE as u64));
        assert_eq!(system_state(to + PAGE), MEM_FREE);
        assert!(map_alias((to + 2 * PAGE) as u64, 0, PAGE as u64));
        assert_eq!(system_state(to + 2 * PAGE), MEM_RESERVE);
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 2, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(2, pages.as_ptr()), 1);
    });
}

#[test]
fn aliases_of_aliases_survive_their_source() {
    in_child("aliases_of_aliases_survive_their_source", || {
        let pages = allocate(1);
        fill(&pages);
        let from = pool(&pages);
        let (a, b) = (reserve(1), reserve(1));
        assert!(map_alias(a as u64, from as u64, PAGE as u64));
        // The source is itself a recorded page.
        assert!(map_alias(b as u64, a as u64, PAGE as u64));
        // The source remapped, the pool view gone, the pages freed.
        assert!(map_alias(a as u64, 0, PAGE as u64));
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 1, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(1, pages.as_ptr()), 1);
        assert_eq!(host_bytes(b), Some(pattern(0)));
        assert_eq!(guest_bytes(b, PAGE), pattern(0));
        assert!(host_bytes(a).is_none());
        assert!(map_alias(b as u64, 0, PAGE as u64));
    });
}

#[test]
fn unmapped_pages_are_missing_and_their_neighbours_exact() {
    in_child("unmapped_pages_are_missing_and_their_neighbours_exact", || {
        let pages = allocate(3);
        fill(&pages);
        let (from, to) = (pool(&pages), reserve(3));
        assert!(map_alias(to as u64, from as u64, 3 * PAGE as u64));
        assert!(map_alias((to + PAGE) as u64, 0, PAGE as u64));
        let found: Vec<usize> = host_pages(to, 3 * PAGE).iter().map(|p| p.at).collect();
        assert_eq!(found, [to, to + 2 * PAGE]);
        assert_eq!(query(to + PAGE).host.State, MEM_RESERVE);
        // The bytes beside the hole, up to its edges.
        assert_eq!(guest_bytes(to + PAGE - 16, 16), pattern(0)[PAGE - 16..]);
        assert_eq!(guest_bytes(to + 2 * PAGE, 16), pattern(2)[..16]);
        assert_eq!(host_bytes(to + 2 * PAGE), Some(pattern(2)));
        assert!(map_alias(to as u64, 0, 3 * PAGE as u64));
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 3, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
    });
}

/// A reader of the destination during multi-page replacements: in place
/// with the remapping lock, a whole page is the old one or the new one;
/// in place without it, a page is never a gap.
#[test]
fn a_reader_racing_replacements_never_sees_a_torn_page() {
    in_child("a_reader_racing_replacements_never_sees_a_torn_page", || {
        let (one, two) = (allocate(4), allocate(4));
        for (pages, byte) in [(&one, 1u8), (&two, 2u8)] {
            for &page in pages.iter() {
                assert!(write_physical(&[page], 0, &vec![byte; PAGE]));
            }
        }
        let (a, b, to) = (pool(&one), pool(&two), reserve(4));
        assert!(map_alias(to as u64, a as u64, 4 * PAGE as u64));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let whole = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    for k in 0..4 {
                        let _reading = victory_gpu::title_memory_reading();
                        let page = guest_bytes(to + k * PAGE, PAGE);
                        assert!(page[0] == 1 || page[0] == 2, "{}", page[0]);
                        assert!(page.iter().all(|&x| x == page[0]), "a torn page");
                        reads += 1;
                    }
                }
                reads
            })
        };
        let plain = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    for k in 0..4 {
                        // SAFETY: the page is mapped or recorded throughout.
                        let x = unsafe { ((to + k * PAGE + 4096) as *const u8).read_volatile() };
                        assert!(x == 1 || x == 2, "{x}");
                        reads += 1;
                    }
                }
                reads
            })
        };
        for round in 0..1500 {
            let from = if round % 2 == 0 { b } else { a };
            assert!(map_alias(to as u64, from as u64, 4 * PAGE as u64));
        }
        stop.store(true, Ordering::Relaxed);
        assert!(whole.join().expect("the reader with the lock saw whole pages") > 0);
        assert!(plain.join().expect("the reader without it never faulted") > 0);
        assert!(map_alias(to as u64, 0, 4 * PAGE as u64));
        for (at, pages) in [(a, &one), (b, &two)] {
            assert_eq!(XMemMapPhysicalPages(at as *mut c_void, 4, std::ptr::null()), at as *mut c_void);
            assert_eq!(XMemFreePhysicalPages(4, pages.as_ptr()), 1);
        }
    });
}

#[test]
fn a_page_aliased_onto_itself_fails_and_overlaps_behave_as_before() {
    in_child("a_page_aliased_onto_itself_fails_and_overlaps_behave_as_before", || {
        let pages = allocate(2);
        fill(&pages);
        let (from, to) = (pool(&pages), reserve(3));
        assert!(map_alias(to as u64, from as u64, PAGE as u64));
        // Cleared first, as the eager path: the source is gone.
        assert!(!map_alias(to as u64, to as u64, PAGE as u64));
        assert!(host_bytes(to).is_none());
        assert_eq!(query(to).host.State, MEM_RESERVE);
        // Shifted onto itself by a page: each source page is looked up once
        // the one before is mapped, so both show the first page.
        assert!(map_alias(to as u64, from as u64, 2 * PAGE as u64));
        assert!(map_alias((to + PAGE) as u64, to as u64, 2 * PAGE as u64));
        assert_eq!(host_bytes(to), Some(pattern(0)));
        assert_eq!(host_bytes(to + PAGE), Some(pattern(0)));
        assert_eq!(host_bytes(to + 2 * PAGE), Some(pattern(0)));
        // Unaligned, overflowing, or a null destination: refused, as the
        // eager path refuses them.
        assert!(!map_alias((to + 1) as u64, from as u64, PAGE as u64));
        assert!(!map_alias(to as u64, u64::MAX - 10, PAGE as u64));
        assert!(!map_alias(0, from as u64, PAGE as u64));
        assert!(map_alias(to as u64, from as u64, 0), "no bytes succeed");
        assert!(map_alias(to as u64, 0, 3 * PAGE as u64));
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 2, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(2, pages.as_ptr()), 1);
    });
}

/// A system call writing a recorded page has its view first.
#[test]
fn kernel_writers_get_views_first() {
    in_child("kernel_writers_get_views_first", || {
        let pages = allocate(1);
        fill(&pages);
        let (from, to) = (pool(&pages), reserve(2));
        assert!(map_alias(to as u64, from as u64, PAGE as u64));
        assert_eq!(system_state(to), MEM_RESERVE);
        assert!(materialize(to + 10, 20));
        assert_eq!(system_state(to), MEM_COMMIT);
        assert_eq!(system_state(to + PAGE), MEM_RESERVE);
        // The kernel writes it now (as `ReadFile` would).
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        let bytes = [9u8; 4];
        let mut done = 0;
        // SAFETY: our own process; a mapped destination of four bytes.
        let read = unsafe {
            ReadProcessMemory(GetCurrentProcess(), bytes.as_ptr().cast(), (to + 100) as *mut c_void, 4, &mut done)
        };
        assert_ne!(read, 0);
        assert_eq!(host_bytes(to).expect("recorded")[100..104], [9; 4]);
        // A protection change has its page too.
        let mut old = 0;
        assert_ne!(VirtualProtect(to as *const c_void, PAGE, PAGE_READWRITE, &mut old), 0);
        assert!(map_alias(to as u64, 0, PAGE as u64));
        assert_eq!(XMemMapPhysicalPages(from as *mut c_void, 1, std::ptr::null()), from as *mut c_void);
        assert_eq!(XMemFreePhysicalPages(1, pages.as_ptr()), 1);
    });
}
