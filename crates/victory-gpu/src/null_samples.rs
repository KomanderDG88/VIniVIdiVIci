//! `VICTORY_NULL_SAMPLES=count` (diagnostics; off when unset; changes
//! nothing): for every draw whose pixel shader picks slots by index where
//! the layer can tell which (`State::null_samples`), what their host slots
//! show at the draw ([`Counts`]), to attribute black or banded faces to a
//! draw. A WARN for each of the first 64 draws with something wrong
//! (`VICTORY_NULL_SAMPLES_PS=<hash>[,<hash>]`: those pixel shaders' only),
//! and while they come an INFO at most once a second with the top 5 pixel
//! shaders by such draws ([`Tally`]).
//!
//! Cost when on: three locks per slot. A root-selected view is a few slots
//! (some microseconds a draw); a bake draw names up to 1,290 (5 constants,
//! 257 records of 5): some tens of microseconds. Thousands of such draws a
//! frame make several milliseconds of the recording thread: for
//! attribution runs, not for measuring frame rates. Unset: one cached
//! branch a draw.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Whether `VICTORY_NULL_SAMPLES=count` (read once).
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("VICTORY_NULL_SAMPLES").is_ok_and(|v| v.trim() == "count");
        if on {
            tracing::info!(only = ?only(), "slots draws pick by index, by what their host slot shows (VICTORY_NULL_SAMPLES)");
        }
        on
    })
}

/// The pixel shaders `VICTORY_NULL_SAMPLES_PS` keeps the WARNs to (hash
/// prefixes); empty: all.
fn only() -> &'static [String] {
    static ONLY: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    ONLY.get_or_init(|| {
        std::env::var("VICTORY_NULL_SAMPLES_PS")
            .map(|v| v.split(',').map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect())
            .unwrap_or_default()
    })
}

/// What a draw's slots picked by index show (a slot may be null and stale).
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct Counts {
    /// Slots that hold a texture view.
    pub(crate) textures: u32,
    /// Of them: the host slot shows nothing (`SlotShows::Null`).
    pub(crate) null: u32,
    /// Made from this record but null (the host could not make it).
    pub(crate) made_but_null: u32,
    /// A copy shown while it waits in the queue (written or mapped after
    /// its upload), or nothing shown, its memory there, and in no queue
    /// (made stale: the bake's `made_stale`).
    pub(crate) stale: u32,
    /// Showing a copy made from another record than the title's now.
    pub(crate) other_copy: u32,
    /// Slots that hold a buffer view.
    pub(crate) buffers: u32,
    /// Slots past the host heap's: they read its null slot.
    pub(crate) clamped: u32,
}

/// Draws with something wrong, by pixel shader: draws, null, stale.
#[derive(Default, Debug)]
pub(crate) struct Tally {
    by: HashMap<String, [u64; 3]>,
    draws: u64,
}

impl Tally {
    pub(crate) fn add(&mut self, pixel: &str, null: u64, stale: u64) {
        let t = self.by.entry(pixel.to_owned()).or_default();
        t[0] += 1;
        t[1] += null;
        t[2] += stale;
        self.draws += 1;
    }

    /// The `n` pixel shaders with the most draws (ties by hash).
    pub(crate) fn top(&self, n: usize) -> Vec<(&str, [u64; 3])> {
        let mut all: Vec<(&str, [u64; 3])> = self.by.iter().map(|(k, &v)| (k.as_str(), v)).collect();
        all.sort_unstable_by(|a, b| b.1[0].cmp(&a.1[0]).then(a.0.cmp(b.0)));
        all.truncate(n);
        all
    }
}

/// Counts one draw of pixel shader `pixel`, and logs as the module says.
pub(crate) fn record(pixel: &str, c: Counts) {
    static TALLY: Mutex<Option<Tally>> = Mutex::new(None);
    static SAID_MS: AtomicU64 = AtomicU64::new(0);
    static LINES: AtomicU64 = AtomicU64::new(0);
    if c.null + c.made_but_null + c.stale + c.other_copy + c.clamped == 0 {
        return;
    }
    let frame = crate::frame::current();
    let shown = only().is_empty() || only().iter().any(|h| pixel.starts_with(h.as_str()));
    if shown && LINES.fetch_add(1, Relaxed) < 64 {
        tracing::warn!(
            frame,
            pixel,
            textures = c.textures,
            null = c.null,
            made_but_null = c.made_but_null,
            stale = c.stale,
            other_copy = c.other_copy,
            buffers = c.buffers,
            clamped = c.clamped,
            "a draw samples slots picked by index whose host slot is null, stale or another copy (VICTORY_NULL_SAMPLES)"
        );
    }
    let mut tally = TALLY.lock().unwrap_or_else(|e| e.into_inner());
    let tally = tally.get_or_insert_with(Tally::default);
    tally.add(pixel, u64::from(c.null + c.made_but_null + c.clamped), u64::from(c.stale));
    let now = crate::host::now_ms();
    if now.saturating_sub(SAID_MS.load(Relaxed)) < 1000 {
        return;
    }
    SAID_MS.store(now, Relaxed);
    let top: Vec<String> =
        tally.top(5).iter().map(|(h, [d, n, s])| format!("{h}: {d} draws, {n} null, {s} stale")).collect();
    tracing::info!(
        frame,
        draws = tally.draws,
        top = %top.join("; "),
        "draws in the last second sampling slots picked by index that are null or stale, top pixel shaders (VICTORY_NULL_SAMPLES)"
    );
    *tally = Tally::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_by_draws_then_hash() {
        let mut t = Tally::default();
        for (pixel, null, stale) in [("b", 1, 0), ("a", 2, 1), ("c", 0, 3), ("a", 1, 1), ("c", 4, 0), ("d", 0, 1)] {
            t.add(pixel, null, stale);
        }
        assert_eq!(t.draws, 6);
        assert_eq!(t.top(3), vec![("a", [2, 3, 2]), ("c", [2, 4, 3]), ("b", [1, 1, 0])]);
        assert_eq!(t.top(9).len(), 4);
        assert!(Tally::default().top(5).is_empty());
    }
}
