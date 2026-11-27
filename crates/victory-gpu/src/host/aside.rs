//! Work for a list that a thread of ours does while the title records on:
//! the walk of the pass over the heap's buffer views (`crate::draw`,
//! `heap_buffer_pass`), which takes 12 to 17 ms of one recording thread at
//! the frame's first list that needs it in avatarmanz's base, with the
//! submission of that list, and of those behind it, waiting.
//! `VICTORY_HEAP_PASS_ASIDE=1`; unset (or anything else), done where it is
//! needed, as before.
//!
//! From avatarmanz/VIctory-7900XTX f6721303 (on there unless `=0`).
//!
//! What it finds are refreshes of host copies, which are steps of the list
//! for its submission, and what it makes are descriptors, which the card
//! reads when the list runs: both are there when the list is closed
//! (`HostList::close` waits), and the refreshes stand at the place in the
//! list's steps where the pass was begun ([`splice_once`]).

use super::*;

/// A host copy, and the title memory of it (from, to).
pub(crate) type Refresh = (std::sync::Arc<crate::shadow::Shadow>, u64, u64);

/// What is being found, until it is.
pub(crate) struct Coming(Mutex<Option<Vec<Refresh>>>, std::sync::Condvar);

/// Since the last frame line: microseconds lists waited at their close.
pub(crate) static WAITED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Coming {
    /// What was found.
    pub(crate) fn wait(&self) -> Vec<Refresh> {
        let began = std::time::Instant::now();
        let mut found = self.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(found) = found.take() {
                WAITED.fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
                return found;
            }
            found = self.1.wait(found).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn found(&self, what: Vec<Refresh>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(what);
        self.1.notify_all();
    }
}

/// Whether the walk is done beside the recording (`VICTORY_HEAP_PASS_ASIDE=1`;
/// off unless set, until a run of ours has carried it).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_PASS_ASIDE").is_some_and(|v| v == "1"))
}

type Job = Box<dyn FnOnce() + Send>;

/// `work` begins on the thread for it, one after another; here and now if
/// there is no such thread.
pub(crate) fn begin(work: impl FnOnce() -> Vec<Refresh> + Send + 'static) -> std::sync::Arc<Coming> {
    static THREAD: OnceLock<Option<Mutex<std::sync::mpsc::Sender<Job>>>> = OnceLock::new();
    let coming = std::sync::Arc::new(Coming(Mutex::new(None), std::sync::Condvar::new()));
    let told = coming.clone();
    let job: Job = Box::new(move || {
        // (However it ends, the list that waits goes on.)
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_default();
        told.found(found);
    });
    let thread = THREAD.get_or_init(|| {
        let (send, jobs) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("victory-aside".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .ok()
            .map(|_| Mutex::new(send))
    });
    match thread {
        Some(send) => {
            if let Err(job) = send.lock().unwrap_or_else(|e| e.into_inner()).send(job) {
                (job.0)();
            }
        }
        None => job(),
    }
    coming
}

/// Puts `found` into `steps` at `at` as if each had been noted there, one
/// after another, by a list that notes a key once until a step that
/// `clears` comes (`HostList::defer_refresh`, whose set a title copy or a
/// run of ours empties): one of `found` whose key is noted before `at`,
/// with no such step between, is left out; and one noted after `at` (by the
/// recording, while the walk went on) with no such step between, which the
/// list would have left out had the walk been done at `at`, goes.
pub(crate) fn splice_once<T, K: Eq + std::hash::Hash>(
    steps: &mut Vec<T>,
    at: usize,
    found: Vec<T>,
    key: impl Fn(&T) -> Option<K>,
    clears: impl Fn(&T) -> bool,
) {
    let at = at.min(steps.len());
    let noted: std::collections::HashSet<K> =
        steps[..at].iter().rev().take_while(|s| !clears(s)).filter_map(&key).collect();
    let mut added: std::collections::HashSet<K> = std::collections::HashSet::new();
    let mut kept = Vec::with_capacity(found.len());
    for step in found {
        match key(&step) {
            Some(k) if noted.contains(&k) || added.contains(&k) => {}
            Some(k) => {
                added.insert(k);
                kept.push(step);
            }
            None => kept.push(step),
        }
    }
    let end = steps[at..].iter().position(&clears).map_or(steps.len(), |p| at + p);
    let after: Vec<T> = steps
        .drain(at..end)
        .filter(|s| key(s).is_none_or(|k| !added.contains(&k)))
        .collect();
    steps.splice(at..at, kept.into_iter().chain(after));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_found_is_waited_for_and_a_panic_lets_the_list_go_on() {
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let coming = begin(move || {
            let _ = wait.recv();
            Vec::new()
        });
        assert!(coming.0.lock().unwrap().is_none());
        go.send(()).unwrap();
        assert!(coming.wait().is_empty());
        assert!(begin(|| panic!("a panic of the work")).wait().is_empty());
        assert!(begin(Vec::new).wait().is_empty());
    }

    /// `R(k)` a refresh of key `k`, `C` a step that empties the noted set.
    #[derive(Debug, PartialEq, Clone, Copy)]
    enum S {
        R(u32),
        C,
        F,
    }

    fn splice(steps: &[S], at: usize, found: &[S]) -> Vec<S> {
        let mut steps = steps.to_vec();
        let key = |s: &S| match *s {
            S::R(k) => Some(k),
            _ => None,
        };
        splice_once(&mut steps, at, found.to_vec(), key, |s| *s == S::C);
        steps
    }

    #[test]
    fn found_refreshes_stand_where_the_walk_began_noted_once_until_a_copy() {
        use S::*;
        // Into an empty list, and at its end.
        assert_eq!(splice(&[], 0, &[R(1), R(2)]), [R(1), R(2)]);
        assert_eq!(splice(&[F, R(5)], 2, &[R(1)]), [F, R(5), R(1)]);
        // At the place, not at the end.
        assert_eq!(splice(&[F, C, F], 1, &[R(1)]), [F, R(1), C, F]);
        // Noted before the place, no copy between: left out.
        assert_eq!(splice(&[R(1), F], 2, &[R(1), R(2)]), [R(1), F, R(2)]);
        // A copy between: noted again.
        assert_eq!(splice(&[R(1), C], 2, &[R(1)]), [R(1), C, R(1)]);
        // Noted after the place by the recording, no copy between: it goes.
        assert_eq!(splice(&[F, R(1), F, R(3)], 1, &[R(1)]), [F, R(1), F, R(3)]);
        assert_eq!(splice(&[F, F, R(1)], 1, &[R(1), R(2)]), [F, R(1), R(2), F]);
        // After a copy it stays.
        assert_eq!(splice(&[F, C, R(1)], 1, &[R(1)]), [F, R(1), C, R(1)]);
        // A place past the end (the list was emptied meanwhile): the end.
        assert_eq!(splice(&[F], 9, &[R(1)]), [F, R(1)]);
        // Nothing found: as it was.
        assert_eq!(splice(&[R(1), C, R(2)], 1, &[]), [R(1), C, R(2)]);
        // Twice in what was found: once.
        assert_eq!(splice(&[], 0, &[R(1), R(1)]), [R(1)]);
    }
}
