//! Residency by use (notes 053): what of the card's memory the host copies
//! of textures the title fills hold, set against what draws sample.
//!
//! - `VICTORY_RESIDENCY=1` (off by default; does something only with
//!   `VICTORY_LEVELS_ON_DEMAND=1`): the tiles of the levels the title
//!   itself no longer holds go back while the host copy lives
//!   (`crate::tiles::Tiled::take_back_on`; before, they went back only
//!   with the copy). Such a level was written with zeros by the upload
//!   that found it let go, so a level given back reads what it read: zeros.
//! - `VICTORY_RESIDENCY_STATS=1` (diagnostics builds; off by default;
//!   changes nothing): per title slot, the frame a draw last sampled it,
//!   and how that is known ([`TABLE`], [`ROOT`], [`BAKE`]); every
//!   [`EVERY`] frames a line with the textures the slots show by whether
//!   they were sampled in the last [`WINDOW`] frames
//!   (`HostHeap::residency_census`).
//!
//! What is sampled is known three ways. Exactly: a draw read the slot
//! through a table (the table-read stamp, `HostHeap::tabled_in`), or a
//! pixel shader picks it from root constants or a skin bake's records
//! (`State::residency_samples`). Possibly: a shader picks slots by an index
//! it computes from other memory; nothing says which, so every made slot
//! that is not a table's counts as possibly sampled in a frame in which
//! such a shader drew.

use std::sync::atomic::AtomicU64;
#[cfg(any(test, feature = "diagnostics"))]
use std::sync::atomic::Ordering::Relaxed;

/// Whether the tiles of levels the title let go of go back while the host
/// copy lives (`VICTORY_RESIDENCY=1`; off by default). Read once.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_RESIDENCY").is_some_and(|v| v == "1");
        if on {
            tracing::info!(
                calls_a_pass = CALLS_A_PASS,
                "the tiles of texture levels the title let go of go back to the pool while the host copy lives (VICTORY_RESIDENCY=1; \
                 with VICTORY_LEVELS_ON_DEMAND=1 only)"
            );
        }
        on
    })
}

/// Since the start ([`on`]): levels given back, their bytes, the calls of
/// `UpdateTileMappings` that took their tiles away, and times a texture's
/// levels were put off because a list recorded a write of the card's into
/// it and was not submitted yet.
pub(crate) static RETURNED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Calls of `UpdateTileMappings` a pass that gives levels back makes at
/// most (notes 053: at most 32 a frame); the rest wait for the next pass.
pub(crate) const CALLS_A_PASS: u32 = 32;

/// Of the levels with tiles `given` (a bit a level), the ones whose tiles
/// go back now: let go of by the title (`loose`: with tiles and neither
/// held nor written by the card at the upload of submission `loose_at`,
/// 0: none) and still not `named` (held or the card's now), once that
/// submission has finished (`finished`: submissions finished; its zeros
/// are in them), while no list holds a recorded write of the card's into
/// the texture (`pending`). The levels the card packs together
/// (`packed_bits`) go together or not at all. Nothing with the switch off.
#[allow(clippy::too_many_arguments)]
pub(crate) fn returnable(
    on: bool,
    given: u32,
    named: u32,
    loose: u32,
    loose_at: u64,
    finished: u64,
    pending: u32,
    packed_bits: u32,
) -> u32 {
    if !on || pending != 0 || loose_at == 0 || loose_at > finished {
        return 0;
    }
    let mut out = given & loose & !named;
    if packed_bits != 0 && out & packed_bits != packed_bits {
        out &= !packed_bits;
    }
    out
}

/// Whether the census of sampled textures runs (`VICTORY_RESIDENCY_STATS=1`;
/// off by default). Read once. When on: a store per slot and frame for
/// each table read and each slot a pixel shader picks from root constants
/// or bake records, and every [`EVERY`] frames a pass over the placed
/// slots with a lock per slot (as the five-second slot census): for runs
/// that measure memory, not frame rates.
#[cfg(feature = "diagnostics")]
pub(crate) fn stats_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_RESIDENCY_STATS").is_some_and(|v| v == "1");
        if on {
            tracing::info!(window = WINDOW, every = EVERY, "textures by what draws sample (VICTORY_RESIDENCY_STATS=1)");
        }
        on
    })
}

/// Frames a sample counts for in the census.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) const WINDOW: u64 = 64;

/// Frames between two census lines.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) const EVERY: u64 = 16;

/// A stamp's frame bits (frame plus one, wrapping; 0: never).
#[cfg(any(test, feature = "diagnostics"))]
const FRAME_BITS: u32 = 0x3fff_ffff;

/// How a stamp's sample is known: a draw read the slot through a table.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) const TABLE: u32 = 1;
/// A pixel shader picks it by an index from its root constants.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) const ROOT: u32 = 2;
/// A skin bake picks it from its records.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) const BAKE: u32 = 3;

/// The stamp of a sample in `frame` known by `source` (one of [`TABLE`],
/// [`ROOT`], [`BAKE`]): the frame plus one in the low 30 bits (never 0),
/// the source in the top two.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) fn stamp(frame: u64, source: u32) -> u32 {
    let f = (frame as u32).wrapping_add(1) & FRAME_BITS;
    (if f == 0 { 1 } else { f }) | (source & 3) << 30
}

/// Frames from `stamp`'s to `frame`; `None` for a stamp of no sample.
#[cfg(any(test, feature = "diagnostics"))]
fn age(stamp: u32, frame: u64) -> Option<u64> {
    let then = stamp & FRAME_BITS;
    if then == 0 {
        return None;
    }
    let now = (frame as u32).wrapping_add(1) & FRAME_BITS;
    Some(u64::from(now.wrapping_sub(then) & FRAME_BITS))
}

/// Of two stamps, the later sample's (0 if neither is one).
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) fn newer(a: u32, b: u32, frame: u64) -> u32 {
    match (age(a, frame), age(b, frame)) {
        (Some(x), Some(y)) => {
            if y < x {
                b
            } else {
                a
            }
        }
        (Some(_), None) => a,
        (None, Some(_)) => b,
        (None, None) => 0,
    }
}

/// The frame (plus one) in which a shader that picks views from the heap
/// by an index it computes last drew (0: never).
#[cfg(any(test, feature = "diagnostics"))]
static PICKED: AtomicU64 = AtomicU64::new(0);

/// A shader that picks views from the heap by an index it computes is
/// drawn in `frame`.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) fn picked_by_index(frame: u64) {
    if PICKED.load(Relaxed) != frame + 1 {
        PICKED.store(frame + 1, Relaxed);
    }
}

/// Whether such a shader drew within the last `window` frames of `frame`.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) fn picked_lately(frame: u64, window: u64) -> bool {
    let last = PICKED.load(Relaxed);
    last != 0 && (frame + 1).saturating_sub(last) <= window
}

/// Of a texture or slot: 0 sampled within the window (exactly), 1 possibly
/// sampled, 2 not sampled lately.
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) fn class(stamp: u32, possible: bool, frame: u64, window: u64) -> usize {
    match age(stamp, frame) {
        Some(a) if a <= window => 0,
        _ if possible => 1,
        _ => 2,
    }
}

/// A census of textures by what draws sample ([`aggregate`]).
#[cfg(any(test, feature = "diagnostics"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Textures sampled exactly, possibly, not lately ([`class`]).
    pub(crate) textures: [u64; 3],
    /// Their bytes on the card.
    pub(crate) bytes: [u64; 3],
    /// Of the bytes sampled exactly, by how the last sample is known
    /// ([`TABLE`], [`ROOT`], [`BAKE`]).
    pub(crate) exact: [u64; 3],
    /// Slots held for room, by the same classes.
    pub(crate) held: [u64; 3],
}

/// The census in `frame` with a window of `window` frames: `textures` as
/// (bytes on the card, the newest stamp of its slots, whether a slot of it
/// is possibly sampled), `held` the slots held for room as (stamp,
/// possibly sampled).
#[cfg(any(test, feature = "diagnostics"))]
pub(crate) fn aggregate(
    frame: u64,
    window: u64,
    textures: impl IntoIterator<Item = (u64, u32, bool)>,
    held: impl IntoIterator<Item = (u32, bool)>,
) -> Summary {
    let mut s = Summary::default();
    for (bytes, stamp, possible) in textures {
        let c = class(stamp, possible, frame, window);
        s.textures[c] += 1;
        s.bytes[c] += bytes;
        if c == 0 {
            let source = (stamp >> 30) as usize;
            if (1..=3).contains(&source) {
                s.exact[source - 1] += bytes;
            }
        }
    }
    for (stamp, possible) in held {
        s.held[class(stamp, possible, frame, window)] += 1;
    }
    s
}

/// Whether the census is due in `frame` (every [`EVERY`] frames; one
/// caller of the frame gets it).
#[cfg(feature = "diagnostics")]
pub(crate) fn due(frame: u64) -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let last = LAST.load(Relaxed);
    frame >= last + EVERY && LAST.compare_exchange(last, frame, Relaxed, Relaxed).is_ok()
}

/// The census line (diagnostics): `s` over `slots` placed slots in
/// `micros`; with what levels given back did since the line before.
#[cfg(feature = "diagnostics")]
pub(crate) fn log(frame: u64, s: &Summary, slots: u64, micros: u64) {
    static BEFORE: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
    let returned: Vec<u64> = RETURNED.iter().map(|r| r.load(Relaxed)).collect();
    let levels = returned[0] - BEFORE[0].swap(returned[0], Relaxed);
    let bytes = returned[1] - BEFORE[1].swap(returned[1], Relaxed);
    let frames = frame.saturating_sub(BEFORE[2].swap(frame, Relaxed)).max(1);
    let filled = crate::resource::KINDS[1].load(Relaxed);
    let tiles = crate::tiles::given_total();
    let mb = |b: u64| b >> 20;
    tracing::info!(
        "residency (VICTORY_RESIDENCY_STATS=1): host copies of textures the title fills hold {} MB of the card ({} MB whole copies, \
         {} MB tiles of levels on demand); the textures placed slots show: {} sampled in the last {} frames exactly, {} MB (by the \
         last sample: tables {} MB, root constants {} MB, bake records {} MB); {} possibly (a shader picked by a computed index), \
         {} MB; {} not sampled lately, {} MB; slots held for room: {} sampled exactly, {} possibly, {} not lately; levels the title \
         let go of given back (VICTORY_RESIDENCY={}): {} levels, {} MB in the last {} frames ({} KB a frame), in all {} levels, \
         {} MB, {} calls of UpdateTileMappings, {} times put off for a card write not submitted; the census took {} us over {} slots",
        mb(filled + tiles),
        mb(filled),
        mb(tiles),
        s.textures[0],
        WINDOW,
        mb(s.bytes[0]),
        mb(s.exact[0]),
        mb(s.exact[1]),
        mb(s.exact[2]),
        s.textures[1],
        mb(s.bytes[1]),
        s.textures[2],
        mb(s.bytes[2]),
        s.held[0],
        s.held[1],
        s.held[2],
        u8::from(on()),
        levels,
        mb(bytes),
        frames,
        (bytes / frames) >> 10,
        returned[0],
        mb(returned[1]),
        returned[2],
        returned[3],
        micros,
        slots,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_keep_frame_and_source_and_compare_by_age() {
        let a = stamp(100, TABLE);
        let b = stamp(130, BAKE);
        assert_eq!(a >> 30, TABLE);
        assert_eq!(b >> 30, BAKE);
        assert_eq!(age(a, 100), Some(0));
        assert_eq!(age(a, 164), Some(64));
        assert_eq!(newer(a, b, 140), b);
        assert_eq!(newer(b, a, 140), b);
        assert_eq!(newer(0, a, 140), a);
        assert_eq!(newer(0, 0, 140), 0);
        // The frame bits wrap; a stamp is never 0.
        assert_ne!(stamp(u64::from(FRAME_BITS), ROOT) & FRAME_BITS, 0);
        assert_eq!(age(stamp(u64::from(FRAME_BITS) - 1, ROOT), u64::from(FRAME_BITS) + 2), Some(3));
    }

    #[test]
    fn aggregate_sorts_textures_and_held_slots_by_class() {
        let frame = 1000;
        let textures = [
            // Read through a table 10 frames ago.
            (4 << 20, stamp(990, TABLE), false),
            // Picked from root constants just now; also possible.
            (2 << 20, stamp(1000, ROOT), true),
            // From a bake 64 frames ago: still in the window.
            (1 << 20, stamp(936, BAKE), false),
            // Sampled 65 frames ago, possibly sampled since.
            (8 << 20, stamp(935, TABLE), true),
            // Never stamped, possible.
            (16 << 20, 0, true),
            // Never, not possible.
            (32 << 20, 0, false),
            // Long ago.
            (64 << 20, stamp(10, ROOT), false),
        ];
        let held = [(stamp(999, ROOT), false), (0, true), (0, false), (stamp(1, TABLE), false)];
        let s = aggregate(frame, WINDOW, textures, held);
        assert_eq!(s.textures, [3, 2, 2]);
        assert_eq!(s.bytes, [7 << 20, 24 << 20, 96 << 20]);
        assert_eq!(s.exact, [4 << 20, 2 << 20, 1 << 20]);
        assert_eq!(s.held, [1, 1, 2]);
        assert_eq!(aggregate(frame, WINDOW, [], []), Summary::default());
    }

    #[test]
    fn index_picks_count_for_the_window() {
        picked_by_index(500);
        assert!(picked_lately(500, WINDOW));
        assert!(picked_lately(564, WINDOW));
        assert!(!picked_lately(565, WINDOW));
    }

    #[test]
    fn returnable_needs_the_switch_the_finished_upload_and_no_card_write() {
        let packed = 0b1110_0000;
        // Levels 1 and 2 let go of at submission 7; 3.. held.
        let (given, named, loose) = (0xff, 0xf8, 0b110);
        assert_eq!(returnable(false, given, named, loose, 7, 9, 0, packed), 0, "switch off: nothing");
        assert_eq!(returnable(true, given, named, loose, 7, 6, 0, packed), 0, "before the upload's submission finished");
        assert_eq!(returnable(true, given, named, loose, 7, 7, 1, packed), 0, "a card write recorded");
        assert_eq!(returnable(true, given, named, loose, 0, 9, 0, packed), 0, "none let go of");
        assert_eq!(returnable(true, given, named, loose, 7, 7, 0, packed), 0b110);
        // Held again since: kept.
        assert_eq!(returnable(true, given, named | 0b10, loose, 7, 7, 0, packed), 0b100);
        // Without tiles: nothing to give.
        assert_eq!(returnable(true, given & !0b110, named, loose, 7, 7, 0, packed), 0);
        // The packed levels only all together.
        assert_eq!(returnable(true, 0xff, 0b1, 0b1110_0000, 3, 3, 0, packed), packed);
        assert_eq!(returnable(true, 0xff, 0b1000_0001, 0b0110_0000, 3, 3, 0, packed), 0);
    }
}
