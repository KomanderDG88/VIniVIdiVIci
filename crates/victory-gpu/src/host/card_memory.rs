//! The host's video memory queries and its upload, shared and
//! unordered-access buffers (moved out of host.rs as is).

use super::*;

impl Host {
    /// The card's memory as the system accounts it to this process, in
    /// bytes: the budget and the use of the card's own memory, then of the
    /// PC's memory shared with the card (where the system moves what does
    /// not fit on the card).
    pub(crate) fn video_memory(&self) -> Option<[u64; 4]> {
        let adapter = self.adapter.as_ref()?;
        let mut own = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        let mut shared = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: plain queries that fill our structures.
        unsafe {
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut own)
                .ok()?;
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut shared)
                .ok()?;
        }
        // The card's budget moved by 256 MB or more since the last look:
        // said at once, with what was in use at the last look, so the log
        // has the moment. (It fell from 11,242 MB to some 10,300 MB with
        // the first world frame in runs 179 to 185, and the memory line,
        // seconds later, was the first to show it. We reserve nothing
        // and make the swap chain once, at the start.)
        static LAST: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];
        let now = [own.Budget, own.CurrentUsage, shared.CurrentUsage];
        let last = [0, 1, 2].map(|i| LAST[i].swap(now[i], Ordering::Relaxed));
        if last[0] != 0 && last[0].abs_diff(own.Budget) >= (256u64 << 20) {
            tracing::info!(
                "the system changed the budget of the card's memory from {} MB to {} MB (frame {}); at the last look {} MB of the card's and {} MB of the PC's shared memory were in use",
                last[0] >> 20,
                own.Budget >> 20,
                crate::frame::current(),
                last[1] >> 20,
                last[2] >> 20
            );
            self.log_video_memory("right after that change");
        }
        // For the line between the memory lines ([`video_memory_line`]).
        VIDEO_PEAK[0].fetch_max(own.CurrentUsage, Ordering::Relaxed);
        VIDEO_PEAK[1].fetch_min(own.Budget, Ordering::Relaxed);
        VIDEO_PEAK[2].fetch_max(shared.CurrentUsage, Ordering::Relaxed);
        VIDEO_PEAK[3].fetch_add(1, Ordering::Relaxed);
        // And for the seconds a removal's report says ([`SECOND_ROWS`]).
        second_look(own.Budget, own.CurrentUsage, shared.CurrentUsage);
        Some([own.Budget, own.CurrentUsage, shared.Budget, shared.CurrentUsage])
    }

    /// One look at the card's memory of our own, at most every tenth of a
    /// second, before a submission is prepared: for [`VIDEO_PEAK`], and so
    /// that a change of the budget is said within a tenth of a second
    /// ([`Self::video_memory`]). Two plain questions to the system.
    pub(super) fn video_memory_look(&self) {
        if !video_look_on() {
            return;
        }
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = now_ms();
        let last = LAST.load(Ordering::Relaxed);
        if now < last + 100 || LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
            return;
        }
        let _ = self.video_memory();
    }

    /// Both of the system's memory groups for this process, in bytes: for
    /// the card's own memory and then for the PC's memory shared with the
    /// card, the budget, the use, what could still be reserved and what
    /// is reserved (the layer reserves none).
    pub(crate) fn video_memory_groups(&self) -> Option<[[u64; 4]; 2]> {
        let adapter = self.adapter.as_ref()?;
        let mut own = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        let mut shared = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: plain queries that fill our structures.
        unsafe {
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut own)
                .ok()?;
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut shared)
                .ok()?;
        }
        let all = |m: &DXGI_QUERY_VIDEO_MEMORY_INFO| {
            [m.Budget, m.CurrentUsage, m.AvailableForReservation, m.CurrentReservation]
        };
        Some([all(&own), all(&shared)])
    }

    /// Logs [`Self::video_memory_groups`], with `when` it is said.
    pub(crate) fn log_video_memory(&self, when: &str) {
        let Some([own, shared]) = self.video_memory_groups() else {
            return;
        };
        tracing::info!(
            "video memory as the system counts it, {}: the card's own: budget {} MB, in use {} MB, {} MB could still be reserved, {} MB reserved; the PC's shared with the card: budget {} MB, in use {} MB, {} MB could still be reserved, {} MB reserved (the layer reserves none)",
            when,
            own[0] >> 20,
            own[1] >> 20,
            own[2] >> 20,
            own[3] >> 20,
            shared[0] >> 20,
            shared[1] >> 20,
            shared[2] >> 20,
            shared[3] >> 20
        );
    }

    /// Whether the device has GPU upload heaps ([`GPU_UPLOAD`]).
    pub(crate) fn gpu_upload(&self) -> bool {
        GPU_UPLOAD.load(Ordering::Relaxed)
    }

    /// A buffer in the card's own memory that the CPU maps and writes (a
    /// GPU upload heap; write-combined: reading it back is slow, and
    /// nothing of ours does) and shaders read: for host copies the CPU
    /// fills and no shader writes. `None` where the device has no such
    /// heaps or turns the buffer down (then the caller's usual buffer).
    pub(crate) fn gpu_upload_buffer(&self, size: u64) -> Option<(ID3D12Resource, *mut u8)> {
        // A failed mapping probe applies to every later window on this device.
        static WORKS: OnceLock<bool> = OnceLock::new();
        if !self.gpu_upload() || WORKS.get() == Some(&false) {
            return None;
        }
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_GPU_UPLOAD,
            ..Default::default()
        };
        // (Unordered access allowed, as the buffers it stands in for: a
        // view of that kind over a window must still be makeable.)
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: size.max(1),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut buffer: Option<ID3D12Resource> = None;
        let mut data = std::ptr::null_mut();
        // SAFETY: plain creation and mapping of a CPU-visible buffer.
        let made = unsafe {
            self.device
                .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buffer)
                .and_then(|()| {
                    let b = buffer.as_ref().ok_or_else(windows::core::Error::empty)?;
                    b.Map(0, None, Some(&mut data))
                })
        };
        match (made, buffer) {
            (Ok(()), Some(buffer)) if !data.is_null() => {
                // The first one is tried before any is used: title memory
                // reaches a host copy by a plain copy and, where a page is
                // not there, through the kernel (`read`), and this memory
                // is the card's, mapped through the bus. Both ways must
                // write it and it must read back the same, or no copy goes
                // to such a heap in this run.
                let works = *WORKS.get_or_init(|| {
                    let n = (size as usize).min(8192);
                    let pattern: Vec<u8> = (0..n).map(|i| (i * 31 + 7) as u8).collect();
                    let half = n / 2;
                    // SAFETY: the buffer's own mapping, `size` bytes of it; a
                    // source of ours.
                    let same = unsafe {
                        std::ptr::copy_nonoverlapping(pattern.as_ptr(), data.cast::<u8>(), half);
                        let by_kernel = read(pattern[half..].as_ptr(), data.cast::<u8>().add(half), n - half);
                        by_kernel && std::slice::from_raw_parts(data.cast::<u8>(), n) == pattern.as_slice()
                    };
                    // SAFETY: as above (left as a fresh buffer is: zeroed).
                    unsafe { std::ptr::write_bytes(data.cast::<u8>(), 0, n) };
                    if same {
                        tracing::info!("a buffer in a GPU upload heap takes a plain copy and the kernel's copy and reads back the same: such heaps are used");
                    } else {
                        tracing::warn!("a buffer in a GPU upload heap did not read back what was written into it: such heaps are not used in this run");
                    }
                    same
                });
                works.then_some((buffer, data.cast()))
            }
            (made, _) => {
                // Said once: every later one would say the same.
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| {
                    tracing::warn!(size, result = ?made, "a buffer in a GPU upload heap could not be made or mapped: such copies stay in the PC's memory")
                });
                None
            }
        }
    }

    /// A buffer in system memory that the CPU maps (write-back, so reading
    /// it is fast too) and shaders read and write: for host copies of title
    /// memory that shaders write, which upload memory cannot be. Starts in
    /// the common state (buffers move out of it implicitly).
    pub(crate) fn shared_buffer(&self, size: u64) -> Option<(ID3D12Resource, *mut u8)> {
        crate::census::made(2, size);
        MADE[2].fetch_add(1, Ordering::Relaxed);
        MADE[3].fetch_add(size, Ordering::Relaxed);
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_CUSTOM,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
            MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: size.max(1),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        // Once more after the idle pools are emptied ([`oom_reclaim`]).
        for attempt in 0..2 {
            let mut buffer: Option<ID3D12Resource> = None;
            let mut data = std::ptr::null_mut();
            // SAFETY: plain creation and mapping of a CPU-visible buffer.
            let made = unsafe {
                self.device
                    .CreateCommittedResource(
                        &heap,
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        D3D12_RESOURCE_STATE_COMMON,
                        None,
                        &mut buffer,
                    )
                    .and_then(|()| {
                        let b = buffer.as_ref().ok_or_else(windows::core::Error::empty)?;
                        b.Map(0, None, Some(&mut data))
                    })
            };
            match made {
                Ok(()) => return Some((buffer?, data.cast())),
                Err(e) if retry_after_oom(oom_reclaim(), attempt, e.code().0) => reclaim_for_oom("shared buffer"),
                Err(e) => {
                    tracing::warn!(error = %e, size, "no shared buffer");
                    return None;
                }
            }
        }
        None
    }

    /// A device-local buffer shaders may write (unordered access) and indirect
    /// execution may read, starting in the common state.
    pub(crate) fn uav_buffer(&self, size: u64) -> Option<ID3D12Resource> {
        crate::census::made(3, size);
        MADE[4].fetch_add(1, Ordering::Relaxed);
        MADE[5].fetch_add(size, Ordering::Relaxed);
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: size.max(1),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        // Once more after the idle pools are emptied ([`oom_reclaim`]).
        for attempt in 0..2 {
            let mut buffer: Option<ID3D12Resource> = None;
            // SAFETY: plain creation of a device-local buffer.
            let made = unsafe {
                self.device.CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut buffer,
                )
            };
            match made {
                Ok(()) => return buffer,
                Err(e) if retry_after_oom(oom_reclaim(), attempt, e.code().0) => reclaim_for_oom("unordered-access buffer"),
                Err(e) => {
                    tracing::warn!(error = %e, size, "no unordered-access buffer");
                    return None;
                }
            }
        }
        None
    }
}
