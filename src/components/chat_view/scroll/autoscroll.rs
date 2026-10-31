use relm4::gtk::{self, prelude::AdjustmentExt};

/// The adjustment's lower end state, live, from one call: the only
/// "am I at the bottom" the system ever asks, because candidates and
/// gestures evaluate identically everywhere else.
pub(crate) fn touching_bottom(adj: &gtk::Adjustment) -> bool {
    adj.value() + adj.page_size() >= adj.upper() - 2.0
}

/// Whether the reader is parked in the near-bottom region loosely
/// enough that growth should be re-pinned without a hiccup: zone in
/// on the one the scroll-to-bottom flow deliberately used to mean
/// "someone is watching the newest rows".
pub(crate) fn parked_at_bottom(adj: &gtk::Adjustment) -> bool {
    adj.value() + adj.page_size() >= adj.upper() - 25.0
}

#[derive(Default, Debug)]
pub(crate) struct AutoScroll {
    /// One explicit jump currently owns the adjustment.
    jump_in_flight: bool,
    /// The reader parked the viewport at the bottom themselves.
    reader_at_bottom: bool,
}

impl AutoScroll {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The reader dictated a scroll to the bottom.
    pub(crate) fn begin_jump(&mut self) {
        self.jump_in_flight = true;
        self.reader_at_bottom = false;
    }

    /// The jump arrived — whatever draws the bottom did its part.
    /// The viewport is now owned by the reader again until they
    /// scroll.
    pub(crate) fn land(&mut self, adj: &gtk::Adjustment) {
        self.jump_in_flight = false;
        self.reader_at_bottom = touching_bottom(adj);
    }

    /// The user took the adjustment out of machine hands.
    pub(crate) fn user_moved(&mut self, adj: &gtk::Adjustment) {
        if touching_bottom(adj) {
            self.jump_in_flight = false;
            self.reader_at_bottom = true;
        } else {
            self.reader_at_bottom = false;
        }
    }

    /// Whether content growth may move the viewport at all right now.
    pub(crate) fn contents_grow(&self) -> bool {
        self.jump_in_flight || self.reader_at_bottom
    }

    /// The one home call: a bottom of the content just appeared, and
    /// when the viewport belongs to us, snap, cleanly.
    pub(crate) fn pin_bottom(&mut self, adj: &gtk::Adjustment) {
        if !self.contents_grow() {
            return;
        }

        let target = (adj.upper() - adj.page_size()).max(adj.lower());
        adj.set_value(target);
        self.reader_at_bottom = true;
    }

    pub(crate) fn is_jump_in_flight(&self) -> bool {
        self.jump_in_flight
    }

    pub(crate) fn is_reader_at_bottom(&self) -> bool {
        self.reader_at_bottom
    }
}
