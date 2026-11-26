//! The methods of [`HostList`], kept apart from `host.rs` so that file
//! stays under the size limit the repository sets for source files.

use super::*;

impl HostList {
    /// The timestamp heap and its read-back buffer (diagnostics, timing on).
    #[cfg(feature = "diagnostics")]
    pub(super) fn stamps(&self) -> Option<&(ID3D12QueryHeap, ID3D12Resource)> {
        if !timing() {
            return None;
        }
        self.stamps
            .get_or_init(|| {
                let device = &host()?.device;
                let mut heap: Option<ID3D12QueryHeap> = None;
                let mut back: Option<ID3D12Resource> = None;
                let readback = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: u64::from(STAMPS) * 8,
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    ..Default::default()
                };
                // SAFETY: plain object creation.
                unsafe {
                    device
                        .CreateQueryHeap(
                            &D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: STAMPS, NodeMask: 0 },
                            &mut heap,
                        )
                        .ok()?;
                    device
                        .CreateCommittedResource(
                            &readback,
                            D3D12_HEAP_FLAG_NONE,
                            &desc,
                            D3D12_RESOURCE_STATE_COPY_DEST,
                            None,
                            &mut back,
                        )
                        .ok()?;
                }
                Some((heap?, back?))
            })
            .as_ref()
    }

    /// The noted commands that took longest on the GPU, longest first, as
    /// (milliseconds, text) (diagnostics, timing on, after the list ran).
    #[cfg(feature = "diagnostics")]
    pub(super) fn slowest(&self, frequency: u64, n: usize) -> Vec<(f64, String)> {
        let notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        let (Some((_, back)), count) = (self.stamps(), notes.len().min(STAMPS as usize - 1)) else {
            return Vec::new();
        };
        let mut data = std::ptr::null_mut();
        let range = D3D12_RANGE { Begin: 0, End: (count + 1) * 8 };
        // SAFETY: a read-back buffer the finished list resolved into.
        let stamps: Vec<u64> = unsafe {
            if back.Map(0, Some(&range), Some(&mut data)).is_err() {
                return Vec::new();
            }
            let s = std::slice::from_raw_parts(data.cast::<u64>(), count + 1).to_vec();
            back.Unmap(0, None);
            s
        };
        let mut times: Vec<(f64, String)> = (0..count)
            .map(|i| {
                let ticks = stamps[i + 1].saturating_sub(stamps[i]);
                (ticks as f64 * 1e3 / frequency.max(1) as f64, notes[i].0.chars().take(300).collect())
            })
            .collect();
        times.sort_by(|a, b| b.0.total_cmp(&a.0));
        times.truncate(n);
        times
    }

    pub(crate) fn close(&self) {
        // The copies below write buffers on the card: no draw after them joins a window.
        crate::indirect::end_window(self);
        // Host queries begun in this list and never ended here: ended now, answered "visible".
        let open = std::mem::take(&mut *self.open_queries.lock().unwrap_or_else(|e| e.into_inner()));
        if !open.is_empty() {
            crate::query::close_open(open, self);
        }
        self.twins_to_depth();
        self.textures_back();
        let scratch = self.indirect_args.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((scratch, _)) = scratch {
            self.indirect_common(&scratch);
        }
        self.card_budget_out();
        // The scratch buffers its prelude fills and leaves readable as
        // arguments: common again behind the list, as every scratch buffer.
        {
            let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
            for set in gather.sets.iter().filter(|s| s.records() > 0) {
                // SAFETY: an open list and a live buffer.
                unsafe {
                    self.list.ResourceBarrier(&[transition(
                        &set.scratch,
                        D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
                        D3D12_RESOURCE_STATE_COMMON,
                    )]);
                }
            }
            // The segments' slots likewise; the last segment ends here.
            gather.segments.open.clear();
            if gather.segments.used
                && let Some(buffer) = &gather.segments.buffer
            {
                // SAFETY: an open list and a live buffer.
                unsafe {
                    self.list.ResourceBarrier(&[transition(
                        buffer,
                        D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
                        D3D12_RESOURCE_STATE_COMMON,
                    )]);
                }
            }
        }
        crate::cardmeter::end(&self.list, crate::cardmeter::Span::from_raw(self.pipeline_meter.swap(u32::MAX, Ordering::Relaxed)));
        crate::cardmeter::end_named(
            &self.list,
            crate::cardmeter::Span::from_raw(self.meter.swap(u32::MAX, Ordering::Relaxed)),
            &self.pipelines.lock().unwrap_or_else(|e| e.into_inner()),
        );
        #[cfg(feature = "diagnostics")]
        crate::shot::close(self);
        #[cfg(feature = "diagnostics")]
        if let Some((heap, back)) = self.stamps() {
            let count = (self.notes.lock().unwrap_or_else(|e| e.into_inner()).len() as u32).min(STAMPS - 1);
            // SAFETY: an open list, our heap and read-back buffer.
            unsafe {
                self.list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, count);
                self.list.ResolveQueryData(heap, D3D12_QUERY_TYPE_TIMESTAMP, 0, count + 1, back, 0);
            }
        }
        // SAFETY: our list.
        if let Err(e) = unsafe { self.list.Close() } {
            fail(format_args!(
                "a host command list did not close ({e}): a command recorded into it was invalid"
            ));
        }
    }

    /// Only after the list's last submission finished (every one is waited
    /// for).
    pub(crate) fn reset(&self) {
        #[cfg(feature = "diagnostics")]
        let started = std::time::Instant::now();
        self._alive.used();
        *self.clamp_window.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.deferred() = Deferred::default();
        // Oversized per-list tables shrunk on reset only with
        // `VICTORY_RAM_TRIM=1`.
        let trim = crate::ram_trim::aggressive();
        {
            let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
            states.clear();
            if trim && states.capacity() > 1024 {
                states.shrink_to(256);
            }
        }
        {
            let mut twins = self.twins.lock().unwrap_or_else(|e| e.into_inner());
            twins.clear();
            if trim && twins.capacity() > 256 {
                twins.shrink_to(64);
            }
        }
        self.pipelines.lock().unwrap_or_else(|e| e.into_inner()).clear();
        {
            let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
            let Gather { closed, written, sets, spare, segments, .. } = &mut *gather;
            *closed = false;
            written.clear();
            segments.used = false;
            segments.passes = 0;
            segments.open.clear();
            if trim && written.capacity() > 256 {
                written.shrink_to(64);
            }
            for set in sets.drain(..) {
                if spare.len() < GATHER_SETS {
                    spare.push(set.into_scratch());
                }
            }
        }
        #[cfg(feature = "diagnostics")]
        {
            let mut notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
            notes.clear();
            if trim && notes.capacity() > 512 {
                notes.shrink_to(128);
            }
        }
        #[cfg(feature = "diagnostics")]
        {
            *self.shot_block.lock().unwrap_or_else(|e| e.into_inner()) = (0, 0, 0);
            *self.shot_mark.lock().unwrap_or_else(|e| e.into_inner()) = (0, 0);
        }
        {
            let mut scratch = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
            (scratch.current, scratch.used) = (0, 0);
            // Every submission of the list has run: nothing reads its
            // scratch memory any more, and the next list to ask takes it.
            if scratch_pool() {
                give_chunks(std::mem::take(&mut scratch.chunks));
            }
        }
        // SAFETY: the list is not executing.
        let reset = unsafe {
            self.allocator
                .Reset()
                .and_then(|()| self.list.Reset(&self.allocator, None))
        };
        if let Err(e) = reset {
            fail(format_args!("a host command list did not reset ({e})"));
        }
        self.meter
            .store(crate::cardmeter::begin(&self.list, crate::cardmeter::Kind::Lists).raw(), Ordering::Relaxed);
        // A frame shot tells the lists begun inside its frame from the ones
        // it found half recorded, and times each from its very start: what
        // the card does in a list before the title's first command is the
        // list's too.
        #[cfg(feature = "diagnostics")]
        if crate::shot::keeping_lines() {
            crate::shot::stamp(self);
            crate::shot::line(std::ptr::from_ref(self) as usize, "LIST BEGINS");
        }
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::ResetNs, started.elapsed().as_nanos() as u64);
    }

    /// Collects spare gather/indirect COM buffers held on an idle list into `out`
    /// so callers can drop D3D12 COM references outside `gather`, `indirect_args`,
    /// and `list::IDLE` mutexes.
    pub(crate) fn take_idle_caches(&self, out: &mut TrimmedListCaches) -> u64 {
        let mut freed_bytes = 0u64;
        {
            let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
            if !gather.spare.is_empty() {
                let each = u64::from(crate::indirect::GATHER_MOST)
                    * u64::from(crate::indirect::GATHER_STRIDE);
                freed_bytes += (gather.spare.len() as u64) * each;
                out.0.append(&mut gather.spare);
                gather.spare.shrink_to_fit();
            }
        }
        {
            let mut slot = self.indirect_args.lock().unwrap_or_else(|e| e.into_inner());
            let cap = slot.as_ref().map_or(0, |(_, cap)| *cap);
            if cap >= (1 << 20)
                && let Some((res, _)) = slot.take()
            {
                freed_bytes += cap;
                out.0.push(res);
            }
        }
        freed_bytes
    }

    /// Moves a title texture's host copy to `wanted` (a host state) in this
    /// list: the host texture and the state it leaves, if that is a change.
    /// The list's first use of a texture records nothing: its prelude brings
    /// the texture there at submission.
    pub(crate) fn transition(
        &self,
        resource: crate::object::This,
        wanted: u32,
    ) -> Option<(&'static ID3D12Resource, u32)> {
        let texture = crate::resource::host_resource(resource)?;
        let copy = crate::resource::host_copy(resource)?;
        let wanted = legal_state(texture, wanted);
        // A frame shot names its pictures after the title's textures.
        #[cfg(feature = "diagnostics")]
        if crate::shot::recording() {
            crate::shot::known(std::sync::Arc::as_ptr(&copy) as usize, resource as usize);
        }
        let before = {
            let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
            match states.entry(std::sync::Arc::as_ptr(&copy) as usize) {
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert((wanted, wanted, copy.clone()));
                    None
                }
                std::collections::hash_map::Entry::Occupied(mut o) => Some(std::mem::replace(&mut o.get_mut().1, wanted)),
            }
        }?;
        if before == wanted {
            return None;
        }
        // A frame shot copies a texture where it leaves a state shaders
        // write in, and leaves it in that state: the caller's move is the
        // same as in any other frame.
        #[cfg(feature = "diagnostics")]
        if crate::shot::recording() {
            crate::shot::leaves(&self.list, std::ptr::from_ref(self) as usize, Some(self), None, &copy, before, wanted);
        }
        Some((texture, before))
    }

    /// Moves host texture `copy` to `wanted` (a host state) in this list,
    /// as [`HostList::transition`] does a title resource's host copy, and
    /// records the barrier: for textures that are not a title resource's
    /// host copy (colour twins) and for the copies between the two.
    pub(super) fn settle(&self, copy: &std::sync::Arc<crate::resource::HostTexture>, wanted: u32) {
        let wanted = legal_state(&copy.texture, wanted);
        let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
        let before = match states.entry(std::sync::Arc::as_ptr(copy) as usize) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert((wanted, wanted, copy.clone()));
                None
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                let before = std::mem::replace(&mut o.get_mut().1, wanted);
                (before != wanted).then_some(before)
            }
        };
        drop(states);
        if let Some(before) = before {
            let barrier = transition(
                &copy.texture,
                D3D12_RESOURCE_STATES(before as i32),
                D3D12_RESOURCE_STATES(wanted as i32),
            );
            // SAFETY: an open list and a live texture.
            unsafe { self.list.ResourceBarrier(&[barrier]) };
        }
    }

    /// A use of title texture `resource` about to be recorded, if it is a
    /// depth texture with a colour twin (false if it is not): of the twin
    /// (`colour`: unordered access) or of the depth texture (depth target,
    /// clear, shader read, copy, resolve), writing it or only reading. The
    /// side used is first brought up to date: by a copy recorded here if
    /// this list wrote the other side since, by the list's prelude if the
    /// list uses the side as it finds it. The twin is left in the
    /// unordered access state; the caller moves the depth texture to the
    /// state its use needs, as for any texture. A twin still ahead when the
    /// list closes is copied then (`twins_to_depth`).
    /// shortcut: every depth target bound for a draw counts as written
    /// (also when the draw only tests), and every unordered access view a
    /// shader names as written; views a shader picks from the heap by a
    /// computed index are not seen here at all.
    pub(crate) fn twin_use(
        &self,
        resource: crate::object::This,
        colour: bool,
        write: bool,
    ) -> bool {
        const UNORDERED_ACCESS: u32 = 0x8;
        // Asked for every texture a draw reads: most have no twin.
        let Some(twin) = crate::resource::twin(resource) else {
            return false;
        };
        let Some(depth) = crate::resource::host_copy(resource) else {
            return false;
        };
        let side = usize::from(colour);
        let mut twins = self.twins.lock().unwrap_or_else(|e| e.into_inner());
        let u = twins
            .entry(std::sync::Arc::as_ptr(&twin) as usize)
            .or_insert_with(|| TwinUse {
                depth: depth.clone(),
                twin: twin.clone(),
                needs: [false; 2],
                behind: [None; 2],
                writing: false,
                late: false,
            });
        let copy = match u.behind[side] {
            Some(behind) => behind,
            None => {
                u.needs[side] = true;
                false
            }
        };
        u.behind[side] = Some(false);
        match (write, colour) {
            // Shaders write the twin when the draw or dispatch runs: what
            // the same one reads of the depth texture is prepared before
            // (`twins_written`).
            (true, true) => u.writing = true,
            (true, false) => u.behind[1] = Some(true),
            (false, _) => {}
        }
        // shortcut: the depth plane's twin is not drawn into the plane
        // here: the pass sets a pipeline and a target of ours, which a list
        // still recording the title's commands cannot take. This use sees
        // the plane as it was; the pass runs when the list closes
        // (`twins_to_depth`). Counted.
        if copy && !colour && twin.plane.is_some() {
            u.late = true;
            crate::resource::PLANES[2].fetch_add(1, Ordering::Relaxed);
        }
        drop(twins);
        if copy {
            self.twin_copy(&depth, &twin, colour);
        } else if colour {
            self.settle(&twin.colour, UNORDERED_ACCESS);
        }
        // The depth plane's twin is written through unordered access too.
        if colour && let Some(plane) = &twin.plane {
            self.settle(plane, UNORDERED_ACCESS);
        }
        true
    }

    /// Records the pass that draws the twin of the depth plane of `depth`
    /// into the plane, if `twin` has one, with the moves to the states the
    /// pass needs; the twin is left in the unordered access state. Only
    /// for a list that records nothing of the title's afterwards
    /// (`record_plane_pass`).
    pub(super) fn pass_plane(
        &self,
        depth: &std::sync::Arc<crate::resource::HostTexture>,
        twin: &crate::resource::Twin,
    ) {
        const UNORDERED_ACCESS: u32 = 0x8;
        const DEPTH_WRITE: u32 = 0x10;
        const PIXEL_SHADER_RESOURCE: u32 = 0x80;
        let Some(plane) = &twin.plane else { return };
        self.settle(depth, DEPTH_WRITE);
        self.settle(plane, PIXEL_SHADER_RESOURCE);
        record_plane_pass(self, &depth.texture, &plane.texture);
        self.settle(plane, UNORDERED_ACCESS);
    }

    /// Records the copy of a depth texture into its colour twin (`colour`)
    /// or of the twin into the depth texture, with the moves to the copy
    /// states. The twin is left in the unordered access state.
    pub(super) fn twin_copy(
        &self,
        depth: &std::sync::Arc<crate::resource::HostTexture>,
        twin: &crate::resource::Twin,
        colour: bool,
    ) {
        const UNORDERED_ACCESS: u32 = 0x8;
        const COPY_DEST: u32 = 0x400;
        const COPY_SOURCE: u32 = 0x800;
        let (from, to) = if colour {
            (depth, &twin.colour)
        } else {
            (&twin.colour, depth)
        };
        self.settle(from, COPY_SOURCE);
        self.settle(to, COPY_DEST);
        record_twin_copy(&self.list, &depth.texture, twin, colour);
        self.settle(&twin.colour, UNORDERED_ACCESS);
    }

    /// Before the list closes: the depth textures whose twins it wrote last
    /// are given what was written, and left readable by shaders. Shaders
    /// that pick a view from the heap by a computed index are not seen by
    /// `twin_use` and read the depth texture in lists that never name it
    /// (in a captured frame of the title no table names a shadow
    /// map for reading but the pass that fills the next one).
    pub(super) fn twins_to_depth(&self) {
        const SHADER_RESOURCE: u32 = 0x40 | 0x80;
        let mut twins = self.twins.lock().unwrap_or_else(|e| e.into_inner());
        let written: Vec<_> = twins
            .values_mut()
            .filter(|u| u.behind[0] == Some(true) || u.late)
            .map(|u| {
                // Not copied again if the list copied since the last write
                // and only the depth plane's pass is left.
                let copy = u.behind[0] == Some(true);
                u.behind[0] = Some(false);
                u.late = false;
                (u.depth.clone(), u.twin.clone(), copy)
            })
            .collect();
        drop(twins);
        for (depth, twin, copy) in &written {
            if *copy {
                self.twin_copy(depth, twin, false);
            }
            // Nothing of the title's is recorded after this: the pass may
            // leave its own pipeline and target set.
            self.pass_plane(depth, twin);
            self.settle(depth, SHADER_RESOURCE);
        }
    }

    /// A draw or dispatch being prepared writes title texture `resource`
    /// through unordered access: if it is of a kind copied back
    /// ([`crate::texback`]), the list copies it when it closes.
    pub(crate) fn texture_written(&self, resource: crate::object::This) {
        if !crate::texback::wanted(resource) {
            return;
        }
        let mut deferred = self.deferred();
        if !deferred.back_wanted.iter().any(|target| target.owner() == resource as usize)
            && let Some(target) = crate::texback::Target::new(resource)
        {
            deferred.back_wanted.push(target);
        }
    }

    /// Before the list closes: the textures its shaders wrote that are copied
    /// back go into read-back buffers ([`crate::texback::record`]).
    pub(super) fn textures_back(&self) {
        const COPY_SOURCE: u32 = 0x800;
        let wanted = std::mem::take(&mut self.deferred().back_wanted);
        for target in wanted {
            self.settle(&target.copy, COPY_SOURCE);
            // With device-removed data in its full form: the copies that
            // follow are named in the breadcrumbs (a stop in them read "op
            // 8, no marker"). Not otherwise: a note is also a line of a
            // listed frame and a stamp of a frame shot.
            #[cfg(feature = "diagnostics")]
            {
                if dred_on() {
                    self.note_op(|| format!("texture read-back of title texture {:#x} (texback)", target.owner()));
                }
            }
            if let Some(back) = crate::texback::record(&self.list, target) {
                self.deferred().texture_backs.push(back);
            }
        }
    }

    /// The draw or dispatch just prepared is recorded: the twins it writes
    /// now hold what their depth textures lack.
    pub(crate) fn twins_written(&self) {
        let mut twins = self.twins.lock().unwrap_or_else(|e| e.into_inner());
        for u in twins.values_mut().filter(|u| u.writing) {
            u.writing = false;
            u.behind[0] = Some(true);
        }
    }

    /// At submission: the list that moves each texture this list uses from
    /// its real state to the one the list starts with; the textures' real
    /// states become those the list ends with. For depth textures with a
    /// colour twin it first copies the side the list uses as it finds it
    /// from the other, if a list submitted earlier left it behind.
    /// With `join` the list is the first of a title submission merged
    /// behind others: the prelude is made in any case, and first waits for
    /// everything before it (`merging::join_barriers`).
    pub(super) fn prelude(&self, join: bool) -> Option<ID3D12CommandList> {
        const COPY_DEST: u32 = 0x400;
        const COPY_SOURCE: u32 = 0x800;
        let host_state = |state: u32| D3D12_RESOURCE_STATES(state as i32);
        let states = self.states.lock().unwrap_or_else(|e| e.into_inner());
        // The copies (depth texture, twin, whether into the twin); the
        // sides are then as this list leaves them.
        let mut syncs = Vec::new();
        let twins = self.twins.lock().unwrap_or_else(|e| e.into_inner());
        for u in twins.values() {
            let real = u.twin.behind.load(Ordering::Relaxed);
            let mut behind = [real & 1 != 0, real & 2 != 0];
            for side in [0, 1] {
                if u.needs[side] && behind[side] {
                    syncs.push((u.depth.clone(), u.twin.clone(), side == 1));
                    behind = [false; 2];
                }
            }
            let left = [0, 1].map(|side| u.behind[side].unwrap_or(behind[side]));
            let left = u32::from(left[0]) | (u32::from(left[1]) << 1);
            u.twin.behind.store(left, Ordering::Relaxed);
        }
        drop(twins);
        // A copy runs with its textures in the copy states: each moves
        // there from its real state, and from there to the state the list
        // starts with (back to its real state if the list does not use it).
        let mut to_copy = Vec::new();
        let mut barriers = Vec::new();
        let mut copying = std::collections::HashMap::new();
        for (depth, twin, colour) in &syncs {
            let (from, to) = if *colour {
                (depth, &twin.colour)
            } else {
                (&twin.colour, depth)
            };
            for (texture, state) in [(from, COPY_SOURCE), (to, COPY_DEST)] {
                let key = std::sync::Arc::as_ptr(texture) as usize;
                let real = texture.state.load(Ordering::Relaxed);
                copying.insert(key, state);
                if real != state {
                    to_copy.push(transition(
                        &texture.texture,
                        host_state(real),
                        host_state(state),
                    ));
                    if !states.contains_key(&key) {
                        barriers.push(transition(
                            &texture.texture,
                            host_state(state),
                            host_state(real),
                        ));
                    }
                }
            }
        }
        // A frame shot copies the textures that leave a state shaders write
        // in between lists, each before the moves below and left in the
        // state it was found in.
        #[cfg(feature = "diagnostics")]
        let shot_begins = crate::shot::of_the_frame(self);
        #[cfg(feature = "diagnostics")]
        let mut leaving: Vec<(std::sync::Arc<crate::resource::HostTexture>, u32, u32)> = Vec::new();
        barriers.extend(states.iter().filter_map(|(key, (first, last, copy))| {
            let real = copy.state.swap(*last, Ordering::Relaxed);
            let real = copying.get(key).copied().unwrap_or(real);
            #[cfg(feature = "diagnostics")]
            if shot_begins.is_some() && !copying.contains_key(key) && crate::shot::might_leave(real, *first) {
                leaving.push((copy.clone(), real, *first));
            }
            (real != *first)
                .then(|| transition(&copy.texture, host_state(real), host_state(*first)))
        }));
        // (The same order in every frame, whatever order the map gives.)
        #[cfg(feature = "diagnostics")]
        leaving.sort_by_key(|l| std::sync::Arc::as_ptr(&l.0) as usize);
        // The records the list's draws read from slots (`crate::indirect::gather`).
        let gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
        let gathered = gather.sets.iter().any(|s| s.records() > 0);
        if !join && barriers.is_empty() && syncs.is_empty() && !gathered {
            return None;
        }
        let Some((allocator, list)) = self
            .prelude
            .get_or_init(|| host().and_then(|h| h.make_prelude()))
            .as_ref()
        else {
            fail(format_args!("no host list for a state prelude"));
        };
        // SAFETY: the prelude is not executing (every submission is waited
        // for); the textures are live.
        let recorded = unsafe {
            allocator.Reset().and_then(|()| {
                list.Reset(allocator, None)?;
                if join {
                    list.ResourceBarrier(&merging::join_barriers());
                }
                #[cfg(feature = "diagnostics")]
                let shot_span = if shot_begins.is_some() { crate::shot::span_begin(list) } else { u32::MAX };
                if !to_copy.is_empty() {
                    list.ResourceBarrier(&to_copy);
                }
                for (depth, twin, colour) in &syncs {
                    record_twin_copy(list, &depth.texture, twin, *colour);
                }
                #[cfg(feature = "diagnostics")]
                for (copy, real, first) in &leaving {
                    crate::shot::leaves(list, std::ptr::from_ref(self) as usize, None, shot_begins, copy, *real, *first);
                }
                if !barriers.is_empty() {
                    list.ResourceBarrier(&barriers);
                }
                if gathered {
                    // The slots must hold clamped records before a draw
                    // reads them: a prelude that cannot record the passes
                    // is no prelude.
                    let filled = match &gather.budget {
                        Some(budget) => crate::indirect::record_gathers(list, &gather.sets, budget),
                        None => false,
                    };
                    if !filled {
                        fail(format_args!("a list's gathered indirect records could not be clamped in its prelude"));
                    }
                }
                #[cfg(feature = "diagnostics")]
                crate::shot::span_end(list, shot_span, std::ptr::from_ref(self) as usize, || {
                    format!(
                        "PRELUDE of the recording that begins at line {}: {} moves of textures, {} copies between a depth texture and its twin, {} sets of gathered draw records clamped",
                        shot_begins.unwrap_or(0),
                        barriers.len() + to_copy.len(),
                        syncs.len(),
                        gather.sets.iter().filter(|s| s.records() > 0).count()
                    )
                });
                list.Close()
            })
        };
        if let Err(e) = recorded {
            fail(format_args!("a host state prelude did not record ({e})"));
        }
        list.cast().ok()
    }

    /// From here on the list gathers no records (`crate::indirect::gather`):
    /// what is about to be recorded may write arguments.
    pub(crate) fn gather_close(&self) {
        let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
        gather.closed = true;
        gather.segments.open.clear();
    }

    /// What is about to be recorded may write arguments: the segment open now ends
    /// (`crate::indirect::gather_segments`), whatever the prelude's gathering does.
    pub(crate) fn segments_end(&self) {
        self.gather.lock().unwrap_or_else(|e| e.into_inner()).segments.open.clear();
    }

    /// Shaders of the list were declared to write the host copy numbered
    /// `copy` (`Shadow::gpu_writes`): with
    /// `crate::indirect::gather_by_copy`, no record of that copy is
    /// gathered from here on. (Without the switch nothing is noted: the
    /// list stops gathering whole, as before.)
    pub(crate) fn gather_wrote(&self, copy: u64) {
        let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
        // Shaders about to write: the open segment ends (`crate::indirect::gather_segments`).
        gather.segments.open.clear();
        if crate::indirect::gather_by_copy_kept() {
            gather.written.insert(copy);
        }
    }

    /// A slot for the record at `offset` of the host copy `src` in the
    /// scratch buffer the list's prelude fills (`crate::indirect::gather`,
    /// which checked the record): the buffer and the slot's place in it.
    ///
    /// Once the list stopped gathering (whole, or for this copy): with `crate::indirect::gather_segments`, a slot of a
    /// pass of the open segment ([`HostList::segment_slot`]; the third value: a pass was recorded for it).
    pub(crate) fn gather_slot(&self, src: &ID3D12Resource, src_id: u64, offset: u32, (kind, stride): (u32, u32)) -> Option<(ID3D12Resource, u64, bool)> {
        let none = |why: usize| {
            crate::indirect::GATHER_NONE[why].fetch_add(1, Ordering::Relaxed);
            None
        };
        let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
        if gather.closed || gather.written.contains(&src_id) {
            drop(gather);
            if !crate::indirect::gather_segments() {
                return none(1);
            }
            return self.segment_slot(src, src_id, offset, (kind, stride));
        }
        let found = gather
            .sets
            .iter()
            .position(|s| s.src_id == src_id && s.kind == kind && s.stride == stride && s.src.as_raw() == src.as_raw());
        let at = match found {
            Some(at) => at,
            None => {
                if gather.sets.len() >= GATHER_SETS {
                    return none(2);
                }
                // The prelude zeroes the budget as the list zeroes its own.
                if self.list.cast::<ID3D12GraphicsCommandList2>().is_err() {
                    return none(4);
                }
                let Some(host) = host() else {
                    return none(4);
                };
                if gather.budget.is_none() {
                    let Some(budget) = host.uav_buffer(256) else {
                        return none(4);
                    };
                    name(&budget, || "indirect budget of gathered records".to_owned());
                    gather.budget = Some(budget);
                }
                let scratch = match gather.spare.pop() {
                    Some(scratch) => scratch,
                    None => {
                        let Some(scratch) = host.uav_buffer(u64::from(crate::indirect::GATHER_MOST) * u64::from(crate::indirect::GATHER_STRIDE)) else {
                            return none(4);
                        };
                        name(&scratch, || "indirect arguments, gathered".to_owned());
                        scratch
                    }
                };
                let Some(table) = self.scratch(u64::from(crate::indirect::GATHER_MOST) * 4) else {
                    gather.spare.push(scratch);
                    return none(4);
                };
                gather.sets.push(crate::indirect::GatherSet::new(src.clone(), src_id, (kind, stride), scratch, table));
                gather.sets.len() - 1
            }
        };
        let set = &mut gather.sets[at];
        let Some(slot) = set.slot(offset) else {
            return none(3);
        };
        Some((set.scratch.clone(), u64::from(slot) * u64::from(stride), false))
    }

    /// [`HostList::gather_slot`] once the list stopped gathering for its prelude (`crate::indirect::gather_segments`):
    /// a slot of a pass of the segment open now, or of a pass recorded here for it. None: clamped alone, as before.
    fn segment_slot(&self, src: &ID3D12Resource, src_id: u64, offset: u32, (kind, stride): (u32, u32)) -> Option<(ID3D12Resource, u64, bool)> {
        use crate::indirect::{GATHER_STRIDE, SEGMENT_PASS, SEGMENT_PASSES, SEGMENT_STATS};
        let fell_back = || {
            SEGMENT_STATS[2].fetch_add(1, Ordering::Relaxed);
            crate::indirect::GATHER_NONE[1].fetch_add(1, Ordering::Relaxed);
            None
        };
        let pass_bytes = u64::from(SEGMENT_PASS) * u64::from(GATHER_STRIDE);
        let take = |pass: &mut SegmentPass, buffer: &ID3D12Resource| {
            let i = pass.records;
            pass.records += 1;
            // SAFETY: the table holds a count and `SEGMENT_PASS` offsets, in the list's upload memory alive until its
            // reset; the card reads it only when the list runs. The count is written last.
            unsafe {
                let table = pass.table as *mut u32;
                table.add(1 + i as usize).write_volatile(offset);
                table.write_volatile(pass.records);
            }
            SEGMENT_STATS[0].fetch_add(1, Ordering::Relaxed);
            (buffer.clone(), u64::from(pass.pass) * pass_bytes + u64::from(i) * u64::from(stride))
        };
        {
            let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
            let Segments { buffer, open, .. } = &mut gather.segments;
            if let Some(buffer) = buffer.as_ref()
                && let Some(pass) = open
                    .iter_mut()
                    .find(|p| p.src_id == src_id && p.kind == kind && p.stride == stride && p.src.as_raw() == src.as_raw() && p.records < SEGMENT_PASS)
            {
                let (buffer, at) = take(pass, buffer);
                return Some((buffer, at, false));
            }
        }
        // A pass for the record, here: the list's budget first (it may record the budget's zeroing).
        let Some(budget) = self.budget() else {
            return fell_back();
        };
        let Some(table) = self.scratch(4 + u64::from(SEGMENT_PASS) * 4) else {
            return fell_back();
        };
        let mut gather = self.gather.lock().unwrap_or_else(|e| e.into_inner());
        let segments = &mut gather.segments;
        if segments.passes >= SEGMENT_PASSES {
            return fell_back();
        }
        if segments.buffer.is_none() {
            let Some(made) = host().and_then(|h| h.uav_buffer(u64::from(SEGMENT_PASSES) * pass_bytes)) else {
                return fell_back();
            };
            name(&made, || "indirect arguments, gathered in segments".to_owned());
            segments.buffer = Some(made);
        }
        let Some(buffer) = segments.buffer.clone() else {
            return fell_back();
        };
        // SAFETY: the list's own upload memory: the count starts at none.
        unsafe { (table.1 as *mut u32).write_volatile(0) };
        let before = if segments.used { D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT } else { D3D12_RESOURCE_STATE_COMMON };
        let pass = segments.passes;
        if !crate::indirect::record_segment(&self.list, (&buffer, before), u64::from(pass) * pass_bytes, budget, src, table.0, (kind, stride)) {
            return fell_back();
        }
        segments.used = true;
        segments.passes += 1;
        segments.open.push(SegmentPass { src: src.clone(), src_id, kind, stride, pass, records: 0, table: table.1 as usize });
        let passes = SEGMENT_STATS[1].fetch_add(1, Ordering::Relaxed) + 1;
        if passes.is_power_of_two() {
            tracing::info!(
                "segments so far (VICTORY_GATHER_SEGMENTS): {passes} passes took {} records (records per pass is what it saves), {} found no room and were clamped alone",
                SEGMENT_STATS[0].load(Ordering::Relaxed) + 1,
                SEGMENT_STATS[2].load(Ordering::Relaxed)
            );
        }
        let last = segments.open.len() - 1;
        let (buffer, at) = take(&mut segments.open[last], &buffer);
        Some((buffer, at, true))
    }

    /// Notes what the command about to be recorded does, as a marker
    /// command whose text the list keeps (diagnostics, with device-removed
    /// data on): the report pairs the breadcrumbs' marker commands with the
    /// texts in order and names the commands a fault stopped among.
    #[cfg(feature = "diagnostics")]
    pub(crate) fn note_op(&self, what: impl FnOnce() -> String) {
        self.note(what, None);
    }

    /// As [`HostList::note_op`], for a copy that reads the host GPU range
    /// `(address, bytes)`: the report places the faulting address in it.
    #[cfg(feature = "diagnostics")]
    pub(crate) fn note_read(&self, what: impl FnOnce() -> String, range: (u64, u64)) {
        self.note(what, Some(range));
    }

    #[cfg(feature = "diagnostics")]
    pub(super) fn note(&self, what: impl FnOnce() -> String, range: Option<(u64, u64)>) {
        // A frame shot times the command: the card's clock before it.
        crate::shot::stamp(self);
        // Only the full form of device-removed data goes on: `light` and
        // `faults` make no text and set no marker, as with the data off.
        if !dred_on() {
            crate::capture::op(std::ptr::from_ref(self) as usize, what);
            return;
        }
        {
            let started = std::time::Instant::now();
            let at = self.notes.lock().unwrap_or_else(|e| e.into_inner()).len() as u32;
            if let Some((heap, _)) = self.stamps() && at < STAMPS - 1 {
                // SAFETY: an open list and our query heap.
                unsafe { self.list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, at) };
            }
            let what = what();
            crate::capture::op(std::ptr::from_ref(self) as usize, || what.clone());
            let text: Vec<u16> = what.encode_utf16().chain([0]).collect();
            const UNICODE: u32 = 0;
            // SAFETY: an open list; the marker is copied when recorded.
            unsafe {
                self.list.SetMarker(UNICODE, Some(text.as_ptr().cast()), 2 * text.len() as u32)
            };
            self.notes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((what, range));
            crate::copystat::add(crate::copystat::C::NoteNs, started.elapsed().as_nanos() as u64);
        }
    }

    /// Whether nothing recorded since the list was opened does any work:
    /// no draw or dispatch (the first keeps its pipeline), no copy, clear,
    /// upload or resolve (they defer a step or move a texture's state), no
    /// bottom-of-pipe write. State set on the list does not count.
    pub(crate) fn untouched(&self) -> bool {
        {
            let deferred = self.deferred();
            if !(deferred.copies.is_empty()
                && deferred.uploads.is_empty()
                && deferred.kept.is_empty()
                && deferred.textures.is_empty()
                && deferred.writes.is_empty()
                && deferred.backs.is_empty()
                && deferred.texture_backs.is_empty())
            {
                return false;
            }
        }
        self.states.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
            && self.twins.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// Notes a pipeline the list binds; called where it is bound.
    pub(crate) fn used_pipeline(&self, name: &str) {
        {
            let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
            if !pipelines.contains(name) {
                pipelines.insert(name.to_owned());
            }
        }
        if crate::cardmeter::pipelines() {
            // The pipeline before it ends here on the card's clock.
            crate::cardmeter::end(&self.list, crate::cardmeter::Span::from_raw(self.pipeline_meter.swap(u32::MAX, Ordering::Relaxed)));
            self.pipeline_meter.store(crate::cardmeter::begin_named(&self.list, name).raw(), Ordering::Relaxed);
        }
    }

    /// With [`list_reuse`]: whether this is the first time since the reset
    /// that the list binds the host pipeline at `pipeline` (its interface
    /// pointer). The list keeps the pipeline from its first bind to its
    /// reset, so the address names the same one until then, and a later
    /// bind need not keep it and its root signature again.
    pub(crate) fn first_bind(&self, pipeline: usize) -> bool {
        self.deferred().bound.insert(pipeline)
    }

    /// With [`list_reuse`]: the host GPU address of `len` bytes of upload
    /// memory holding the title memory at `src` as of the list's
    /// submission: one copy for every ask until a `Copy` or `Run` step
    /// intervenes (every fill reads at the submission, in step order, so a
    /// second fill of the same bytes with neither between them would read
    /// what the first did).
    pub(crate) fn fill_scratch(&self, src: *const u8, len: u64) -> Option<u64> {
        let key = (src as usize, len as usize);
        let found = self.deferred().filled.get(&key).copied();
        if let Some(gpu) = found {
            return Some(gpu);
        }
        let (gpu, to) = self.scratch(len)?;
        self.defer_fill(to, src, len as usize);
        self.deferred().filled.insert(key, gpu);
        Some(gpu)
    }

    pub(super) fn deferred(&self) -> std::sync::MutexGuard<'_, Deferred> {
        self.deferred.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Copies `len` bytes from `src` to `dst` when the list is submitted.
    pub(crate) fn defer_copy(&self, dst: *mut u8, src: *const u8, len: usize) {
        let mut deferred = self.deferred();
        COPIES_JOINED[0].fetch_add(1, Ordering::Relaxed);
        // A copy that goes on where the one recorded just before it ended,
        // source and destination both: with `join_copies` the two are one
        // step.
        if let Some(Step::Copy(d, s, n)) = deferred.copies.last_mut()
            && continues((*d, *s, *n), (dst as usize, src as usize, len))
        {
            COPIES_JOINED[1].fetch_add(1, Ordering::Relaxed);
            if join_copies() {
                *n += len;
                return;
            }
        }
        deferred
            .copies
            .push(Step::Copy(dst as usize, src as usize, len));
        deferred.refreshed.clear();
        if list_reuse_set() {
            deferred.filled.clear();
        }
    }

    /// Copies `len` bytes of title memory at `src` into the list's upload
    /// memory at `dst` when the list is submitted.
    pub(crate) fn defer_fill(&self, dst: *mut u8, src: *const u8, len: usize) {
        self.deferred()
            .copies
            .push(Step::Fill(dst as usize, src as usize, len, None));
    }

    /// As [`HostList::defer_fill`], for upload memory that only `slot` (a
    /// title heap's address, and the slot) shows.
    pub(crate) fn defer_constants(&self, dst: *mut u8, src: *const u8, len: usize, slot: (usize, u32)) {
        self.deferred()
            .copies
            .push(Step::Fill(dst as usize, src as usize, len, Some(card_steps::FillTo::Slot(slot.0, slot.1))));
    }

    /// As [`HostList::defer_fill`], for the beginning of a buffer
    /// [`HostList::upload`] made.
    pub(crate) fn defer_fill_buffer(&self, buffer: &ID3D12Resource, dst: *mut u8, src: *const u8, len: usize) {
        let to = (card_steps::stage() > 1).then(|| card_steps::FillTo::Buffer(buffer.clone()));
        self.deferred().copies.push(Step::Fill(dst as usize, src as usize, len, to));
    }

    /// Brings `[address, address + len)` of a buffer's host copy up to date
    /// when the list is submitted (once per range until a title copy
    /// intervenes), keeping the copy alive until the list is reset.
    pub(crate) fn defer_refresh(
        &self,
        shadow: &std::sync::Arc<crate::shadow::Shadow>,
        address: usize,
        len: usize,
    ) {
        let mut deferred = self.deferred();
        let key = (std::sync::Arc::as_ptr(shadow) as usize, address, len);
        if deferred.refreshed.insert(key) {
            deferred
                .copies
                .push(Step::Refresh(shadow.clone(), address, len));
        }
    }

    /// Shaders of the list write `[address, address + len)` of a buffer's
    /// host copy: copied back into title memory once the list has run.
    pub(crate) fn defer_gpu_write(
        &self,
        shadow: &std::sync::Arc<crate::shadow::Shadow>,
        address: usize,
        len: usize,
    ) {
        self.deferred()
            .copies
            .push(Step::GpuWrite(shadow.clone(), address, len));
    }

    /// A read back: once the list has run, `rows` rows of `row` bytes go
    /// from `buffer` (rows 256-byte pitched, see `PlacedFootprint::host_rows`)
    /// to title memory at `dst`, pitched `pitch`. The buffer lives until
    /// the list is reset.
    pub(crate) fn defer_read_back(&self, buffer: ID3D12Resource, dst: *mut u8, [row, rows, pitch]: [usize; 3]) {
        self.deferred().backs.push((buffer, dst as usize, [row, rows, pitch]));
    }

    /// Stores `what` (`bytes` of it) at `dst` once the list has run.
    pub(crate) fn defer_write(&self, dst: *mut u8, what: crate::queue::Written, bytes: usize) {
        self.deferred().writes.push((dst as usize, what, bytes));
    }

    /// Whether the list, once it has run, stores at `address` a value of
    /// `bytes` bytes or more for which `holds` says yes (one of its
    /// bottom-of-pipe writes, as [`late_met`] reads them of a flight).
    pub(crate) fn will_write(&self, address: usize, bytes: usize, holds: impl Fn(u64) -> bool) -> bool {
        self.deferred()
            .writes
            .iter()
            .any(|(at, what, n)| *at == address && *n >= bytes && matches!(what, crate::queue::Written::Value(v) if holds(*v)))
    }

    /// A mapped upload buffer of `size` bytes, alive until the list is
    /// reset.
    ///
    /// shortcut: one buffer per copy; pool them once uploads are frequent.
    /// (With [`card_steps`]' stage 2 they are: a buffer of the upload's size
    /// class, kept in [`SHARED_SPARE`] between lists; it may be larger than
    /// `size`.)
    pub(crate) fn upload(&self, host: &Host, size: u64) -> Option<(ID3D12Resource, *mut u8)> {
        // With [`card_steps`] the card may be the one that fills it.
        if card_steps::stage() > 1
            && let Some(class) = shared_class(size)
        {
            let held = take_shared(host, class)?;
            let kept = held.kept.as_ref()?;
            let (buffer, data) = (kept.buffer.clone(), kept.data as *mut u8);
            name_frame(&buffer, || format!("list upload {size:#x} (shared, {class:#x})"));
            let mut deferred = self.deferred();
            deferred.hold_upload(buffer.clone(), class);
            deferred.shared.push(held);
            return Some((buffer, data));
        }
        let (buffer, data) = if card_steps::stage() > 1 { host.shared_buffer(size)? } else { host.upload_buffer(size)? };
        name_frame(&buffer, || format!("list upload {size:#x}"));
        self.deferred().hold_upload(buffer.clone(), size);
        Some((buffer, data))
    }

    /// `len` bytes of upload memory (256-byte aligned) until the list is
    /// reset: the host GPU address and the CPU pointer.
    pub(crate) fn scratch(&self, len: u64) -> Option<(u64, *mut u8)> {
        let host = host()?;
        let mut s = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
        let len = len.max(1).next_multiple_of(256);
        if len > SCRATCH_CHUNK {
            let (buffer, data) = host.upload_buffer(len)?;
            name_frame(&buffer, || format!("list scratch {len:#x}"));
            // SAFETY: a live buffer.
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            self.deferred().hold_upload(buffer, len);
            return Some((gpu, data));
        }
        // The chunk in use while `len` fits in what is left of it, else the
        // next one (a list's first chunk may be a small one).
        while s.current < s.chunks.len() && s.used + len > s.chunks[s.current].1 {
            (s.current, s.used) = (s.current + 1, 0);
        }
        if s.current == s.chunks.len() {
            let size = if scratch_pool() && s.chunks.is_empty() && len <= SCRATCH_FIRST {
                SCRATCH_FIRST
            } else {
                SCRATCH_CHUNK
            };
            let chunk = take_chunk(host, size)?;
            s.chunks.push((chunk, size));
            s.used = 0;
        }
        let (data, gpu) = (s.chunks[s.current].0.data, s.chunks[s.current].0.gpu);
        let at = s.used;
        s.used += len;
        Some((gpu + at, (data + at as usize) as *mut u8))
    }

    /// The bytes of scratch chunks the list holds now (after a reset:
    /// none while lists share their chunks).
    pub(crate) fn scratch_kept(&self) -> u64 {
        let scratch = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
        scratch.chunks.iter().map(|c| c.1).sum()
    }

    /// The host address of the list's indirect-work budget, zeroed at each
    /// submission of the list from the first use after a reset on.
    /// The list's window of indirect draws clamped together (`crate::indirect::windowed`).
    pub(crate) fn clamp_window(&self) -> std::sync::MutexGuard<'_, Option<crate::indirect::Window>> {
        self.clamp_window.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An occlusion query begun on the host in this list ([`crate::query::begin`]).
    pub(crate) fn query_opened(&self, host: ID3D12QueryHeap, kind: u32, index: u32, console: usize) {
        self.open_queries.lock().unwrap_or_else(|e| e.into_inner()).push((host, kind, index, console));
    }

    /// The query `index` of the console heap `console` was ended in this list ([`crate::query::end`]).
    pub(crate) fn query_closed(&self, console: usize, index: u32) {
        self.open_queries.lock().unwrap_or_else(|e| e.into_inner()).retain(|q| !(q.3 == console && q.2 == index));
    }

    pub(crate) fn budget(&self) -> Option<u64> {
        let (buffer, data) = self
            .budget
            .get_or_init(|| {
                let (b, d) = host()?.shared_buffer(256)?;
                Some((b, d as usize))
            })
            .as_ref()?;
        let card = self.card_budget.get_or_init(|| self.make_card_budget()).as_ref();
        let mut deferred = self.deferred();
        if !deferred.budget_zeroed {
            deferred.budget_zeroed = true;
            let at = *data;
            // SAFETY: the list's own mapped words, written before it runs.
            // The words after the budget hold what the clamp noted about the
            // list's last run (mesh dispatches: the largest group count
            // asked for, the groups asked in all; with the card's copy, as
            // the list's end copied them here): taken, then zeroed.
            let notes = Box::new(move || unsafe {
                let words = at as *mut u32;
                let (largest, all) = (words.add(1).read_volatile(), words.add(2).read_volatile());
                MESH_GROUPS[0].fetch_max(u64::from(largest), Ordering::Relaxed);
                // A record past what one dispatch runs: indirect dispatches
                // go through the spread signature from now on
                // (`draw::indirect_spread`).
                if crate::draw::asks_spread(largest) && !crate::draw::SPREAD_SEEN.swap(true, Ordering::Relaxed) {
                    tracing::info!(largest, "an indirect record asked for more than 65,535 groups: indirect dispatches are spread across z from now on");
                }
                MESH_GROUPS[1].fetch_add(u64::from(all), Ordering::Relaxed);
                for k in 0..3 {
                    words.add(k).write_volatile(0);
                }
            });
            deferred.copies.push(Step::Host(notes, (0, 0)));
            drop(deferred);
            if let Some(card) = card {
                self.zero_card_budget(card);
            }
        }
        // SAFETY: a live buffer.
        Some(unsafe { card.unwrap_or(buffer).GetGPUVirtualAddress() })
    }

    /// The budget's words in the card's own memory ([`card_budget`]), if
    /// the list can zero them itself (`WriteBufferImmediate`).
    pub(super) fn make_card_budget(&self) -> Option<ID3D12Resource> {
        if !card_budget() {
            return None;
        }
        self.list.cast::<ID3D12GraphicsCommandList2>().ok()?;
        let buffer = host()?.uav_buffer(256)?;
        name(&buffer, || "indirect budget".to_owned());
        Some(buffer)
    }

    /// Records, before the list's first clamp, the zeroing of the card's
    /// copy of the budget: common, written as a copy, then left for
    /// unordered access until the list closes ([`HostList::card_budget_out`]).
    pub(super) fn zero_card_budget(&self, card: &ID3D12Resource) {
        let Ok(list2) = self.list.cast::<ID3D12GraphicsCommandList2>() else {
            return;
        };
        // SAFETY: a live buffer.
        let at = unsafe { card.GetGPUVirtualAddress() };
        let zeros = [0u64, 4, 8].map(|offset| D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
            Dest: at + offset,
            Value: 0,
        });
        // SAFETY: an open list; the buffer is the list's own and in the
        // common state at the start of every recording.
        unsafe {
            self.list.ResourceBarrier(&[transition(
                card,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )]);
            list2.WriteBufferImmediate(zeros.len() as u32, zeros.as_ptr(), None);
            self.list.ResourceBarrier(&[transition(
                card,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )]);
        }
    }

    /// Before the list closes, if this recording charged the card's copy
    /// of the budget: its notes on mesh records go to the mapped words
    /// (read at the next submission), and it goes back to common.
    pub(super) fn card_budget_out(&self) {
        if !self.deferred().budget_zeroed {
            return;
        }
        let (Some(Some(card)), Some(Some((mapped, _)))) =
            (self.card_budget.get(), self.budget.get())
        else {
            return;
        };
        // SAFETY: an open list; both buffers are the list's own. The
        // mapped one is promoted from common to the copy destination.
        unsafe {
            self.list.ResourceBarrier(&[transition(
                card,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            self.list.CopyBufferRegion(mapped, 4, card, 4, 8);
            self.list.ResourceBarrier(&[transition(
                card,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )]);
        }
    }

    /// A device-local unordered-access buffer of at least `size` bytes for
    /// clamped indirect arguments, reused across executions and resets of the
    /// list. Smaller buffers replaced mid-list stay alive until
    /// [`HostList::reset`].
    pub(crate) fn indirect_scratch(&self, size: u64) -> Option<ID3D12Resource> {
        let needed = size.max(64 << 10);
        let mut slot = self.indirect_args.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((buffer, cap)) = &*slot
            && *cap >= needed
        {
            return Some(buffer.clone());
        }
        let prev = slot.as_ref().map_or(0, |(_, cap)| *cap);
        let alloc = prev
            .saturating_mul(2)
            .max(needed)
            .next_multiple_of(64 << 10);
        let buffer = host()?.uav_buffer(alloc)?;
        name(&buffer, || format!("indirect arguments {alloc:#x}"));
        let old = slot.replace((buffer.clone(), alloc));
        drop(slot);
        if let Some((old_buf, _)) = old {
            // It ends the list common, as every scratch buffer does.
            self.indirect_common(&old_buf);
            self.keep(&old_buf);
        }
        Some(buffer)
    }

    /// Moves `scratch` back to the common state if a clamp left it readable
    /// as indirect arguments ([`HostList::indirect_readable`]): where the
    /// list's first clamp expects it. Done before the list closes and before
    /// a bigger scratch buffer replaces it: one barrier per list (where
    /// there was one per indirect execution), and the list ends as it
    /// begins however it is submitted.
    pub(super) fn indirect_common(&self, scratch: &ID3D12Resource) {
        {
            let mut deferred = self.deferred();
            if deferred.indirect_readable != scratch.as_raw() as usize {
                return;
            }
            deferred.indirect_readable = 0;
        }
        let barrier = transition(
            scratch,
            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            D3D12_RESOURCE_STATE_COMMON,
        );
        // SAFETY: an open list and a live buffer this recording left in
        // `INDIRECT_ARGUMENT`.
        unsafe { self.list.ResourceBarrier(&[barrier]) };
    }

    /// The state the list's indirect scratch buffer `scratch` is in at this
    /// point of the recording: indirect arguments if a clamp in this
    /// recording left it so ([`HostList::indirect_readable`]), otherwise
    /// common: the state a buffer is made in, and the one a recording
    /// leaves it in when it closes or replaces it ([`HostList::indirect_common`]).
    pub(crate) fn indirect_state(&self, scratch: &ID3D12Resource) -> D3D12_RESOURCE_STATES {
        if self.deferred().indirect_readable == scratch.as_raw() as usize {
            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT
        } else {
            D3D12_RESOURCE_STATE_COMMON
        }
    }

    /// A clamp recorded in this list left `scratch` readable as indirect
    /// arguments. It stays so for every execution that reads it, until the
    /// next clamp moves it back to unordered access.
    pub(crate) fn indirect_readable(&self, scratch: &ID3D12Resource) {
        self.deferred().indirect_readable = scratch.as_raw() as usize;
    }

    /// The list's buffers for `crate::indirect::window_check`.
    pub(crate) fn window_check(&self) -> Option<&crate::indirect::WindowCheck> {
        self.window_check.get_or_init(crate::indirect::WindowCheck::new).as_ref()
    }

    /// The shader-visible descriptor heaps the list keeps (tests).
    #[cfg(test)]
    pub(crate) fn kept_shader_heaps(&self) -> Vec<ID3D12DescriptorHeap> {
        self.deferred().kept.iter()
            .filter_map(|object| object.cast::<ID3D12DescriptorHeap>().ok())
            .filter(|heap| {
                // SAFETY: a live descriptor heap.
                unsafe { heap.GetDesc() }.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
            })
            .collect()
    }

    /// Keeps `object` alive until the list is reset.
    pub(crate) fn keep(&self, object: &impl Interface) {
        if let Ok(object) = object.cast::<windows::core::IUnknown>() {
            self.deferred().kept.push(object);
        }
    }

    /// Whether this is the first time since the reset that the list asks
    /// about `key` (a texture to bring up to date).
    pub(crate) fn first_use(&self, key: usize) -> bool {
        self.deferred().textures.insert(key)
    }

    pub(crate) fn alias_source_changed(&self, view: &ID3D12Resource, source: &ID3D12Resource) -> bool {
        let view = view.as_raw() as usize;
        let source = source.as_raw() as usize;
        self.deferred().texture_aliases.insert(view, source) != Some(source)
    }

    /// Runs `work` when the list is submitted, in order with its copies.
    /// `work` may write title memory, and only in `[at, at + len)`
    /// (refreshes after it copy again; [`near_writes`] uses the place,
    /// without the switch any write of title memory is assumed).
    pub(crate) fn defer_at(&self, at: usize, len: usize, work: impl Fn() + Send + 'static) {
        self.deferred().copies.push(Step::Run(Box::new(work), Some((at, len))));
        self.deferred().refreshed.clear();
        if list_reuse_set() {
            self.deferred().filled.clear();
        }
    }

    /// As [`HostList::defer`], for work that writes only host memory.
    pub(crate) fn defer_host(&self, work: impl Fn() + Send + 'static) {
        self.deferred().copies.push(Step::Host(Box::new(work), (0, 0)));
    }

    /// As [`HostList::defer_host`], for work that reads `len` bytes of title
    /// memory at `from`.
    pub(crate) fn defer_host_read(&self, from: usize, len: usize, work: impl Fn() + Send + 'static) {
        self.deferred().copies.push(Step::Host(Box::new(work), (from, from + len)));
    }

    /// Brings a texture's staging buffer up to date when the list is
    /// submitted, keeping it alive until the list is reset.
    pub(crate) fn defer_texture(&self, staging: std::sync::Arc<crate::texture::Staging>) {
        self.deferred().copies.push(Step::Texture(staging));
    }

    /// Detiles the textures the list uploads now, ahead of its submission
    /// ([`crate::texture::Staging::refresh_ahead`]); how many were left.
    pub(crate) fn refresh_textures_ahead(&self) -> u64 {
        let textures: Vec<_> = self
            .deferred()
            .copies
            .iter()
            .filter_map(|step| match step {
                Step::Texture(staging) => Some(staging.clone()),
                _ => None,
            })
            .collect();
        // While the queue is behind (the last pass left slots due), on
        // threads of their own (`VICTORY_REFRESH_WIDE=<n>`, 4 unless said;
        // `0` or `1`: one after the other on this thread, as before). Each
        // texture's staging locks itself; reading a texture whose pages are
        // out of memory waits on the disk, and one thread a texture at a
        // time left thousands of slots due at a change of shot. Only while
        // behind: otherwise these reads compete with the submitting thread's
        // copies for the same memory. Not the kept wide threads: the
        // submitting thread waits behind its own jobs there. Only with the
        // heap worker (`VICTORY_HEAP_WORKER=1`, off by default), whose lists
        // are the ones read ahead: without it nothing changes.
        // Risks, not measured here: (1) these threads take the lock of
        // `watch::writes` like any refresh, but `VICTORY_COPY_PLAIN` (the
        // in-place copies of the lab's G1 profile) copies without it, so
        // with both on a texture may be read while a plain copy rewrites
        // the same title memory (the read stays whole or fails and is
        // done again at the submission, but nothing orders the two);
        // (2) each flush makes new threads, and each thread's read buffer
        // (`READ`) grows to the largest texture it read: more short peaks
        // of memory while the queue is behind. Do not turn the worker on
        // for the long sessions until both have been looked at in a run.
        static THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let threads = *THREADS.get_or_init(|| {
            std::env::var("VICTORY_REFRESH_WIDE").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4usize).clamp(1, 8)
        });
        if threads < 2
            || textures.len() < 4
            || !crate::heap_worker::on()
            || !crate::draw::BACKLOG_LEFT.load(Ordering::Relaxed)
        {
            return textures.iter().filter(|staging| !staging.refresh_ahead()).count() as u64;
        }
        let per = textures.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let parts: Vec<_> = textures
                .chunks(per)
                .map(|chunk| scope.spawn(move || chunk.iter().filter(|staging| !staging.refresh_ahead()).count() as u64))
                .collect();
            parts.into_iter().map(|p| p.join().unwrap_or(0)).sum()
        })
    }

    /// Runs the list's submission-time steps; refreshes go into `batch`
    /// (the title's big buffers, a 174 MB one among them, are read by most
    /// lists of a submission, through many ranges). Copies and fills that
    /// `taken` names (by their place) are left out: they are done on the
    /// card ([`card_steps`]).
    pub(super) fn run_deferred(&self, batch: &mut Batch, taken: impl Fn(usize) -> bool) {
        #[cfg(feature = "diagnostics")]
        use crate::copystat::{C, add};
        for (i, step) in self.deferred().copies.iter().enumerate() {
            if matches!(step, Step::Copy(..) | Step::Fill(..)) && taken(i) {
                continue;
            }
            let started = std::time::Instant::now();
            let copied = match step {
                &Step::Copy(dst, src, len) => {
                    #[cfg(feature = "diagnostics")]
                    {
                        add(C::Copies, 1);
                        add(C::CopyBytes, len as u64);
                    }
                    if near_writes() {
                        batch.title_wrote_near(dst, len, false);
                    } else {
                        batch.title_wrote(dst, len);
                    }
                    #[cfg(feature = "diagnostics")]
                    fill_check::copy_runs(dst, src, len);
                    #[cfg(feature = "diagnostics")]
                    let _reads_as = crate::backreads::reading_as(crate::backreads::TITLE_COPY, 0);
                    crate::port::copied(dst, src, len);
                    copy_mapped(dst as *mut u8, src as *const u8, len, true);
                    // With the submission's one asking: the journal learns it.
                    crate::watch::wrote(dst, len);
                    len
                }
                &Step::Fill(dst, src, len, _) => {
                    #[cfg(feature = "diagnostics")]
                    {
                        add(C::Copies, 1);
                        add(C::CopyBytes, len as u64);
                        fill_check::runs(src, len);
                    }
                    #[cfg(feature = "diagnostics")]
                    let _reads_as = crate::backreads::reading_as(crate::backreads::TITLE_COPY, 0);
                    crate::port::filled(src, len);
                    copy_fill(dst as *mut u8, src as *const u8, len);
                    len
                }
                Step::Refresh(shadow, address, len) => {
                    #[cfg(feature = "diagnostics")]
                    {
                        add(C::Refreshes, 1);
                        add(C::RefreshBytes, *len as u64);
                    }
                    batch.add(shadow, *address, *len);
                    continue;
                }
                Step::Run(work, Some((at, len))) if near_writes() => {
                    // Its place is known: only what it touches is brought up
                    // to date first and checked again after it, and the
                    // submission's one asking stands, told of the write, as
                    // for a title buffer copy ([`near_writes`]).
                    let written = batch.title_wrote_near(*at, *len, true);
                    work();
                    for &(from, to) in &written {
                        crate::watch::wrote(from, to - from);
                    }
                    #[cfg(feature = "diagnostics")]
                    {
                        add(C::Runs, 1);
                        add(C::RunUs, started.elapsed().as_micros() as u64);
                    }
                    continue;
                }
                Step::Run(work, _) | Step::Host(work, _) => {
                    if matches!(step, Step::Run(..)) {
                        batch.title_written();
                    } else if !batch.due_textures.is_empty() {
                        // Host callbacks can use a refresh result, including the
                        // mip mask that maps a reserved texture for this upload.
                        batch.check_textures();
                    }
                    work();
                    // It wrote title memory, where is not known here: the
                    // submission's one asking ends, later refreshes ask anew.
                    if matches!(step, Step::Run(..)) {
                        crate::watch::ask_once_end();
                    }
                    #[cfg(feature = "diagnostics")]
                    {
                        add(C::Runs, 1);
                        add(C::RunUs, started.elapsed().as_micros() as u64);
                    }
                    continue;
                }
                Step::GpuWrite(shadow, address, len) => {
                    // The shaders start from the title's data: this copy's
                    // pending refresh first. The other copies' wait for
                    // the next flush of all (flushing all of them at every
                    // range a dispatch writes made thousands of write-watch
                    // askings a frame).
                    batch.flush_one(shadow);
                    #[cfg(feature = "diagnostics")]
                    let snapshot = std::time::Instant::now();
                    shadow.writing(*address, *len);
                    #[cfg(feature = "diagnostics")]
                    {
                        add(C::GpuWrites, 1);
                        add(C::SnapshotUs, snapshot.elapsed().as_micros() as u64);
                    }
                    continue;
                }
                Step::Texture(staging) => {
                    if batch.texture_due(staging) {
                        if near_writes() {
                            // Where it lies, for a write of ours at a known
                            // place to leave it checked ([`Batch::title_wrote_near`]).
                            batch
                                .texture_spans
                                .insert(std::sync::Arc::as_ptr(staging) as *const () as usize, staging.memory());
                        }
                        if parallel_refresh() {
                            // Checked with the others of the batch, several
                            // at a time, before the next step that writes
                            // title memory ([`Batch::check_textures`]).
                            batch.due_textures.push(staging.clone());
                        } else {
                            staging.refresh();
                        }
                    }
                    continue;
                }
            };
            preparing(0, started.elapsed().as_micros() as u64);
            preparing(1, copied as u64);
        }
    }
}
