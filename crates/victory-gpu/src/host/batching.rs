//! [`Batch`]'s methods: the fills put off, the refreshes of host copies
//! and the textures due (moved out of host.rs as is).

use super::*;

impl Batch {
    /// With [`wide_fills`]: the fills noted are done, several at a time
    /// when there are many (`crate::shadow::wide_pieces`, this thread among
    /// them), all done before it returns. The time counted is the time it
    /// took, not the threads' sum; the bytes were counted when noted.
    pub(super) fn fill(&mut self) {
        // Below this many the threads cost more than they save (theirs).
        const FEW: usize = 96;
        const PART: usize = 64;
        if self.fills.is_empty() {
            return;
        }
        self.fill_spans.clear();
        let started = std::time::Instant::now();
        if self.fills.len() < FEW {
            for (dst, src, len) in self.fills.drain(..) {
                copy_fill(dst as *mut u8, src as *const u8, len);
            }
        } else {
            let fills = std::mem::take(&mut self.fills);
            crate::shadow::wide_pieces(fills.len().div_ceil(PART), move |i| {
                for &(dst, src, len) in fills.iter().skip(i * PART).take(PART) {
                    copy_fill(dst as *mut u8, src as *const u8, len);
                }
            });
        }
        preparing(0, started.elapsed().as_micros() as u64);
    }

    /// With [`wide_fills`]: a fill noted, done later by [`Batch::fill`].
    /// Done together, fills land in any order; in steps the later one
    /// wins where two write the same bytes. So if its destination overlaps
    /// that of one noted already, those noted are done first ([`Batch::fill`])
    /// and counted ([`FILLS_OVERLAPPING`], told about once a second).
    pub(super) fn put_off_fill(&mut self, dst: usize, src: usize, len: usize) {
        let end = dst + len;
        if len != 0 {
            // The spans do not overlap: only the last that starts before
            // `end` can reach past `dst`.
            if self.fill_spans.range(..end).next_back().is_some_and(|(_, &to)| to > dst) {
                self.fill();
                Self::overlapped();
            }
            self.fill_spans.insert(dst, end);
        }
        self.fills.push((dst, src, len));
    }

    /// One more fill whose destination overlapped one put off; the count
    /// since the last line, told if that was a second ago or more.
    pub(super) fn overlapped() {
        FILLS_OVERLAPPING[0].fetch_add(1, Ordering::Relaxed);
        let at = now_ms();
        let before = FILLS_OVERLAPPING[1].load(Ordering::Relaxed);
        if at.saturating_sub(before) >= 1000
            && FILLS_OVERLAPPING[1].compare_exchange(before, at, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            let n = FILLS_OVERLAPPING[0].swap(0, Ordering::Relaxed);
            tracing::info!(
                "fills put off (VICTORY_WIDE_FILLS=1) whose destination overlapped one put off before, so that those were done first: \
                 {n} since the last such line"
            );
        }
    }

    pub(super) fn add(&mut self, shadow: &std::sync::Arc<crate::shadow::Shadow>, address: usize, len: usize) {
        let end = address + len;
        let key = std::sync::Arc::as_ptr(shadow) as usize;
        // Brought up to date in this submission already ([`refresh_once`];
        // nothing is in `current` without the switch).
        if let Some(&(_, from, to)) = self.current.get(&key)
            && from <= address
            && end <= to
        {
            #[cfg(feature = "diagnostics")]
            {
                crate::copystat::add(crate::copystat::C::Skipped, 1);
                crate::copystat::add(crate::copystat::C::SkippedBytes, len as u64);
            }
            return;
        }
        // Brought up to date ahead of the steps ([`ahead_of_writes`]).
        if self.ahead.get(&key).is_some_and(|&(from, to)| from <= address && end <= to) {
            return;
        }
        match self.index.get(&key) {
            Some(&at) => {
                let r = &mut self.refreshes[at];
                (r.1, r.2) = (r.1.min(address), r.2.max(end));
            }
            None => {
                self.index.insert(key, self.refreshes.len());
                self.refreshes.push((shadow.clone(), address, end));
            }
        }
    }

    /// Before shaders are given a range of `shadow`'s copy: its pending
    /// refresh is done, the others stay pending.
    pub(super) fn flush_one(&mut self, shadow: &std::sync::Arc<crate::shadow::Shadow>) {
        // From here on it is refreshed around what the shaders write.
        self.ahead.remove(&(std::sync::Arc::as_ptr(shadow) as usize));
        let Some(at) = self.index.remove(&(std::sync::Arc::as_ptr(shadow) as usize)) else {
            return;
        };
        let (shadow, from, to) = self.refreshes.swap_remove(at);
        // The last entry took its place.
        if let Some(moved) = self.refreshes.get(at) {
            self.index.insert(std::sync::Arc::as_ptr(&moved.0) as usize, at);
        }
        Self::update(&shadow, from, to);
        Self::brought(&mut self.current, &shadow, from, to);
    }

    /// With [`refresh_once`]: `[from, to)` of `shadow`'s copy was brought
    /// up to date. Kept: its union with what was up to date if they touch,
    /// else the larger of the two.
    pub(super) fn brought(
        current: &mut std::collections::HashMap<usize, (std::sync::Arc<crate::shadow::Shadow>, usize, usize)>,
        shadow: &std::sync::Arc<crate::shadow::Shadow>,
        from: usize,
        to: usize,
    ) {
        if !refresh_once() || from >= to {
            return;
        }
        let entry = current
            .entry(std::sync::Arc::as_ptr(shadow) as usize)
            .or_insert_with(|| (shadow.clone(), from, to));
        if from <= entry.2 && entry.1 <= to {
            (entry.1, entry.2) = (entry.1.min(from), entry.2.max(to));
        } else if to - from > entry.2 - entry.1 {
            (entry.1, entry.2) = (from, to);
        }
    }

    /// Brings `[from, to)` of a copy up to date, timed.
    pub(super) fn update(shadow: &crate::shadow::Shadow, from: usize, to: usize) {
        let started = std::time::Instant::now();
        let copied = shadow.update(from, to - from);
        let us = started.elapsed().as_micros() as u64;
        #[cfg(feature = "diagnostics")]
        crate::copystat::range(from, to - from, us, 0);
        preparing(0, us);
        preparing(1, copied as u64);
    }

    /// Whether `staging` is still to be refreshed in this submission (and
    /// notes it refreshed).
    pub(super) fn texture_due<T>(&mut self, staging: &std::sync::Arc<T>) -> bool {
        self.textures.insert(std::sync::Arc::as_ptr(staging) as *const () as usize)
    }

    /// Before a step that writes title memory: pending refreshes are done
    /// first, and textures are checked again after it.
    pub(super) fn title_written(&mut self) {
        self.check_textures();
        self.flush();
        self.textures.clear();
        self.texture_spans.clear();
        // Where it writes is not known: no copy counts as up to date.
        self.current.clear();
        self.ahead.clear();
    }

    /// [`Batch::title_written`] for a write of ours whose place is known
    /// (a title buffer copy into `[at, at + len)`): of a copy up to date
    /// in this submission that holds some of those bytes, the larger part
    /// beside them stays so.
    pub(super) fn title_wrote(&mut self, at: usize, len: usize) {
        self.check_textures();
        self.flush();
        // What was brought up to date ahead was so up to the first write.
        self.ahead.clear();
        self.textures.clear();
        self.texture_spans.clear();
        let end = at + len;
        self.current.retain(|_, entry| {
            let (from, to) = (entry.1, entry.2);
            if end <= from || to <= at {
                return true;
            }
            let (left, right) = (at.saturating_sub(from), to.saturating_sub(end));
            if left >= right {
                entry.2 = from + left;
            } else {
                entry.1 = end;
            }
            entry.1 < entry.2
        });
    }

    /// With [`near_writes`]: a write of ours into `[at, at + len)` of title
    /// memory (a title buffer copy, or with `run` a memory-to-memory copy).
    /// Only the pending refreshes whose span holds written bytes are done
    /// first, and only the textures whose memory holds them are checked
    /// again after it; the others stay pending or checked. Where the place
    /// is a view of the title's physical pages, every other address that
    /// shows the same 64 KiB pages counts as written too
    /// (`crate::inplace::pages_of`); a view write of more than
    /// [`NEAR_VIEW_PAGES`] pages, or more than [`NEAR_PENDING`] refreshes
    /// pending, is done as before ([`Batch::title_wrote`]). Returns the
    /// written ranges (the place, and the other addresses of its pages).
    pub(super) fn title_wrote_near(&mut self, at: usize, len: usize, run: bool) -> Vec<(usize, usize)> {
        // Textures due a check read title memory as it is before the write.
        self.check_textures();
        // What was brought up to date ahead was so up to the first write.
        self.ahead.clear();
        use std::sync::atomic::Ordering::Relaxed;
        const K64: usize = 1 << 16;
        let end = at.saturating_add(len);
        let mut written = vec![(at, end)];
        let mut whole = self.refreshes.len() > NEAR_PENDING;
        if !whole && len > 0 && !crate::inplace::views_between(at, end).is_empty() {
            let (first, last) = (at / K64, end.div_ceil(K64));
            if last - first > NEAR_VIEW_PAGES {
                whole = true;
            } else {
                for page in first..last {
                    for other in crate::inplace::pages_of(page * K64) {
                        if other != page * K64 {
                            written.push((other, other + K64));
                        }
                    }
                }
            }
        }
        NEAR[usize::from(run)].fetch_add(1, Relaxed);
        if whole {
            NEAR[2].fetch_add(1, Relaxed);
            self.title_wrote(at, len);
            near_line();
            return written;
        }
        let touches = |from: usize, to: usize| written.iter().any(|&(a, b)| from < b && a < to);
        // The refreshes that hold written bytes: done now, before the write.
        let pending = std::mem::take(&mut self.refreshes);
        let (due, keep): (Vec<_>, Vec<_>) = pending.into_iter().partition(|r| touches(r.1, r.2));
        NEAR[3].fetch_add(due.len() as u64, Relaxed);
        NEAR[4].fetch_add(keep.len() as u64, Relaxed);
        if !due.is_empty() {
            self.refreshes = due;
            self.flush();
        }
        self.refreshes = keep;
        self.index = self
            .refreshes
            .iter()
            .enumerate()
            .map(|(i, r)| (std::sync::Arc::as_ptr(&r.0) as usize, i))
            .collect();
        // Textures: checked again only where the write lands.
        let before = self.textures.len();
        let spans = &self.texture_spans;
        self.textures.retain(|key| spans.get(key).is_some_and(|&(a, n)| !touches(a, a.saturating_add(n))));
        let textures = &self.textures;
        self.texture_spans.retain(|key, _| textures.contains(key));
        NEAR[5].fetch_add(self.textures.len() as u64, Relaxed);
        NEAR[6].fetch_add((before - self.textures.len()) as u64, Relaxed);
        // [`refresh_once`]'s note of what is up to date, as in `title_wrote`.
        for &(a, b) in &written {
            self.current.retain(|_, entry| {
                let (from, to) = (entry.1, entry.2);
                if b <= from || to <= a {
                    return true;
                }
                let (left, right) = (a.saturating_sub(from), to.saturating_sub(b));
                if left >= right {
                    entry.2 = from + left;
                } else {
                    entry.1 = b;
                }
                entry.1 < entry.2
            });
        }
        near_line();
        written
    }

    /// With [`parallel_refresh`]: checks the textures due ([`Batch::due_textures`])
    /// on the helper threads (`crate::shadow::wide_run`, the submitting
    /// thread among them), all done before it returns. Each check takes
    /// its texture's own lock, the journal's for its asking, the runtime's
    /// mapping lock for reading (shared), and a buffer of its thread's own;
    /// the hashing and detiling run side by side. "textures N ms" counts
    /// the time it took, not the threads' sum.
    pub(super) fn check_textures(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        if self.due_textures.is_empty() {
            return;
        }
        let due = std::mem::take(&mut self.due_textures);
        PARALLEL_CHECKS[0].fetch_add(1, Relaxed);
        PARALLEL_CHECKS[1].fetch_add(due.len() as u64, Relaxed);
        if due.len() < 2 {
            for staging in due {
                staging.refresh();
            }
            return;
        }
        let started = std::time::Instant::now();
        let before = PREPARING[2].load(Relaxed);
        let jobs: Vec<Box<dyn FnOnce() + Send>> = due
            .into_iter()
            .map(|staging| Box::new(move || staging.refresh()) as Box<dyn FnOnce() + Send>)
            .collect();
        crate::shadow::wide_run(jobs);
        let summed = PREPARING[2].load(Relaxed).saturating_sub(before);
        let wall = started.elapsed().as_micros() as u64;
        PREPARING[2].fetch_sub(summed.saturating_sub(wall), Relaxed);
        PARALLEL_CHECKS[2].fetch_add(summed, Relaxed);
        PARALLEL_CHECKS[3].fetch_add(wall, Relaxed);
        let batches = PARALLEL_CHECKS[0].load(Relaxed);
        if batches.is_multiple_of(1 << 14) {
            let [b, n, sum, w] = [0, 1, 2, 3].map(|i| PARALLEL_CHECKS[i].load(Relaxed));
            tracing::info!(
                "texture checks side by side (VICTORY_PARALLEL_REFRESH) so far: {b} batches, {n} textures; the threads' sum {} ms, the time taken {} ms",
                sum / 1000,
                w / 1000
            );
        }
    }

    /// With [`ahead_of_writes`], before the steps of `runs`: the copies that
    /// shaders are given before the first write of ours to title memory,
    /// each over the span of its refreshes before then ([`written_spans`]),
    /// brought up to date together (`crate::shadow::wide_pieces`, this
    /// thread among them), with one write-watch asking as [`Batch::flush`]
    /// makes. Copies and fills that `taken` names are done on the card and
    /// write no title memory here. The time counted is the time it took,
    /// not the threads' sum.
    pub(super) fn refresh_ahead(&mut self, runs: &[&HostList], taken: impl Fn(&HostList, usize) -> bool) {
        let mut copies: std::collections::HashMap<usize, std::sync::Arc<crate::shadow::Shadow>> =
            std::collections::HashMap::new();
        let mut touches = Vec::new();
        'steps: for list in runs {
            for (i, step) in list.deferred().copies.iter().enumerate() {
                let touch = match step {
                    Step::Refresh(shadow, at, len) => {
                        let key = std::sync::Arc::as_ptr(shadow) as usize;
                        copies.entry(key).or_insert_with(|| shadow.clone());
                        Touch::Refresh(key, *at, *at + *len)
                    }
                    Step::GpuWrite(shadow, ..) => {
                        Touch::Write(std::sync::Arc::as_ptr(shadow) as usize)
                    }
                    Step::Copy(..) | Step::CardCopy(..) if taken(list, i) => Touch::Nothing,
                    // Left to the card ([`readcopy`]): no title memory here.
                    Step::CardCopy(..) if !step.on_cpu() => Touch::Nothing,
                    Step::Copy(..) | Step::CardCopy(..) | Step::Run(..) => Touch::Title,
                    Step::Fill(..) | Step::Host(..) | Step::Texture(_) => Touch::Nothing,
                };
                touches.push(touch);
                if matches!(touch, Touch::Title) {
                    break 'steps;
                }
            }
        }
        let due: Vec<_> = written_spans(touches.into_iter())
            .into_iter()
            .filter_map(|(key, from, to)| Some((copies.get(&key)?.clone(), from, to)))
            .collect();
        if due.is_empty() {
            return;
        }
        for (shadow, from, to) in &due {
            self.ahead.insert(std::sync::Arc::as_ptr(shadow) as usize, (*from, *to));
            Self::brought(&mut self.current, shadow, *from, *to);
        }
        let started = std::time::Instant::now();
        let ask = due.len() > 1;
        if ask {
            let ranges: Vec<(usize, usize)> =
                due.iter().filter(|r| !r.0.unwatched()).map(|r| (r.1, r.2)).collect();
            crate::watch::ask_ahead(&ranges);
        }
        let before = PREPARING[0].load(Ordering::Relaxed);
        crate::shadow::wide_pieces(due.len(), move |i| {
            let (shadow, from, to) = &due[i];
            if !shadow.nothing_new(*from, *to) {
                Self::update(shadow, *from, *to);
            }
        });
        if ask {
            crate::watch::ask_ahead_done();
        }
        let summed = PREPARING[0].load(Ordering::Relaxed).saturating_sub(before);
        let wall = started.elapsed().as_micros() as u64;
        let over = summed.saturating_sub(wall);
        PREPARING[0].fetch_sub(over, Ordering::Relaxed);
        crate::framelog::sub(crate::framelog::T::CopyUs, over);
        // The asking, counted as `Batch::flush` counts its own.
        preparing(0, wall.saturating_sub(summed));
    }

    pub(super) fn flush(&mut self) {
        // Every fill put off is done before ([`Batch::fill`]): a path that
        // forgets it would leave them undone or done after what follows.
        debug_assert!(self.fills.is_empty(), "{} fills put off and not done before a flush", self.fills.len());
        self.check_textures();
        self.index.clear();
        // One write-watch asking for the copies that lie close together,
        // not one per copy.
        let ahead = self.refreshes.len() > 1;
        if ahead {
            let started = std::time::Instant::now();
            let ranges: Vec<(usize, usize)> = self
                .refreshes
                .iter()
                .filter(|r| !r.0.unwatched())
                .map(|r| (r.1, r.2))
                .collect();
            crate::watch::ask_ahead(&ranges);
            preparing(0, started.elapsed().as_micros() as u64);
        }
        if wide_refresh() && self.refreshes.len() >= 4 {
            // Each copy on its own, several at a time: a copy's refresh
            // writes that copy and reads title memory, nothing of another's.
            // The time counted is the time it took, not the threads' sum.
            let started = std::time::Instant::now();
            let before = PREPARING[0].load(Ordering::Relaxed);
            // Noted now: nothing reads the notes before the jobs are done.
            for (shadow, from, to) in &self.refreshes {
                Self::brought(&mut self.current, shadow, *from, *to);
            }
            let jobs: Vec<Box<dyn FnOnce() + Send>> = self
                .refreshes
                .drain(..)
                .map(|(shadow, from, to)| Box::new(move || Self::update(&shadow, from, to)) as Box<dyn FnOnce() + Send>)
                .collect();
            crate::shadow::wide_run(jobs);
            let summed = PREPARING[0].load(Ordering::Relaxed).saturating_sub(before);
            let over = summed.saturating_sub(started.elapsed().as_micros() as u64);
            PREPARING[0].fetch_sub(over, Ordering::Relaxed);
            crate::framelog::sub(crate::framelog::T::CopyUs, over);
        } else {
            for (shadow, from, to) in self.refreshes.drain(..) {
                Self::update(&shadow, from, to);
                Self::brought(&mut self.current, &shadow, from, to);
            }
        }
        if ahead {
            crate::watch::ask_ahead_done();
        }
    }
}
