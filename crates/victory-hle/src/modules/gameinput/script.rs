//! `VICTORY_KEYS`: keys pressed on a script, for runs nobody plays by hand
//! (diagnostics builds). `<seconds>:<key>` presses a key that long after
//! input starts; `<seconds>:<key>*<n>/<gap>` presses it `n` times, `gap`
//! seconds apart (10 when only `*<n>` is given). `<key>~<hold>` holds the key
//! down exactly `hold` seconds (0.2 to 60), whatever the title reads, to walk
//! or turn: `300:W~3` holds W for three seconds at 300 s, `300:W~3*4/10` does
//! so four times; its line in the log says how many reads saw it down (0: the
//! title did not read its input then). A key is a letter, a digit, or `Enter`,
//! `Esc`, `Tab`, `Back`, `Up`, `Down`, `Left`, `Right`.
//!
//! A press is held until the title has read the input with the key down
//! [`Script::need`] times (`VICTORY_KEYS_READS`, 1 by default: while loading
//! the title reads its input seldom, and one read with the key down was
//! enough in the first unattended run), and at least [`MIN_HOLD`]: at one to three frames a
//! second a fixed 0.2 s fell between two of the title's reads and was never
//! seen. A press nobody reads is let go after [`GIVE_UP`], and said so. The
//! next press of the same key waits until the title has read it up once
//! (or [`GIVE_UP`]), so that each press is a press. Each press leaves a line
//! in the log: when it went down, how many reads saw it, and how long it was
//! held.
//!
//! `VICTORY_KEYS_FILE=<path>` (off when unset): keys a runner asks for now.
//! Each non-empty line is `<key>` or `<key>~<hold>` (no times: pressed when
//! the file is taken), at most [`FILE_MAX_LINES`] lines and
//! [`FILE_MAX_BYTES`]; the presses go through the script above (same reads,
//! same waits, same log lines) and queue behind the presses still down.
//! The protocol, looked at once a second from the input poll:
//! 1. The runner writes `<path>.tmp` and renames it to `<path>` (the next
//!    file only once `<path>` is gone: a rename over it replaces it).
//! 2. We rename `<path>` to `<path>.taken` (the claim: each file is taken
//!    once; a failed rename is tried again a second later), then read it.
//! 3. We rename it to `<path>.done`, or, if any line is not understood or it
//!    is too long, press nothing and rename it to `<path>.rejected` (both
//!    overwritten by the next file).
//!
//! How the runner detects the prompt: the layer does NOT detect prompts. The
//! runner decides from its own evidence, e.g. its 1 s captures showing the
//! prompt, compared over consecutive captures so that a still loading screen
//! is not taken for one. The first visible frame does NOT mean a prompt. One
//! key per prompt, and never during a scene.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The shortest a press is held, whatever the reads.
pub(super) const MIN_HOLD: Duration = Duration::from_millis(200);
/// A press no read has seen after this long is let go.
pub(super) const GIVE_UP: Duration = Duration::from_secs(15);
/// Reads closer together than this count as one (the title reads every
/// device in the same frame).
const SAME_READ_US: u64 = 5_000;
/// A `VICTORY_KEYS_FILE` longer than this is rejected.
pub(super) const FILE_MAX_BYTES: usize = 1024;
pub(super) const FILE_MAX_LINES: usize = 16;

/// One scripted press of a key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Press {
    /// When, after input starts.
    pub at: Duration,
    /// The virtual key.
    pub vk: u8,
    /// `~<hold>`: held down exactly this long, whatever the reads; `None`:
    /// until read (see the module).
    pub hold: Option<Duration>,
}

/// What happened to a press, for the log.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Note {
    /// The key went down this late after input starts.
    Down { vk: u8, at: Duration },
    /// Let go after `reads` reads saw it down, held `held`.
    Seen {
        vk: u8,
        at: Duration,
        reads: u32,
        held: Duration,
    },
    /// A key with `~<hold>` let go after its hold; `reads` saw it down.
    Held {
        vk: u8,
        at: Duration,
        reads: u32,
        held: Duration,
    },
    /// Let go after [`GIVE_UP`] with fewer reads than asked for (`reads`,
    /// possibly none).
    Unread {
        vk: u8,
        at: Duration,
        reads: u32,
        held: Duration,
    },
}

/// Parses `VICTORY_KEYS`: the presses in time order, and the entries that
/// are not understood (left out).
pub(super) fn parse(list: &str) -> (Vec<Press>, Vec<String>) {
    let mut presses = Vec::new();
    let mut bad = Vec::new();
    for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match entry_presses(entry) {
            Some(found) => presses.extend(found),
            None => bad.push(entry.to_owned()),
        }
    }
    presses.sort_by_key(|p| p.at);
    (presses, bad)
}

fn entry_presses(entry: &str) -> Option<Vec<Press>> {
    let (at, rest) = entry.split_once(':')?;
    let at = seconds(at)?;
    let (key, repeat) = match rest.split_once('*') {
        Some((key, repeat)) => (key, Some(repeat)),
        None => (rest, None),
    };
    let (vk, hold) = key_hold(key)?;
    let (times, gap) = match repeat {
        None => (1, Duration::ZERO),
        Some(repeat) => {
            let (times, gap) = match repeat.split_once('/') {
                Some((times, gap)) => (times, seconds(gap)?),
                None => (repeat, Duration::from_secs(10)),
            };
            let times: u32 = times.trim().parse().ok()?;
            // A gap shorter than a held press would run presses together.
            if !(1..=100).contains(&times) || (times > 1 && gap < hold.unwrap_or(MIN_HOLD) + MIN_HOLD) {
                return None;
            }
            (times, gap)
        }
    };
    Some(
        (0..times)
            .map(|i| Press {
                at: at + gap * i,
                vk,
                hold,
            })
            .collect(),
    )
}

/// `<key>` or `<key>~<hold>`.
fn key_hold(key: &str) -> Option<(u8, Option<Duration>)> {
    let (key, hold) = match key.split_once('~') {
        Some((key, hold)) => (key, Some(seconds(hold)?)),
        None => (key, None),
    };
    // Held at least the shortest hold, at most a minute.
    if hold.is_some_and(|h| h < MIN_HOLD || h > Duration::from_secs(60)) {
        return None;
    }
    Some((key_code(key.trim())?, hold))
}

/// Parses a `VICTORY_KEYS_FILE`: its presses (pressed when added, see
/// [`Script::add_now`]), or, if anything is wrong, what (nothing pressed).
pub(super) fn parse_file(text: &str) -> Result<Vec<Press>, Vec<String>> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if text.len() > FILE_MAX_BYTES || lines.len() > FILE_MAX_LINES {
        return Err(vec![format!("longer than {FILE_MAX_BYTES} bytes or {FILE_MAX_LINES} lines")]);
    }
    let mut presses = Vec::new();
    let mut bad = Vec::new();
    for line in lines {
        match key_hold(line) {
            Some((vk, hold)) => presses.push(Press { at: Duration::ZERO, vk, hold }),
            None => bad.push(line.to_owned()),
        }
    }
    match (presses.is_empty(), bad.is_empty()) {
        (true, true) => Err(vec!["no keys".to_owned()]),
        (_, true) => Ok(presses),
        _ => Err(bad),
    }
}

/// Takes the `VICTORY_KEYS_FILE` at `path` by the protocol of the module:
/// `Ok(None)` when there is none.
pub(super) fn take_file(path: &Path) -> io::Result<Option<Result<Vec<Press>, Vec<String>>>> {
    let beside = |suffix: &str| {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        PathBuf::from(p)
    };
    let taken = beside(".taken");
    match std::fs::rename(path, &taken) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        claim => claim?,
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&taken)?
        .take(FILE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let parsed = match std::str::from_utf8(&bytes) {
        Ok(text) => parse_file(text),
        Err(_) => Err(vec!["not UTF-8".to_owned()]),
    };
    // Where it is left is for the runner to look at; a failure changes nothing.
    let _ = std::fs::rename(&taken, beside(if parsed.is_ok() { ".done" } else { ".rejected" }));
    Ok(Some(parsed))
}

fn seconds(text: &str) -> Option<Duration> {
    Duration::try_from_secs_f64(text.trim().parse().ok()?).ok()
}

/// The virtual key a script names.
pub(super) fn key_code(key: &str) -> Option<u8> {
    Some(match key {
        "Enter" => 0x0D,
        "Esc" => 0x1B,
        "Tab" => 0x09,
        "Back" => 0x08,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        k if k.len() == 1 && k.as_bytes()[0].is_ascii_alphanumeric() => {
            k.as_bytes()[0].to_ascii_uppercase()
        }
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Waiting,
    /// Down since this time after input starts, and since this reading
    /// timestamp (the hub's clock, microseconds).
    Down {
        since: Duration,
        since_us: u64,
    },
    Done,
}

/// The presses of a script and where each one is.
#[derive(Debug)]
pub(super) struct Script {
    presses: Vec<(Press, State, u32)>,
    /// Reads that must see a press before it is let go.
    need: u32,
    /// The last read counted (hub clock), to count a frame's reads once.
    last_read_us: Option<u64>,
    /// Per key: let go at this time (script clock) and reading timestamp
    /// (hub clock), until a read has seen it up.
    released: Vec<(u8, Duration, u64)>,
}

impl Script {
    pub(super) fn new(presses: Vec<Press>, need: u32) -> Script {
        Script {
            presses: presses
                .into_iter()
                .map(|p| (p, State::Waiting, 0))
                .collect(),
            need: need.max(1),
            last_read_us: None,
            released: Vec::new(),
        }
    }

    /// Adds `press` due at `now` (its own `at` is not looked at), after
    /// the presses due by then: it waits behind those of the same key.
    pub(super) fn add_now(&mut self, press: Press, now: Duration) {
        let i = self.presses.partition_point(|(p, _, _)| p.at <= now);
        self.presses.insert(i, (Press { at: now, ..press }, State::Waiting, 0));
    }

    /// The keys down at `now` (after input starts; `now_us` on the hub's
    /// clock), with what changed.
    pub(super) fn down(&mut self, now: Duration, now_us: u64) -> (Vec<u8>, Vec<Note>) {
        let mut notes = Vec::new();
        // Let go: seen enough, or nobody reads.
        for (press, state, reads) in &mut self.presses {
            let State::Down { since, .. } = *state else {
                continue;
            };
            let held = now.saturating_sub(since);
            if let Some(hold) = press.hold {
                if held < hold {
                    continue;
                }
                notes.push(Note::Held {
                    vk: press.vk,
                    at: since,
                    reads: *reads,
                    held,
                });
            } else if *reads >= self.need && held >= MIN_HOLD {
                notes.push(Note::Seen {
                    vk: press.vk,
                    at: since,
                    reads: *reads,
                    held,
                });
            } else if held >= GIVE_UP {
                notes.push(Note::Unread {
                    vk: press.vk,
                    at: since,
                    reads: *reads,
                    held,
                });
            } else {
                continue;
            }
            *state = State::Done;
            self.released.push((press.vk, now, now_us));
        }
        // Press: due, the key not down already and seen up since its last
        // press (or that wait given up).
        for i in 0..self.presses.len() {
            let (press, state, _) = self.presses[i];
            if state != State::Waiting || now < press.at {
                continue;
            }
            let busy = self
                .presses
                .iter()
                .any(|(p, s, _)| p.vk == press.vk && matches!(s, State::Down { .. }));
            // A press with a hold of its own (`~<hold>`) has its gap set by
            // the script: it does not wait for a read to see the key up.
            let unseen = press.hold.is_none()
                && self
                    .released
                    .iter()
                    .any(|&(vk, at, _)| vk == press.vk && now.saturating_sub(at) < GIVE_UP);
            if busy || unseen {
                continue;
            }
            self.released.retain(|&(vk, _, _)| vk != press.vk);
            self.presses[i].1 = State::Down {
                since: now,
                since_us: now_us,
            };
            notes.push(Note::Down {
                vk: press.vk,
                at: now,
            });
        }
        let down = self
            .presses
            .iter()
            .filter(|(_, s, _)| matches!(s, State::Down { .. }))
            .map(|(p, _, _)| p.vk)
            .collect();
        (down, notes)
    }

    /// The title read a reading taken at `timestamp_us` (the hub's clock).
    /// `timestamp_us` is the reading's own time (a key held down is one
    /// reading the title gets again and again); `now_us`, when the title
    /// read it: reads are counted by the latter.
    pub(super) fn read(&mut self, timestamp_us: u64, now_us: u64) {
        if self
            .last_read_us
            .is_some_and(|last| now_us < last + SAME_READ_US)
        {
            return;
        }
        self.last_read_us = Some(now_us);
        for (_, state, reads) in &mut self.presses {
            if let State::Down { since_us, .. } = *state
                && timestamp_us >= since_us
            {
                *reads += 1;
            }
        }
        // A key let go before this reading was read up.
        self.released.retain(|&(_, _, at_us)| timestamp_us < at_us);
    }

    /// Every press done.
    #[cfg(test)]
    pub(super) fn finished(&self) -> bool {
        self.presses.iter().all(|(_, s, _)| *s == State::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTER: u8 = 0x0D;

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    /// The script polled every 8 ms from `from` to `to` seconds, with the
    /// title reading at `reads` (seconds); the keys seen down at each read.
    fn run(
        script: &mut Script,
        from: f64,
        to: f64,
        reads: &[f64],
    ) -> (Vec<(f64, Vec<u8>)>, Vec<Note>) {
        let mut notes = Vec::new();
        let mut seen = Vec::new();
        let mut down = Vec::new();
        let mut t = from;
        let mut next_read = reads.iter().copied().peekable();
        while t <= to {
            let (keys, n) = script.down(secs(t), (t * 1e6) as u64);
            notes.extend(n);
            down = keys;
            while next_read.peek().is_some_and(|&r| r <= t) {
                let r = next_read.next().unwrap();
                // The title gets the newest reading: this poll's.
                script.read((t * 1e6) as u64, (r * 1e6) as u64);
                seen.push((r, down.clone()));
            }
            t += 0.008;
        }
        let _ = down;
        (seen, notes)
    }

    #[test]
    fn a_press_is_held_until_the_title_has_read_it_twice() {
        let (presses, bad) = parse("1:Enter");
        assert!(bad.is_empty());
        let mut script = Script::new(presses, 2);
        // One read every 0.7 s: a fixed 0.2 s would fall between two.
        let (seen, notes) = run(&mut script, 0.0, 4.0, &[0.6, 1.3, 2.0, 2.7, 3.4]);
        let with: Vec<f64> = seen
            .iter()
            .filter(|(_, k)| k.contains(&ENTER))
            .map(|(r, _)| *r)
            .collect();
        assert_eq!(with, [1.3, 2.0], "seen down by two reads, then up");
        assert!(matches!(notes[0], Note::Down { vk: ENTER, .. }));
        let Note::Seen { reads, held, .. } = notes[1] else {
            panic!("{notes:?}")
        };
        assert_eq!(reads, 2);
        assert!(held >= secs(0.7) && held < secs(1.1), "{held:?}");
        assert!(script.finished());
    }

    #[test]
    fn a_press_nobody_reads_is_let_go_and_said_so() {
        let mut script = Script::new(parse("1:Enter").0, 2);
        let (_, notes) = run(&mut script, 0.0, 1.0 + GIVE_UP.as_secs_f64() + 0.1, &[]);
        let Note::Unread { reads, held, .. } = notes[1] else {
            panic!("{notes:?}")
        };
        assert_eq!(reads, 0);
        assert!(held >= GIVE_UP);
        assert!(script.finished());
    }

    #[test]
    fn a_fast_reader_still_sees_the_shortest_hold() {
        let mut script = Script::new(parse("1:Enter").0, 2);
        let reads: Vec<f64> = (0..200).map(|i| f64::from(i) * 0.016).collect();
        let (_, notes) = run(&mut script, 0.0, 3.0, &reads);
        let Note::Seen { held, reads, .. } = notes[1] else {
            panic!("{notes:?}")
        };
        assert!(held >= MIN_HOLD && held < MIN_HOLD + secs(0.02), "{held:?}");
        assert!(reads >= 2);
    }

    #[test]
    fn repeated_presses_are_separate_presses() {
        let (presses, bad) = parse("1:Enter*3/2");
        assert!(bad.is_empty());
        assert_eq!(
            presses.iter().map(|p| p.at).collect::<Vec<_>>(),
            [secs(1.0), secs(3.0), secs(5.0)]
        );
        let mut script = Script::new(presses, 2);
        let reads: Vec<f64> = (0..20).map(|i| 0.5 + f64::from(i) * 0.5).collect();
        let (seen, notes) = run(&mut script, 0.0, 10.0, &reads);
        let downs = notes
            .iter()
            .filter(|n| matches!(n, Note::Down { .. }))
            .count();
        let lets_go = notes
            .iter()
            .filter(|n| matches!(n, Note::Seen { reads: 2, .. }))
            .count();
        assert_eq!((downs, lets_go), (3, 3));
        // Between two presses some read saw the key up.
        let pattern: Vec<bool> = seen.iter().map(|(_, k)| k.contains(&ENTER)).collect();
        let edges = pattern.windows(2).filter(|w| !w[0] && w[1]).count();
        assert_eq!(edges, 3, "{pattern:?}");
        assert!(script.finished());
    }

    /// A key held down is one reading (the hub makes one per change), which
    /// the title reads again and again: each read counts.
    #[test]
    fn a_held_key_counts_every_read_of_its_one_reading() {
        let mut script = Script::new(parse("0:W~2").0, 1);
        script.down(Duration::ZERO, 0);
        for i in 1..=10u64 {
            script.read(8_000, i * 100_000);
        }
        let (_, notes) = script.down(secs(2.0), 2_000_000);
        let Some(Note::Held { reads, .. }) = notes.iter().find(|n| matches!(n, Note::Held { .. })) else {
            panic!("{notes:?}")
        };
        assert_eq!(*reads, 10);
    }

    #[test]
    fn reads_in_the_same_frame_count_once() {
        let mut script = Script::new(parse("0:Enter").0, 2);
        script.down(Duration::ZERO, 0);
        for us in [10_000, 11_000, 12_000] {
            script.read(us, us);
        }
        let (down, notes) = script.down(secs(0.5), 500_000);
        assert_eq!(down, [ENTER], "one frame's reads are one read: {notes:?}");
        script.read(400_000, 400_000);
        let (down, _) = script.down(secs(0.6), 600_000);
        assert!(down.is_empty());
    }

    /// `~<hold>`: a key held for a walk is held exactly that long, read
    /// often or not at all; repeats leave room for the hold; bad holds are
    /// named.
    #[test]
    fn a_held_key_stays_down_exactly_its_hold() {
        let (presses, bad) = parse("1:W~3, 10:D~2*2/5, 20:W~0.1, 20:W~61, 30:W~3*2/3");
        assert_eq!(bad, ["20:W~0.1", "20:W~61", "30:W~3*2/3"]);
        assert_eq!(presses.len(), 3);
        // Read often (every 0.1 s): not let go before its hold.
        let mut script = Script::new(presses.clone(), 1);
        let reads: Vec<f64> = (0..300).map(|i| f64::from(i) * 0.1).collect();
        let (_, notes) = run(&mut script, 0.0, 25.0, &reads);
        let helds: Vec<(u8, u32, Duration)> = notes
            .iter()
            .filter_map(|n| match n {
                Note::Held { vk, reads, held, .. } => Some((*vk, *reads, *held)),
                _ => None,
            })
            .collect();
        assert_eq!(helds.len(), 3, "{notes:?}");
        assert_eq!(helds[0].0, b'W');
        assert!(helds[0].1 >= 25, "read about 30 times: {helds:?}");
        assert!(helds[0].2 >= secs(3.0) && helds[0].2 < secs(3.05), "{helds:?}");
        for &(vk, _, held) in &helds[1..] {
            assert_eq!(vk, b'D');
            assert!(held >= secs(2.0) && held < secs(2.05), "{helds:?}");
        }
        // Never read: let go at its hold all the same, with 0 reads.
        let mut script = Script::new(presses, 1);
        let (_, notes) = run(&mut script, 0.0, 25.0, &[]);
        let unread: Vec<(u32, Duration)> = notes
            .iter()
            .filter_map(|n| match n {
                Note::Held { reads, held, .. } => Some((*reads, *held)),
                _ => None,
            })
            .collect();
        assert_eq!(unread.len(), 3, "{notes:?}");
        assert!(unread.iter().all(|&(r, h)| r == 0 && h < secs(3.05)), "{unread:?}");
    }

    #[test]
    fn the_old_form_still_presses_once() {
        let (presses, bad) = parse("150:Enter, 151.5:a");
        assert!(bad.is_empty());
        assert_eq!(
            presses,
            [
                Press {
                    at: secs(150.0),
                    vk: ENTER,
                    hold: None
                },
                Press {
                    at: secs(151.5),
                    vk: b'A',
                    hold: None
                }
            ]
        );
    }

    #[test]
    fn entries_not_understood_are_named() {
        let (presses, bad) = parse("x:Enter,5:F13,5:Enter*0/2,5:Enter*3/0.1,5:Enter*two,7:Esc");
        assert_eq!(
            presses,
            [Press {
                at: secs(7.0),
                vk: 0x1B,
                hold: None
            }]
        );
        assert_eq!(
            bad,
            [
                "x:Enter",
                "5:F13",
                "5:Enter*0/2",
                "5:Enter*3/0.1",
                "5:Enter*two"
            ]
        );
        // `*n` alone: 10 s apart.
        let (presses, _) = parse("2:Enter*2");
        assert_eq!(
            presses.iter().map(|p| p.at).collect::<Vec<_>>(),
            [secs(2.0), secs(12.0)]
        );
    }

    fn file(text: &str) -> Vec<Press> {
        parse_file(text).unwrap()
    }

    #[test]
    fn a_keys_file_is_pressed_whole_or_not_at_all() {
        let presses = file("Enter

  W~2 
");
        assert_eq!(presses.iter().map(|p| (p.vk, p.hold)).collect::<Vec<_>>(), [(ENTER, None), (b'W', Some(secs(2.0)))]);
        assert_eq!(parse_file("Enter
F13
5:Enter
W~0.1
Esc"), Err(vec!["F13".into(), "5:Enter".into(), "W~0.1".into()]));
        assert!(parse_file(&"Enter
".repeat(FILE_MAX_LINES + 1)).is_err());
        assert!(parse_file(&format!("{}Enter", " ".repeat(FILE_MAX_BYTES))).is_err());
        assert!(parse_file(" 
").is_err());
    }

    /// A key from a file while the same key is still down waits until a
    /// read has seen it up, then is a press of its own.
    #[test]
    fn a_file_key_waits_behind_the_same_key_down() {
        let mut script = Script::new(parse("0:Enter").0, 1);
        let (_, mut notes) = run(&mut script, 0.0, 0.1, &[0.05]);
        script.add_now(file("Enter")[0], secs(0.1));
        let (seen, more) = run(&mut script, 0.1, 3.0, &[1.0, 2.0]);
        notes.extend(more);
        assert_eq!(seen.iter().map(|(_, k)| k.contains(&ENTER)).collect::<Vec<_>>(), [false, true]);
        assert_eq!(notes.iter().filter(|n| matches!(n, Note::Down { .. })).count(), 2, "{notes:?}");
        assert!(script.finished());
    }

    #[test]
    fn two_files_in_a_row_are_both_pressed() {
        let mut script = Script::new(Vec::new(), 1);
        for p in file("Enter") {
            script.add_now(p, secs(1.0));
        }
        let (_, mut notes) = run(&mut script, 0.0, 1.1, &[0.5]);
        for p in file("Esc
Enter") {
            script.add_now(p, secs(1.1));
        }
        let reads: Vec<f64> = (3..20).map(|i| f64::from(i) * 0.5).collect();
        notes.extend(run(&mut script, 1.1, 10.0, &reads).1);
        let downs: Vec<u8> = notes.iter().filter_map(|n| match n {
            Note::Down { vk, .. } => Some(*vk),
            _ => None,
        }).collect();
        assert_eq!(downs, [ENTER, 0x1B, ENTER], "{notes:?}");
        assert_eq!(notes.iter().filter(|n| matches!(n, Note::Seen { .. })).count(), 3);
        assert!(script.finished());
    }

    #[test]
    fn a_keys_file_is_claimed_once_and_left_done_or_rejected() {
        let dir = std::env::temp_dir().join(format!("victory-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys");
        let at = |s: &str| dir.join(format!("keys{s}"));
        assert!(take_file(&path).unwrap().is_none());
        std::fs::write(&path, "Enter
").unwrap();
        assert_eq!(take_file(&path).unwrap().unwrap().unwrap().len(), 1);
        assert!(!path.exists() && at(".done").exists() && !at(".taken").exists());
        assert!(take_file(&path).unwrap().is_none(), "taken once");
        std::fs::write(&path, "Enter
nope
").unwrap();
        assert_eq!(take_file(&path).unwrap().unwrap(), Err(vec!["nope".into()]));
        assert!(!path.exists() && at(".rejected").exists());
        std::fs::write(&path, "Esc").unwrap();
        assert!(take_file(&path).unwrap().unwrap().is_ok(), ".done overwritten");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
