use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{
    glib::ControlFlow,
    prelude::*,
    {Adjustment, ScrolledWindow, TickCallbackId},
};
use relm4::prelude::*;

/// Exponential deceleration friction, matching GTK's kinetic scrolling.
const DECEL_FRICTION: f64 = 4.0;
/// Velocity in px/s below which the continuation stops.
const STOP_VELOCITY: f64 = 50.0;
/// Minimum upward velocity in px/s for a flick to continue after a prepend.
const MIN_CONTINUE_VELOCITY: f64 = 150.0;
/// Maximum time to wait for the post-splice layout to land.
const LAYOUT_TIMEOUT: Duration = Duration::from_millis(250);
/// Maximum gap between samples to count as touchpad-driven motion.
const MAX_SAMPLE_GAP: Duration = Duration::from_millis(120);
/// Minimum value increase from the baseline to consider the layout landed.
const LAYOUT_LANDED_THRESHOLD: f64 = 100.0;
/// Absolute velocity cap in px/s.
const MAX_VELOCITY: f64 = 30000.0;
/// Backwards value jump in px that signals a kinetic deceleration fight.
const FIGHT_JUMP_THRESHOLD: f64 = 15.0;
/// Minimum downward velocity in px/s for a fight takeover to continue.
const MIN_DOWN_VELOCITY: f64 = 150.0;
/// Minimum time between fight takeovers to avoid re-trigger churn.
const TAKEOVER_COOLDOWN: Duration = Duration::from_millis(300);
const PROGRAMMATIC_MUTE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimPhase {
    WaitingLayout,
    Running,
}

#[derive(Debug, Clone, Copy)]
struct Anim {
    phase: AnimPhase,
    armed_at: Instant,
    baseline: f64,
    velocity: f64,
    last_tick: Instant,
}

#[derive(Debug, Default)]
struct Inner {
    sw: Option<ScrolledWindow>,
    adj: Option<Adjustment>,
    anim: Option<Anim>,
    tick_id: Option<TickCallbackId>,
    muted_until: Option<Instant>,
    prev_sample: Option<(Instant, f64)>,
    last_velocity: Option<f64>,
    last_takeover_at: Option<Instant>,
    last_velocity_at: Option<Instant>,
}

/// Touchpad flick continuation across prepended message batches.
#[derive(Debug, Clone, Default)]
pub(crate) struct Momentum {
    inner: Rc<RefCell<Inner>>,
}

impl Momentum {
    /// Creates a new momentum tracker.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Attaches the tracker to a scrolled window.
    pub(crate) fn attach(&self, sw: &ScrolledWindow) {
        let adj = sw.vadjustment();
        let mut inner = self.inner.borrow_mut();
        inner.sw = Some(sw.clone());
        inner.adj = Some(adj);
    }

    /// Records a scroll position sample from the adjustment.
    ///
    /// Returns `true` when the value jumped backwards while a downward
    /// flick was still moving, which signals a kinetic deceleration fight.
    pub(crate) fn record(&self, value: f64) -> bool {
        let mut inner = self.inner.borrow_mut();
        let now = Instant::now();

        if inner.muted_until.is_some_and(|until| now < until) {
            inner.prev_sample = None;
            inner.last_velocity = None;
            inner.last_velocity_at = None;
            return false;
        }

        let fight = inner.anim.is_none()
            && inner
                .last_takeover_at
                .is_none_or(|t| t.elapsed() > TAKEOVER_COOLDOWN)
            && inner
                .prev_sample
                .is_some_and(|(_, prev_v)| prev_v - value > FIGHT_JUMP_THRESHOLD)
            && inner.last_velocity.is_some_and(|v| v > MIN_DOWN_VELOCITY);

        if !fight
            && inner.anim.is_none()
            && let Some((prev_t, prev_v)) = inner.prev_sample
        {
            let dt = now.duration_since(prev_t);
            if dt.as_secs_f64() > 1e-6 && (value - prev_v).abs() > 0.5 && dt <= MAX_SAMPLE_GAP {
                let velocity = (value - prev_v) / dt.as_secs_f64();
                if velocity.abs() <= MAX_VELOCITY {
                    inner.last_velocity = Some(velocity);
                    inner.last_velocity_at = Some(now);
                }
            }
        }

        inner.prev_sample = Some((now, value));
        fight
    }

    pub(crate) fn pause_recording(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.muted_until = Some(Instant::now() + PROGRAMMATIC_MUTE);
    }

    /// Captures the scroll baseline and flick velocity before a prepend.
    #[must_use]
    pub(crate) fn capture(&self) -> (f64, Option<f64>) {
        let mut inner = self.inner.borrow_mut();

        if let Some(id) = inner.tick_id.take() {
            id.remove();
        }

        let baseline = inner.adj.as_ref().map_or(0.0, AdjustmentExt::value);

        let velocity = if let Some(anim) = inner.anim.take() {
            (anim.velocity < -MIN_CONTINUE_VELOCITY).then_some(anim.velocity.max(-MAX_VELOCITY))
        } else if let (Some(v), Some(at)) = (inner.last_velocity, inner.last_velocity_at) {
            let decayed = v * (-DECEL_FRICTION * at.elapsed().as_secs_f64()).exp();
            (decayed < -MIN_CONTINUE_VELOCITY).then_some(decayed.max(-MAX_VELOCITY))
        } else {
            None
        };

        (baseline, velocity)
    }

    /// Continues a captured flick after the prepend splice.
    pub(crate) fn continue_from(&self, baseline: f64, velocity: Option<f64>) {
        let Some(velocity) = velocity else {
            return;
        };

        let mut inner = self.inner.borrow_mut();
        let Some(sw) = inner.sw.clone() else {
            return;
        };

        if let Some(id) = inner.tick_id.take() {
            id.remove();
        }

        let now = Instant::now();
        inner.anim = Some(Anim {
            phase: AnimPhase::WaitingLayout,
            armed_at: now,
            baseline,
            velocity,
            last_tick: now,
        });
        drop(inner);

        let momentum = self.clone();
        let tick_id = sw.add_tick_callback(move |_, _| momentum.tick());

        let mut inner = self.inner.borrow_mut();
        inner.tick_id = Some(tick_id);
    }

    /// Kills the kinetic deceleration and continues the flick with a
    /// closed-loop animation that absorbs listview compensation.
    pub(crate) fn take_over_down(&self) {
        let sw = self.inner.borrow().sw.clone();

        if let Some(sw) = sw.as_ref() {
            sw.set_kinetic_scrolling(false);
            sw.set_kinetic_scrolling(true);
        }

        let velocity = {
            let mut inner = self.inner.borrow_mut();

            if let Some(id) = inner.tick_id.take() {
                id.remove();
            }

            inner.last_takeover_at = Some(Instant::now());

            let velocity = match (inner.last_velocity, inner.last_velocity_at) {
                (Some(v), Some(at)) if v > 0.0 => {
                    let decayed = v * (-DECEL_FRICTION * at.elapsed().as_secs_f64()).exp();
                    (decayed > MIN_DOWN_VELOCITY).then(|| decayed.min(MAX_VELOCITY))
                }
                _ => None,
            };

            if let Some(vel) = velocity {
                let now = Instant::now();
                inner.anim = Some(Anim {
                    phase: AnimPhase::Running,
                    armed_at: now,
                    baseline: inner.adj.as_ref().map_or(0.0, AdjustmentExt::value),
                    last_tick: now,
                    velocity: vel,
                });
            }

            velocity
        };

        if velocity.is_some()
            && let Some(sw) = sw
        {
            let momentum = self.clone();
            let tick_id = sw.add_tick_callback(move |_, _| momentum.tick());

            let mut inner = self.inner.borrow_mut();
            inner.tick_id = Some(tick_id);
        }
    }

    /// Stops any running momentum animation.
    pub(crate) fn stop(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(id) = inner.tick_id.take() {
            id.remove();
        }
        inner.anim = None;
        inner.muted_until = None;
        inner.prev_sample = None;
        inner.last_velocity = None;
        inner.last_takeover_at = None;
        inner.last_velocity_at = None;
    }

    fn tick(&self) -> ControlFlow {
        let (anim, adj) = {
            let inner = self.inner.borrow();
            let Some(anim) = inner.anim else {
                return ControlFlow::Break;
            };
            let Some(adj) = inner.adj.clone() else {
                return ControlFlow::Break;
            };
            (anim, adj)
        };

        let now = Instant::now();
        let current = adj.value();

        match anim.phase {
            AnimPhase::WaitingLayout => {
                if current > anim.baseline + LAYOUT_LANDED_THRESHOLD {
                    self.write_anim(Anim {
                        phase: AnimPhase::Running,
                        last_tick: now,
                        ..anim
                    });
                    ControlFlow::Continue
                } else if anim.armed_at.elapsed() > LAYOUT_TIMEOUT {
                    self.clear_anim();
                    ControlFlow::Break
                } else {
                    ControlFlow::Continue
                }
            }
            AnimPhase::Running => {
                let dt = now.duration_since(anim.last_tick).as_secs_f64();

                let velocity = anim.velocity * (-DECEL_FRICTION * dt).exp();
                if velocity.abs() < STOP_VELOCITY {
                    self.clear_anim();
                    return ControlFlow::Break;
                }

                let new_value = current + velocity * dt;
                if new_value <= 0.0 {
                    adj.set_value(0.0);
                    self.clear_anim();
                    ControlFlow::Break
                } else {
                    adj.set_value(new_value);
                    self.write_anim(Anim {
                        velocity,
                        last_tick: now,
                        ..anim
                    });
                    ControlFlow::Continue
                }
            }
        }
    }

    fn write_anim(&self, anim: Anim) {
        let mut inner = self.inner.borrow_mut();
        inner.anim = Some(anim);
    }

    fn clear_anim(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.anim = None;
        inner.tick_id = None;
    }
}
