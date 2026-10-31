use std::time::{Duration, Instant};

/// How long an arrival at the edge must remain unchallenged before
/// the request may land. Small enough for the trigger to feel direct,
/// long enough for a fling's own final drops to refresh the mark.
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Default, Debug)]
pub(crate) struct TopTrigger {
    pending: Option<Instant>,
}

impl TopTrigger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register an arrival at the top edge. Any other arrival
    /// afterward refreshes the same mark — repeated ticks replace
    /// rather than accumulate: the ticket only ever describes the
    /// newest one. Whether anything should land for it is `eligible`.
    pub(crate) fn mark(&mut self) {
        self.pending = Some(Instant::now());
    }

    /// A scroll away from the edge abandons an unlanded arrival;
    /// no request should surface for it.
    pub(crate) fn abandon(&mut self) {
        self.pending = None;
    }

    /// Whether the timeframe since the newest arrival is enough to
    /// actually ask for the page. Instant-identity-matched: only the
    /// ticket that the caller itself may or may not still *own* gets
    /// to land, so a later mark never hands its work to an earlier
    /// one.
    pub(crate) fn eligible(&self, ticket: Option<Instant>, now: Instant) -> bool {
        matches!(
            (self.pending, ticket),
            (Some(pending), Some(ticket))
            if pending == ticket && now.duration_since(pending) >= SETTLE
        )
    }

    /// Fire the shot for the holder of the matching ticket. Returns
    /// `true` once, after which a new arrival is required.
    pub(crate) fn fire_if_eligible(&mut self, ticket: Option<Instant>) -> bool {
        if self.eligible(ticket, Instant::now()) {
            self.pending = None;
            return true;
        }

        false
    }

    pub(crate) fn pending(&self) -> Option<Instant> {
        self.pending
    }
}
