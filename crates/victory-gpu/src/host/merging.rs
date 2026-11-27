//! Title submissions sent ahead (`late`) held back and sent to the card as
//! one host submission (`VICTORY_MERGE_SUBMISSIONS`, [`crate::merge::level`];
//! off by default). The decisions are in `crate::merge`; here are the held
//! lists, the calls that send them, and the thread that sends them on time.
//! Ported from AnyXboxX PR #337 (by Skyfrye), in a module of its own so
//! that `host.rs` keeps only the hooks.

use super::*;

/// The title submissions held back to go to the card as one
/// ([`crate::merge::level`]), in the order the title gave them: their host
/// lists, the order those run in, where each submission after the first
/// begins in it, and what they touch ([`crate::merge::Plan`]). Taken from
/// before a send until the send is on the line of flights, so that sends go
/// in the order their submissions came and nothing that must follow them
/// (a signal, a present) goes ahead.
pub(super) struct Merging {
    lists: Vec<crate::list::Detached>,
    order: Vec<usize>,
    joins: Vec<usize>,
    /// Whether what the first submission touches is not in `plan` yet: it is
    /// read only once a second one is offered or a wait asks
    /// ([`Host::merge_learn`]), so a submission sent alone costs nothing more.
    unread: bool,
    plan: crate::merge::Plan,
    /// When the first of them was held ([`crate::merge::window`]).
    since: Option<std::time::Instant>,
}

// SAFETY: host lists nothing records into (as in [`Line`]); touched under the
// lock.
unsafe impl Send for Merging {}

static MERGING: Mutex<Merging> = Mutex::new(Merging {
    lists: Vec::new(),
    order: Vec::new(),
    joins: Vec::new(),
    unread: false,
    plan: crate::merge::Plan::new(),
    since: None,
});

/// Whether [`MERGING`] holds anything, and whether a signal is carried behind
/// it: read without the lock by every call that would send it. Both stay set
/// until a send is on the line of flights.
static MERGE_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MERGE_CARRIES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Told when a first submission is held, for the thread that sends them on
/// time ([`merge_timer`]).
static MERGE_MOVED: std::sync::Condvar = std::sync::Condvar::new();

fn merging() -> std::sync::MutexGuard<'static, Merging> {
    MERGING.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the steps of `runs` read and write at submission, for
/// [`crate::merge::Plan`]: the ranges [`blocked_by`] looks at (and the title
/// memory steps of ours that write host memory read), what they bring to
/// host copies and staging buffers, what their shaders and read-backs may
/// write, whether code of ours runs, and their bottom-of-pipe writes.
fn touches_of(runs: &[&HostList]) -> crate::merge::Touches {
    let mut touches = crate::merge::Touches {
        lists: runs.len(),
        ..Default::default()
    };
    for list in runs {
        let deferred = list.deferred();
        for step in &deferred.copies {
            match step {
                // A copy also recorded on the card (`readcopy`): as a copy,
                // wherever the submission leaves it.
                &Step::Copy(dst, src, len) | &Step::CardCopy(dst, src, len, _) => {
                    touches.read.push((src, src + len));
                    touches.read.push((dst, dst + len));
                    touches.written.push((dst, dst + len));
                }
                // Constants too: `blocked_by` looks at every fill.
                &Step::Fill(_, src, len, _) => touches.read.push((src, src + len)),
                Step::Refresh(_, address, len) => touches.brought.push((*address, *address + *len)),
                Step::Texture(staging) => {
                    let (at, len) = staging.memory();
                    touches.read.push((at, at + len));
                    touches.brought.push((at, at + len));
                }
                Step::GpuWrite(_, address, len) => touches.shader.push((*address, *address + *len)),
                Step::Run(..) => touches.code = true,
                &Step::Host(_, (from, to)) => {
                    if from < to {
                        touches.read.push((from, to));
                    }
                }
            }
        }
        // What lands in title memory once the list has run.
        touches.shader.extend(deferred.texture_backs.iter().map(crate::texback::Back::range));
        for &(_, at, [row, rows, pitch]) in &deferred.backs {
            if rows > 0 {
                touches.shader.push((at, at + (rows - 1) * pitch + row));
            }
        }
        touches.writes.extend(deferred.writes.iter().map(|&(at, what, bytes)| {
            let value = match what {
                crate::queue::Written::Value(v) => Some(v),
                _ => None,
            };
            (at, value, bytes)
        }));
    }
    touches
}

/// Sends the title submissions held back to be merged, if any, for `why`:
/// a call that the card's work before it must be done or under way for.
/// Not from inside a submission.
pub(crate) fn merge_send(why: crate::merge::Why) {
    if !MERGE_HELD.load(Ordering::Acquire) {
        return;
    }
    let Some(host) = host() else {
        return;
    };
    let direct = {
        let mut held = merging();
        host.merge_send_held(&mut held, why)
    };
    if direct {
        merge_poke();
    }
}

/// A queue signals `fence` with `value`, and holds nothing itself: with
/// title submissions held back to be merged they go to the card first, or
/// (`VICTORY_MERGE_SUBMISSIONS=2`, dangerous: [`crate::merge::level`]) the
/// signal is carried behind them and set when they have run. Whether it was
/// carried; if not the caller signals as before.
pub(crate) fn merge_signal(fence: usize, value: u64) -> bool {
    if !MERGE_HELD.load(Ordering::Acquire) {
        return false;
    }
    let Some(host) = host() else {
        return false;
    };
    let place = crate::fence::value_place(fence as crate::object::This);
    let mut held = merging();
    match held.plan.signal(crate::merge::level(), fence, value, place) {
        crate::merge::Signal::Alone => false,
        crate::merge::Signal::Carried => {
            MERGE_CARRIES.store(true, Ordering::Release);
            crate::merge::carried();
            true
        }
        crate::merge::Signal::Send => {
            let direct = host.merge_send_held(&mut held, crate::merge::Why::Signal);
            drop(held);
            if direct {
                merge_poke();
            }
            false
        }
    }
}

/// The title asks about `fence` (its value, an event for a value it has not
/// reached), or a queue stands at a wait for it: a signal of it carried
/// behind held submissions (`VICTORY_MERGE_SUBMISSIONS=2`) goes to the card
/// with them now, so that it is set once they have run.
pub(crate) fn merge_fence_asked(fence: usize) {
    if !MERGE_CARRIES.load(Ordering::Acquire) {
        return;
    }
    let Some(host) = host() else {
        return;
    };
    let direct = {
        let mut held = merging();
        if !held.plan.carries(fence) {
            return;
        }
        host.merge_send_held(&mut held, crate::merge::Why::Fence)
    };
    if direct {
        merge_poke();
    }
}

/// A queue stands at `wait`, which memory does not meet now: with title
/// submissions held back to be merged, whether one of their bottom-of-pipe
/// writes meets it (the part behind the wait then joins them, behind that
/// write). If they may write the value in a way not known here they go to
/// the card first. Asked before [`late_met`].
pub(crate) fn merge_wait(wait: &crate::list::ValueWait) -> bool {
    if !MERGE_HELD.load(Ordering::Acquire) {
        return false;
    }
    let Some(host) = host() else {
        return false;
    };
    let mut held = merging();
    if held.plan.is_empty() {
        return false;
    }
    host.merge_learn(&mut held);
    match held.plan.value_wait(wait.place(), |value| wait.holds_for(value)) {
        crate::merge::Wait::Alone => false,
        crate::merge::Wait::Met => {
            crate::merge::met();
            true
        }
        crate::merge::Wait::Send => {
            let direct = host.merge_send_held(&mut held, crate::merge::Why::Value);
            drop(held);
            if direct {
                merge_poke();
            }
            false
        }
    }
}

/// After a send that set a fence itself (its flight had landed already), or
/// one of the timer's: the helper thread looks at what queues hold, as after
/// a landing.
fn merge_poke() {
    line().poke = true;
    LINE_MOVED.notify_all();
}

/// Starts the thread that sends held submissions on time, once.
fn merge_thread() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let most = crate::merge::most();
        match std::thread::Builder::new().name("victory-merge".into()).spawn(merge_timer) {
            Ok(_) => tracing::info!(
                level = crate::merge::level(),
                window_us = crate::merge::window().as_micros() as u64,
                lists = most.lists,
                submissions = most.members,
                "title submissions are held back and sent to the card together (VICTORY_MERGE_SUBMISSIONS)"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                "no thread to send held title submissions on time: they wait for the next call that sends them"
            ),
        }
    });
}

/// The thread that sends held submissions once the first of them was held
/// [`crate::merge::window`] ago: a title that polls memory for what they
/// write, with no call of ours between, sees it that much later at most.
fn merge_timer() {
    let Some(host) = host() else {
        return;
    };
    let window = crate::merge::window();
    let mut held = merging();
    loop {
        let Some(since) = held.since else {
            held = MERGE_MOVED.wait(held).unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let age = since.elapsed();
        if age < window {
            held = MERGE_MOVED.wait_timeout(held, window - age).unwrap_or_else(|e| e.into_inner()).0;
            continue;
        }
        host.merge_send_held(&mut held, crate::merge::Why::Timer);
        // What was sent may be what a queue stands at (`late_met`).
        drop(held);
        merge_poke();
        held = merging();
    }
}

/// What the first list of a title submission merged behind others waits
/// for ([`crate::merge::level`]): every shader write before it (an
/// unordered-access barrier on no resource) and every use of memory before
/// it (an aliasing barrier between no resources), as the end of a host
/// submission made the card wait before. Lists of one `ExecuteCommandLists`
/// may overlap on the card; lists of two calls do not.
pub(super) fn join_barriers() -> [D3D12_RESOURCE_BARRIER; 2] {
    [
        D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: std::mem::ManuallyDrop::new(None),
                }),
            },
        },
        D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Aliasing: std::mem::ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                    pResourceBefore: std::mem::ManuallyDrop::new(None),
                    pResourceAfter: std::mem::ManuallyDrop::new(None),
                }),
            },
        },
    ]
}

impl Host {
    /// [`Host::execute_late`] with [`crate::merge::level`]: the title
    /// submission joins those held back since the last send, unless it may
    /// not ([`crate::merge::Plan::refuses`]: they are sent first, and it is
    /// the first of the next ones). A thread of ours sends them if nothing
    /// else has within [`crate::merge::window`].
    pub(super) fn merge_offer(&self, lists: Vec<crate::list::Detached>, order: Vec<usize>) {
        if order.is_empty() {
            crate::list::recycle(lists, &order);
            return;
        }
        late_threads();
        merge_thread();
        crate::merge::offered();
        // `VICTORY_LATE_WAIT_WRITES=0`: a submission with bottom-of-pipe
        // writes is settled as soon as it is sent, as before.
        let told = !late_writes() && order.iter().any(|&i| !lists[i].host().deferred().writes.is_empty());
        let mut direct = false;
        let mut held = merging();
        // What it touches, read only when something is held already.
        let touches = if held.plan.is_empty() {
            None
        } else {
            self.merge_learn(&mut held);
            let touches = {
                let runs: Vec<&HostList> = order.iter().map(|&i| lists[i].host()).collect();
                touches_of(&runs)
            };
            // On its own behind them it would wait for flights in the air
            // whose shaders write what it reads (`late_blocked`): with them,
            // they would wait too. They go now, as they would have.
            let why = held.plan.refuses(&touches, crate::merge::most()).or_else(|| {
                let line = line();
                touches
                    .read
                    .iter()
                    .any(|&(from, to)| super::touches(&line.written, from, to))
                    .then_some(crate::merge::Why::Reads)
            });
            if let Some(why) = why {
                direct |= self.merge_send_held(&mut held, why);
            }
            Some(touches)
        };
        {
            let Merging { lists: all, order: ran, joins, unread, plan, since } = &mut *held;
            if plan.is_empty() {
                *since = Some(std::time::Instant::now());
                match touches {
                    Some(touches) => plan.take(touches),
                    None => {
                        plan.count(order.len());
                        *unread = true;
                    }
                }
            } else {
                joins.push(ran.len());
                if let Some(touches) = touches {
                    plan.take(touches);
                }
            }
            let base = all.len();
            ran.extend(order.iter().map(|&i| base + i));
            all.extend(lists);
        }
        if told {
            direct |= self.merge_send_held(&mut held, crate::merge::Why::Writes);
        } else if held.plan.lists() >= crate::merge::most().lists {
            direct |= self.merge_send_held(&mut held, crate::merge::Why::Most);
        } else {
            MERGE_HELD.store(true, Ordering::Release);
        }
        drop(held);
        MERGE_MOVED.notify_one();
        if direct {
            merge_poke();
        }
    }

    /// What the first held submission touches, into the plan, if it is not
    /// there yet ([`Merging::unread`]).
    fn merge_learn(&self, held: &mut Merging) {
        if !held.unread {
            return;
        }
        held.unread = false;
        let touches = {
            let runs: Vec<&HostList> = held.order.iter().map(|&i| held.lists[i].host()).collect();
            touches_of(&runs)
        };
        held.plan.learn(touches);
    }

    /// Sends what `held` holds as one submission ([`Host::send_late`]), for
    /// `why`, and then the signals carried behind it: each behind its flight,
    /// or set now if the flight has landed already. Whether a fence was set
    /// here (the caller lets the helper thread look at what queues hold).
    /// `held` is taken until the flight is on the line.
    fn merge_send_held(&self, held: &mut Merging, why: crate::merge::Why) -> bool {
        if held.plan.is_empty() {
            return false;
        }
        let lists = std::mem::take(&mut held.lists);
        let order = std::mem::take(&mut held.order);
        let joins = std::mem::take(&mut held.joins);
        let signals = held.plan.clear();
        held.unread = false;
        held.since = None;
        crate::merge::sent(why);
        self.send_late(lists, order, &joins);
        let mut direct = false;
        for (fence, value) in signals {
            if !late_signal(fence as crate::object::This, value) {
                crate::fence::set(fence as crate::object::This, value);
                direct = true;
            }
        }
        // Only now: a call that found these set waited for the lock, and its
        // signal or present goes behind this flight.
        MERGE_CARRIES.store(false, Ordering::Release);
        MERGE_HELD.store(false, Ordering::Release);
        direct
    }
}

#[cfg(test)]
mod tests {
    /// What merging title submissions needs of a list is read from its
    /// submission-time steps ([`super::touches_of`]): both ends of a buffer
    /// copy, fills of upload memory (constants too), code of ours,
    /// bottom-of-pipe writes.
    #[test]
    fn a_lists_steps_say_what_its_preparing_touches() {
        let Some(host) = super::host() else {
            return;
        };
        let list = host.new_list().unwrap();
        let (mut dst, src, mut upload, mut value) = ([0u8; 64], [1u8; 64], [0u8; 64], [0u32; 2]);
        list.defer_copy(dst.as_mut_ptr(), src.as_ptr(), 16);
        list.defer_fill(upload.as_mut_ptr(), src[16..].as_ptr(), 8);
        list.defer_constants(upload[32..].as_mut_ptr(), src[32..].as_ptr(), 8, (0, 0));
        list.defer_write(value.as_mut_ptr().cast(), crate::queue::Written::Value(5), 4);
        list.defer_write(value.as_mut_ptr().cast(), crate::queue::Written::GpuTimestamp, 8);
        let touches = super::touches_of(&[&list]);
        let (d, s, v) = (dst.as_ptr() as usize, src.as_ptr() as usize, value.as_ptr() as usize);
        assert_eq!(touches.lists, 1);
        assert_eq!(touches.read, [(s, s + 16), (d, d + 16), (s + 16, s + 24), (s + 32, s + 40)]);
        assert_eq!(touches.written, [(d, d + 16)]);
        assert_eq!(touches.writes, [(v, Some(5), 4), (v, None, 8)]);
        assert!(!touches.code && touches.shader.is_empty() && touches.brought.is_empty());
        list.defer_host_read(s, 4, || {});
        assert_eq!(super::touches_of(&[&list]).read.last(), Some(&(s, s + 4)));
        list.close();
    }

    /// The first list of a title submission merged behind others has a
    /// prelude that waits for everything before it, even with nothing to
    /// move, and the card runs the two as one submission.
    #[test]
    fn the_first_list_of_a_merged_submission_waits_for_those_before() {
        let Some(host) = super::host() else {
            return;
        };
        let (first, second) = (host.new_list().unwrap(), host.new_list().unwrap());
        first.close();
        second.close();
        assert!(second.prelude(false).is_none(), "nothing to move: no prelude");
        assert!(second.prelude(true).is_some(), "merged behind another: a prelude with the barriers");
        let _ = host.submit(&[&first, &second], false, &[1]);
        // SAFETY: a plain query.
        assert!(unsafe { host.device.GetDeviceRemovedReason() }.is_ok());
    }
}
