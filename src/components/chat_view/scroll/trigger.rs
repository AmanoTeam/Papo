use std::time::{Duration, Instant};

/// Quiet time at the edge before a load request may land.
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Default, Debug)]
pub(crate) struct TopTrigger {
    pending: Option<Instant>,
}

impl TopTrigger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register an arrival at the top edge; a later arrival replaces
    /// the ticket, never accumulates.
    pub(crate) fn mark(&mut self) {
        self.pending = Some(Instant::now());
    }

    /// A scroll away from the edge abandons an unlanded arrival;
    /// no request should surface for it.
    pub(crate) fn abandon(&mut self) {
        self.pending = None;
    }

    /// Whether the newest ticket has sat past the quiet window.
    pub(crate) fn eligible(&self, ticket: Option<Instant>, now: Instant) -> bool {
        matches!(
            (self.pending, ticket),
            (Some(pending), Some(ticket))
            if pending == ticket && now.duration_since(pending) >= SETTLE
        )
    }

    /// Spend the newest ticket if eligible; `true` once per arrival.
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
