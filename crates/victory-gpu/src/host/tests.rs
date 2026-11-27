//! The host's tests that need an adapter (moved out of `host.rs` verbatim, which is at the size
//! the asset policy lets a source file have).

/// Run once with VICTORY_CARD_SYNC_BATCH=0 and once with it set to 1.
#[test]
#[ignore = "requires a D3D12 adapter and the card hash pipeline; run alone"]
fn two_dirty_card_buffers_reach_readback_in_the_same_submission() {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    const MEM_WRITE_WATCH: u32 = 0x20_0000;
    const MIB: usize = 1 << 20;
    const PIECE: usize = 64 << 10;
    let host = super::host().expect("a D3D12 device");
    assert!(host.card_ready(), "the card hash pipeline must be available");
    // SAFETY: a private allocation held through the fenced submissions.
    let allocation = unsafe {
        VirtualAlloc(std::ptr::null(), 2 * MIB, MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH, PAGE_READWRITE)
    };
    assert!(!allocation.is_null());
    let bases = [allocation as usize, allocation as usize + MIB];
    let shadows = bases.map(|base| {
        crate::shadow::register(base, base as u64, MIB as u64);
        let copy = crate::shadow::find(base as u64, MIB as u64).unwrap();
        assert!(copy.carded(), "both title buffers must have a card copy");
        copy
    });
    let readback = host.read_back_buffer(16).unwrap();
    let hash_readback = host.read_back_buffer(64).unwrap();
    let mut previous = [0u128; 4];
    for round in [0x11u32, 0x55] {
        let list = host.new_list().unwrap();
        let mut expected = [0u32; 4];
        for (i, (shadow, &base)) in shadows.iter().zip(&bases).enumerate() {
            for (j, offset) in [32, PIECE + 20].into_iter().enumerate() {
                let k = 2 * i + j;
                expected[k] = round * 0x0101_0101 + k as u32;
                // SAFETY: aligned dword in the private allocation.
                unsafe { ((base + offset) as *mut u32).write_volatile(expected[k]); }
                shadow.refresh(&list, (base + offset) as u64, 4);
                // Request the before-hash for each of the two distinct pieces.
                shadow.gpu_writes(&list, (base + offset) as u64, 4);
                // SAFETY: the readback and the copy outlive this fenced list.
                unsafe { list.list.CopyBufferRegion(&readback, (k * 4) as u64, &shadow.buffer, offset as u64, 4); }
            }
        }
        list.close();
        host.execute(&[&list]);
        let mut mapped = std::ptr::null_mut();
        // SAFETY: execute waited for all four writes, and the buffer holds 16 bytes.
        let got = unsafe {
            readback.Map(0, None, Some(&mut mapped)).unwrap();
            let got = mapped.cast::<[u32; 4]>().read_unaligned();
            readback.Unmap(0, None);
            got
        };
        assert_eq!(got, expected, "both copied pieces of each buffer");
        let hash_list = host.new_list().unwrap();
        for (i, shadow) in shadows.iter().enumerate() {
            let hashes = shadow.card_sides().unwrap().1.expect("before-hashes were made");
            // SAFETY: two consecutive 16-byte piece hashes from each live buffer.
            unsafe { hash_list.list.CopyBufferRegion(&hash_readback, (32 * i) as u64, &hashes, 0, 32); }
        }
        hash_list.close();
        host.execute(&[&hash_list]);
        // SAFETY: all 64 bytes of the readback were written and fenced.
        let current = unsafe {
            hash_readback.Map(0, None, Some(&mut mapped)).unwrap();
            let words = mapped.cast::<[u128; 4]>().read_unaligned();
            hash_readback.Unmap(0, None);
            words
        };
        for (i, hash) in current.iter().enumerate() {
            assert_ne!(*hash, previous[i], "piece {i} must be rehashed");
            assert!(!current[..i].contains(hash), "each piece has different input");
        }
        previous = current;
    }
    for base in bases {
        crate::shadow::unregister(base, base as u64);
    }
    drop(shadows);
    // SAFETY: the lists completed and the copies were unregistered.
    unsafe { VirtualFree(allocation, 0, MEM_RELEASE); }
}

#[test]
fn sparse_copies_preserve_islands_and_observe_remaps() {
    use windows_sys::Win32::System::Memory::*;
    const N: usize = 5 << 16;
    // SAFETY: we reserve a private region, commit selected pages inside it,
    // and only directly access the pages while they are writable.
    unsafe {
        let base = VirtualAlloc(std::ptr::null(), N, MEM_RESERVE, PAGE_NOACCESS).cast::<u8>();
        assert!(!base.is_null());
        let mut expected = vec![0; N];
        for (offset, value) in [(0, 9), (2 << 16, 37), (N - 4096, 85)] {
            let at = base.add(offset);
            assert_eq!(VirtualAlloc(at.cast(), 4096, MEM_COMMIT, PAGE_READWRITE), at.cast());
            at.write_bytes(value, 4096);
            expected[offset..offset + 4096].fill(value);
        }
        for remap in [false, true] {
            if remap {
                let mut old = 0;
                assert_ne!(VirtualProtect(base.cast(), 4096, PAGE_NOACCESS, &mut old), 0);
                expected[..4096].fill(0);
                let at = base.add(1 << 16);
                assert_eq!(VirtualAlloc(at.cast(), 4096, MEM_COMMIT, PAGE_READWRITE), at.cast());
                at.write_bytes(0xc7, 4096);
                expected[1 << 16..(1 << 16) + 4096].fill(0xc7);
            }
            let mut out = vec![0xee; N];
            super::copy_mapped(out.as_mut_ptr().add(27), base.add(27), N - 54, false);
            assert_eq!(out[27..N - 27], expected[27..N - 27]);
            assert!(out[..27].iter().chain(&out[N - 27..]).all(|&v| v == 0xee));
        }
        assert_ne!(VirtualFree(base.cast(), 0, MEM_RELEASE), 0);
    }
}

use super::*;

#[test]
fn warp_adapter_accepts_only_software() {
    // This probes DXGI without creating a D3D12 device or running a title.
    // SAFETY: plain DXGI adapter queries.
    let raw = unsafe {
        let Ok(factory) = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0))
        else {
            return;
        };
        factory.EnumWarpAdapter::<IDXGIAdapter>()
    };
    let Ok(raw) = raw else { return };
    // SAFETY: EnumWarpAdapter returns a DXGI adapter, which implements
    // IDXGIAdapter1; `raw` owns that interface.
    let desc = unsafe { raw.cast::<IDXGIAdapter1>().unwrap().GetDesc1().unwrap() };
    eprintln!(
        "EnumWarpAdapter: vendor={:#x}, flags={:#x}",
        desc.VendorId, desc.Flags
    );
    assert_eq!(
        warp_adapter().is_ok(),
        desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
    );
}

/// [`Batch::fill`] ([`wide_fills`]): every fill put off lands where its
/// step said, one after the other below `FEW` and in pieces above it.
#[test]
fn fills_put_off_are_all_done() {
    for count in [3usize, 500] {
        let source: Vec<u8> = (0..count * 40).map(|i| (i * 7 + 3) as u8).collect();
        let mut out = vec![0u8; count * 48];
        let mut batch = Batch::default();
        for i in 0..count {
            // 40 bytes of the source into a slot of 48, the rest left as is.
            let dst = out.as_mut_ptr() as usize + i * 48;
            let src = source.as_ptr() as usize + i * 40;
            batch.fills.push((dst, src, 40));
        }
        batch.fill();
        assert!(batch.fills.is_empty());
        for i in 0..count {
            let (got, want) = (&out[i * 48..i * 48 + 40], &source[i * 40..i * 40 + 40]);
            assert_eq!(got, want, "fill {i} of {count}");
            assert!(out[i * 48 + 40..i * 48 + 48].iter().all(|&b| b == 0));
        }
        // Nothing noted: nothing done.
        batch.fill();
    }
}

/// [`Batch::put_off_fill`] ([`wide_fills`]): of two fills put off in turn
/// whose destinations overlap, the second's bytes are there after
/// [`Batch::fill`], as in steps, also with enough fills before them to be
/// done in pieces; the first and those before it were done when the second
/// was noted.
#[test]
fn overlapping_fills_put_off_land_in_order() {
    for before in [0usize, 300] {
        let first: Vec<u8> = vec![0x11; 64];
        let second: Vec<u8> = (0..64).map(|i| i as u8 ^ 0xa5).collect();
        let others: Vec<u8> = vec![0x77; 32];
        let mut out = vec![0u8; 128 + before * 32];
        let base = out.as_mut_ptr() as usize;
        let mut batch = Batch::default();
        for i in 0..before {
            batch.put_off_fill(base + 128 + i * 32, others.as_ptr() as usize, 32);
        }
        // Bytes 16..80, then 32..96: 32..80 written by both.
        batch.put_off_fill(base + 16, first.as_ptr() as usize, 64);
        assert_eq!(batch.fills.len(), before + 1);
        batch.put_off_fill(base + 32, second.as_ptr() as usize, 64);
        assert_eq!(batch.fills.len(), 1, "the ones before were done");
        assert_eq!(&out[16..32], &first[..16]);
        batch.fill();
        assert!(batch.fills.is_empty() && batch.fill_spans.is_empty());
        assert_eq!(&out[32..96], &second[..]);
        assert!(out[..16].iter().chain(&out[96..128]).all(|&b| b == 0));
        assert!(out[128..].iter().all(|&b| b == 0x77));
        // Side by side, not overlapping: nothing done early.
        batch.put_off_fill(base, first.as_ptr() as usize, 16);
        batch.put_off_fill(base + 16, first.as_ptr() as usize, 16);
        assert_eq!(batch.fills.len(), 2);
        batch.fill();
    }
}

/// [`written_spans`] ([`ahead_of_writes`]): only copies given to shaders,
/// over their refreshes before that, and nothing after a title write
/// (their test, dc2a3ad4).
#[test]
fn what_is_written_is_refreshed_ahead_up_to_its_write_and_to_title_writes() {
    use Touch::{Nothing, Refresh, Title, Write};
    let mut found = written_spans(
        [
            Refresh(1, 10, 20),
            Refresh(2, 0, 5),
            Nothing,
            Refresh(1, 30, 40),
            Write(1),
            Refresh(1, 0, 100),
            Write(3),
            Refresh(3, 1, 2),
            Refresh(4, 7, 9),
            Write(4),
            Title,
            Write(2),
        ]
        .into_iter(),
    );
    found.sort_unstable();
    assert_eq!(found, [(1, 10, 40), (4, 7, 9)]);
}
