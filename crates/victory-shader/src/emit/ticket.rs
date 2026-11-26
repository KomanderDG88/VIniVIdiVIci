//! The group counter of issue #1243: whether it is on, and the count of the lowerings that took
//! it. Kept out of `lower.rs`, which is at the size the asset policy lets a source file have.

/// Whether programs with console lanes in groups of several host waves take their console
/// threads from a group counter (`ticket`, issue #1243). `VICTORY_THREAD_TICKET=0`: the flat id
/// of the wave's first lane rounded down, as before.
pub(super) fn ticket_on() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_TICKET.get() {
        return on;
    }
    std::env::var_os("VICTORY_THREAD_TICKET").is_none_or(|v| v != "0")
}

#[cfg(test)]
thread_local! {
    /// A test's choice for [`ticket_on`], for its thread only (the process's environment is
    /// shared with the tests that translate meanwhile).
    pub(super) static TEST_TICKET: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    /// See [`thread_tickets`].
    static TICKETS: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// One lowering of a program with console lanes in a group of several host waves outside the
/// paired form and wave64-in-halves: `on`, it took the group counter (`ticket`), else
/// `VICTORY_THREAD_TICKET=0` kept the old rounding.
pub(super) fn count_ticket(on: bool) {
    TICKETS.with(|t| {
        let (took, off) = t.get();
        t.set(if on { (took + 1, off) } else { (took, off + 1) });
    });
}

/// On the calling thread since it started, the lowerings whose console threads come from the
/// group counter of issue #1243 (`ticket`), and those that would have but for
/// `VICTORY_THREAD_TICKET=0`. Per thread, so that a caller can take the difference around one
/// translation (`victory-gpu`'s shader cache does) while others translate elsewhere. A
/// program may be lowered more than once in its translation (forms tried in turn): the
/// difference says whether it was, not how many programs.
pub fn thread_tickets() -> (u64, u64) {
    TICKETS.with(std::cell::Cell::get)
}
