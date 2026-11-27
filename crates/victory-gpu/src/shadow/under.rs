//! Threads kept for the title's bytes under flights' writes
//! (`VICTORY_UNDER_WIDE=1`; off by default here): the pages of
//! [`super::Shadow::merge_under_writes`] compared on them, and the pairs
//! that `crate::host::send` makes (`VICTORY_UNDER_THREADS`, 6 with the
//! calling one).
//!
//! Not the threads of `super::wide`: the thread that waits there takes
//! whatever work comes, and a landing's takes the copy's lock that the
//! compare holds.
//!
//! The pool is VI-BR-PORT's `land::Pool` (8bb5142d, b6202840), without its
//! timeline spans and without asking for the fast cores.

use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

/// Whether the pages of `Shadow::merge_under_writes` (copies in the card's
/// memory) are compared on several threads, and `crate::host::send` makes
/// its pairs on them (`VICTORY_UNDER_WIDE=1`; unset: on the submitting
/// thread, as before).
pub(crate) fn under_wide() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_UNDER_WIDE").is_some_and(|v| v == "1"))
}

/// Threads kept, and the work they are at.
struct Pool {
    state: Mutex<State>,
    more: Condvar,
    idle: Condvar,
    /// One batch at a time.
    turn: Mutex<()>,
    threads: usize,
}

struct State {
    /// What to do with each number below `count`.
    work: Option<Work>,
    next: usize,
    count: usize,
    /// Helpers inside a call.
    running: usize,
    panicked: bool,
}

#[derive(Clone, Copy)]
struct Work(*const (dyn Fn(usize) + Sync));

// SAFETY: the closure may be called from any thread (`Sync`), and
// `Pool::each` keeps it alive until no helper is inside a call.
unsafe impl Send for Work {}

impl Pool {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn new(name: &'static str, threads: usize) -> &'static Pool {
        let pool: &'static Pool = Box::leak(Box::new(Pool {
            state: Mutex::new(State {
                work: None,
                next: 0,
                count: 0,
                running: 0,
                panicked: false,
            }),
            more: Condvar::new(),
            idle: Condvar::new(),
            turn: Mutex::new(()),
            threads,
        }));
        for i in 1..threads {
            // (A helper that is not there is only missed.)
            let _ = std::thread::Builder::new().name(format!("{name}-{i}")).spawn(move || pool.help());
        }
        pool
    }

    fn help(&self) {
        let mut state = self.state();
        loop {
            let Some((work, i)) = take(&mut state) else {
                state = self.more.wait(state).unwrap_or_else(|e| e.into_inner());
                continue;
            };
            state.running += 1;
            drop(state);
            // SAFETY: counted in `running`, so `each` has not returned.
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*work.0)(i) }));
            state = self.state();
            state.running -= 1;
            state.panicked |= ran.is_err();
            if state.running == 0 {
                self.idle.notify_all();
            }
        }
    }

    /// Calls `f` with every number below `count`, on the pool's threads and
    /// this one, and returns when every call has.
    fn each(&self, count: usize, f: &(dyn Fn(usize) + Sync)) {
        if count < 2 || self.threads < 2 {
            (0..count).for_each(f);
            return;
        }
        let _turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the lifetime is left out only until this call returns, and
        // it does not before no helper is inside `f` and none can enter.
        let work = Work(unsafe { std::mem::transmute::<*const (dyn Fn(usize) + Sync + '_), *const (dyn Fn(usize) + Sync + 'static)>(f) });
        let mut state = self.state();
        (state.work, state.next, state.count, state.panicked) = (Some(work), 0, count, false);
        drop(state);
        if count > self.threads {
            self.more.notify_all();
        } else {
            (1..count).for_each(|_| self.more.notify_one());
        }
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                let Some((_, i)) = take(&mut self.state()) else { break };
                f(i);
            }
        }));
        let mut state = self.state();
        // (After a panic here: nothing more is begun.)
        state.work = None;
        while state.running > 0 {
            state = self.idle.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        let panicked = state.panicked;
        drop(state);
        if let Err(panic) = ran {
            std::panic::resume_unwind(panic);
        }
        assert!(!panicked, "a job of the title's bytes under flights' writes panicked");
    }
}

/// The next number of the batch, if there is one.
fn take(state: &mut State) -> Option<(Work, usize)> {
    let work = state.work?;
    if state.next >= state.count {
        return None;
    }
    state.next += 1;
    Some((work, state.next - 1))
}

/// [`Pool::each`] on the threads kept for the title's bytes under flights'
/// writes (`VICTORY_UNDER_THREADS`, 6 with this one).
pub(crate) fn each_under(count: usize, f: &(dyn Fn(usize) + Sync)) {
    static POOL: OnceLock<&'static Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::env::var("VICTORY_UNDER_THREADS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(6usize);
        Pool::new("victory-under", threads.clamp(1, 16))
    })
    .each(count, f);
}

#[cfg(test)]
mod tests {
    use super::each_under;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Every number below the count once, from this thread and the kept
    /// ones, batch after batch.
    #[test]
    fn each_under_calls_every_number_once() {
        for count in [0usize, 1, 2, 7, 64, 1000] {
            let seen: Vec<AtomicUsize> = (0..count).map(|_| AtomicUsize::new(0)).collect();
            each_under(count, &|i| {
                seen[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(seen.iter().all(|s| s.load(Ordering::Relaxed) == 1), "{count}");
        }
    }
}
