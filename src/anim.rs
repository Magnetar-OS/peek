// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The animation layer.
//!
//! Two transitions, and they are not the same kind of motion.
//!
//! **Open and close** use iced's [`Animation`], backed by `lilt`, because they
//! have to be interruptible: pressing space again while the panel is still
//! appearing must reverse from wherever it currently is rather than snapping to
//! fully-open and then closing. Tracking the interpolated value and re-basing
//! the clock by hand is exactly what `lilt` already does.
//!
//! **Stepping between files** is computed arithmetically from a single
//! [`Instant`]. A user holding the right arrow generates a new preview every few
//! frames; allocating an `Animation` per step would mean building and dropping
//! them faster than they finish. One timestamp plus a direction gives the same
//! cross-fade for nothing.
//!
//! There is no scale component on the open transition, for the same reason
//! `light` has none: iced's `Float` only applies a transform when scaling above
//! 1.0, so a 0.96 → 1.0 entrance silently renders unscaled.

use std::time::{Duration, Instant};

use cosmic::iced::animation::{Animation, Easing};

/// How long the panel takes to appear.
///
/// Slightly quicker than `light`'s 220 ms. A launcher's entrance is the start of
/// an interaction the user is about to type into; a preview's is the whole
/// interaction, and any delay before the content is legible is delay in the way.
const OPEN_DURATION: Duration = Duration::from_millis(180);

/// Closing is faster than opening. Dismissal should feel like it already
/// happened.
const CLOSE_DURATION: Duration = Duration::from_millis(120);

/// Vertical offset, in logical pixels, the panel rises through as it opens.
const RISE: f32 = 8.0;

/// How long a cross-fade between two files takes.
///
/// Short. This is not a slideshow transition — it exists so that arrowing
/// through a directory reads as one surface changing its contents rather than as
/// a series of separate windows, and anything longer starts to feel like a
/// delay between the key and the picture.
const STEP_DURATION: Duration = Duration::from_millis(130);

/// Distance, in logical pixels, the content slides during a step.
///
/// The direction carries which way the user moved, which is the whole reason the
/// step animation is directional rather than a plain fade.
const STEP_SLIDE: f32 = 24.0;

/// Drives the panel's transitions.
pub struct Panel {
    /// Target state: `true` open, `false` closed.
    progress: Animation<bool>,
    /// When the preview last changed, and which way the user moved.
    stepped: Instant,
    forward: bool,
    /// Whether transitions play at all.
    ///
    /// When off, every transition still *happens* — the panel opens, steps,
    /// and closes — it simply arrives instantly. Keeping the state machine and
    /// changing only its duration means there is no second code path that can
    /// rot, and no way for the surface to be left un-destroyed because a fade
    /// nobody played never "finished".
    animate: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            // EaseOutExpo decelerates hard, which is what makes the panel feel
            // like it settles into place rather than coasting to a stop.
            progress: Animation::new(false)
                .easing(Easing::EaseOutExpo)
                .duration(OPEN_DURATION),
            // Backdated so the first preview does not animate in on top of the
            // panel's own entrance.
            stepped: Instant::now() - STEP_DURATION,
            forward: true,
            animate: true,
        }
    }

    /// Turn transitions on or off.
    ///
    /// Applied to the *next* transition rather than the one in flight: a
    /// setting changed mid-fade should not make the panel jump.
    pub fn set_animated(&mut self, animate: bool) {
        self.animate = animate;
    }

    /// How long a transition should take, given the setting.
    fn duration(&self, full: Duration) -> Duration {
        if self.animate { full } else { Duration::ZERO }
    }

    /// Begin opening. Safe to call while closing — the transition reverses.
    pub fn open(&mut self, now: Instant) {
        self.progress = std::mem::replace(&mut self.progress, Animation::new(false))
            .duration(self.duration(OPEN_DURATION))
            .easing(Easing::EaseOutExpo);
        self.progress.go_mut(true, now);
    }

    /// Begin closing. Safe to call while opening.
    pub fn close(&mut self, now: Instant) {
        self.progress = std::mem::replace(&mut self.progress, Animation::new(false))
            .duration(self.duration(CLOSE_DURATION))
            // Symmetrical deceleration on the way out leaves the panel hanging
            // around at low opacity; accelerating away reads as crisper.
            .easing(Easing::EaseInQuad);
        self.progress.go_mut(false, now);
    }

    /// Note that the preview changed, restarting the cross-fade.
    pub fn stepped(&mut self, now: Instant, forward: bool) {
        self.stepped = now;
        self.forward = forward;
    }

    /// Panel opacity in 0.0..=1.0.
    #[must_use]
    pub fn opacity(&self, now: Instant) -> f32 {
        self.progress.interpolate(0.0, 1.0, now)
    }

    /// Vertical offset in logical pixels; positive moves the panel down.
    #[must_use]
    pub fn offset_y(&self, now: Instant) -> f32 {
        self.progress.interpolate(RISE, 0.0, now)
    }

    /// Opacity and horizontal offset of the *content* during a step.
    ///
    /// Returns `(opacity, offset_x)`. Multiplied into the panel's own opacity by
    /// the caller, so a step that begins while the panel is still opening fades
    /// from the panel's current level rather than from full.
    #[must_use]
    pub fn step(&self, now: Instant) -> (f32, f32) {
        if !self.animate {
            return (1.0, 0.0);
        }
        let elapsed = now.saturating_duration_since(self.stepped);
        if elapsed >= STEP_DURATION {
            return (1.0, 0.0);
        }

        let linear = elapsed.as_secs_f32() / STEP_DURATION.as_secs_f32();
        let eased = Easing::EaseOutCubic.value(linear);
        let direction = if self.forward { 1.0 } else { -1.0 };

        // Enters from the side the user came *from*: pressing right moves the
        // list left, so the new file slides in from the right.
        (eased, STEP_SLIDE * (1.0 - eased) * direction)
    }

    /// Whether a redraw is still needed.
    ///
    /// Used to decide whether to subscribe to frame callbacks. Returning false
    /// when idle is what keeps the previewer from redrawing at display rate
    /// while the user reads a text file.
    #[must_use]
    pub fn is_animating(&self, now: Instant) -> bool {
        self.animate
            && (self.progress.is_animating(now)
                || now.saturating_duration_since(self.stepped) < STEP_DURATION)
    }

    /// Whether the close transition has finished, meaning the surface can be
    /// destroyed. Destroying it earlier truncates the fade-out.
    #[must_use]
    pub fn is_closed(&self, now: Instant) -> bool {
        !self.progress.is_animating(now) && !self.progress.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_reaches_full_opacity_and_settles() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.open(start);

        let settled = start + OPEN_DURATION + Duration::from_millis(10);
        assert!((panel.opacity(settled) - 1.0).abs() < f32::EPSILON);
        assert!(panel.offset_y(settled).abs() < f32::EPSILON);
        assert!(!panel.is_animating(settled));
    }

    #[test]
    fn a_fresh_panel_is_not_mid_step() {
        let panel = Panel::new();
        let (opacity, offset) = panel.step(Instant::now());
        assert_eq!(opacity, 1.0);
        assert_eq!(offset, 0.0);
    }

    #[test]
    fn stepping_slides_from_the_direction_of_travel() {
        let mut panel = Panel::new();
        let start = Instant::now();

        panel.stepped(start, true);
        let (_, forward) = panel.step(start + Duration::from_millis(10));

        panel.stepped(start, false);
        let (_, backward) = panel.step(start + Duration::from_millis(10));

        assert!(forward > 0.0, "forward steps enter from the right");
        assert!(backward < 0.0, "backward steps enter from the left");
    }

    #[test]
    fn a_step_completes_and_stops_requesting_frames() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.stepped(start, true);

        let done = start + STEP_DURATION + Duration::from_millis(1);
        assert_eq!(panel.step(done), (1.0, 0.0));
        assert!(!panel.is_animating(done));
    }

    #[test]
    fn without_motion_the_panel_arrives_immediately_and_still_closes() {
        let mut panel = Panel::new();
        panel.set_animated(false);

        let start = Instant::now();
        panel.open(start);
        // Open at once, and asking for frames would be asking for nothing.
        assert!((panel.opacity(start) - 1.0).abs() < f32::EPSILON);
        assert!(panel.offset_y(start).abs() < f32::EPSILON);
        assert!(!panel.is_animating(start));
        assert_eq!(panel.step(start), (1.0, 0.0));

        // The close must still complete, or the surface is never destroyed.
        panel.close(start);
        assert!(panel.is_closed(start));
    }

    #[test]
    fn closing_is_only_finished_once_the_fade_has_played() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.open(start);
        panel.close(start + OPEN_DURATION);

        assert!(!panel.is_closed(start + OPEN_DURATION));
        assert!(panel.is_closed(start + OPEN_DURATION + CLOSE_DURATION + Duration::from_millis(1)));
    }
}
