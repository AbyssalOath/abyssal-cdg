//! Pure logic for the line-level fine-tuning timeline: time <-> pixel
//! mapping, zoom bounds, tick spacing, and how a drag on a bubble's body or
//! edges translates into new `start`/`sing_end` values. Kept separate from
//! `main.rs` (which owns the actual egui widget/painting/interaction glue)
//! so this math can be unit tested without a running egui context.
//!
//! One bubble is drawn per timed line, spanning `[start, sing_end)` - the
//! part of the line that's actually being sung (a real karaoke-editor
//! "clip"). The gap between one bubble's right edge and the next bubble's
//! left edge is the musical break between lines, and is intentionally left
//! empty on the timeline (no clip to show there).

/// Horizontal zoom, in pixels per second of song time.
pub const MIN_PX_PER_SEC: f32 = 4.0;
pub const MAX_PX_PER_SEC: f32 = 400.0;

/// A drag starting within this many pixels of a bubble's left/right edge
/// grabs that edge (to resize) instead of the whole bubble (to move it).
pub const EDGE_GRAB_PX: f32 = 8.0;

/// Maps song time (seconds) to/from horizontal pixel position within the
/// timeline's drawing area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub px_per_sec: f32,
    /// Song time (seconds) at the left edge of the visible area.
    pub scroll_secs: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            px_per_sec: 40.0,
            scroll_secs: 0.0,
        }
    }
}

impl View {
    pub fn time_to_x(&self, area_left: f32, t: f64) -> f32 {
        area_left + ((t - self.scroll_secs) * self.px_per_sec as f64) as f32
    }

    pub fn x_to_time(&self, area_left: f32, x: f32) -> f64 {
        self.scroll_secs + ((x - area_left) / self.px_per_sec) as f64
    }

    /// Zoom by `factor` (>1 zooms in, <1 zooms out), keeping `anchor_time`
    /// (usually whatever time is under the mouse cursor) fixed on screen.
    pub fn zoom_at(&mut self, factor: f32, area_left: f32, anchor_time: f64) {
        let anchor_x = self.time_to_x(area_left, anchor_time);
        self.px_per_sec = (self.px_per_sec * factor).clamp(MIN_PX_PER_SEC, MAX_PX_PER_SEC);
        // Re-solve scroll so `anchor_time` still lands at `anchor_x`.
        self.scroll_secs = anchor_time - ((anchor_x - area_left) / self.px_per_sec) as f64;
        self.clamp_scroll();
    }

    pub fn pan_by_pixels(&mut self, dx_px: f32) {
        self.scroll_secs -= (dx_px / self.px_per_sec) as f64;
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        if self.scroll_secs < 0.0 {
            self.scroll_secs = 0.0;
        }
    }
}

/// A "nice" tick spacing (in seconds) for the time ruler, given the current
/// zoom - the smallest of a fixed set of round intervals whose on-screen
/// spacing is still at least `min_px` apart, so labels never overlap.
pub fn tick_interval_secs(px_per_sec: f32, min_px: f32) -> f64 {
    const CANDIDATES: &[f64] = &[
        0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
    ];
    for &secs in CANDIDATES {
        if secs as f32 * px_per_sec >= min_px {
            return secs;
        }
    }
    *CANDIDATES.last().unwrap()
}

/// Which part of a bubble a drag starting at `local_x` (pixels from the
/// bubble's own left edge, i.e. `pointer_x - bubble_left`) should affect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    /// Move the whole bubble - changes `start`, preserving its duration.
    Body,
    /// Trim the left edge - changes `start`, keeping `sing_end` fixed.
    LeftEdge,
    /// Trim the right edge - changes `sing_end`, keeping `start` fixed.
    RightEdge,
}

pub fn classify_drag(local_x: f32, width_px: f32) -> DragMode {
    // Shrink the grab zone for bubbles narrower than 2*EDGE_GRAB_PX so both
    // edges stay reachable (split down the middle) instead of the left
    // check's fixed threshold swallowing the whole bubble.
    let edge = EDGE_GRAB_PX.min(width_px / 2.0);
    if local_x <= edge {
        DragMode::LeftEdge
    } else if local_x >= width_px - edge {
        DragMode::RightEdge
    } else {
        DragMode::Body
    }
}

/// The range a line's `start`/`sing_end` may move within while dragging on
/// the timeline - never overlapping the previous line's sung window or the
/// next line's start, so dragging can't reorder lines or eat into a
/// neighbor's time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragBounds {
    pub min_start: f64,
    pub max_sing_end: f64,
}

pub fn drag_bounds(prev_sing_end: Option<f64>, next_start: Option<f64>) -> DragBounds {
    DragBounds {
        min_start: prev_sing_end.unwrap_or(0.0).max(0.0),
        max_sing_end: next_start.unwrap_or(f64::INFINITY),
    }
}

/// Smallest sung-window duration a drag will leave a line with - keeps a
/// dragged-to-zero bubble from becoming unselectable/invisible.
const MIN_DURATION: f64 = 0.1;

/// Move the whole bubble by `delta_secs`, preserving its duration. Returns
/// the new `(start, sing_end)`.
pub fn apply_body_drag(
    orig_start: f64,
    orig_sing_end: f64,
    delta_secs: f64,
    bounds: DragBounds,
) -> (f64, f64) {
    let duration = (orig_sing_end - orig_start).max(MIN_DURATION);
    let max_start = (bounds.max_sing_end - duration).max(bounds.min_start);
    let new_start = (orig_start + delta_secs).clamp(bounds.min_start, max_start);
    (new_start, new_start + duration)
}

/// Trim the left edge by `delta_secs`, keeping `sing_end` fixed. Returns
/// the new `start`.
pub fn apply_left_edge_drag(
    orig_start: f64,
    sing_end: f64,
    delta_secs: f64,
    bounds: DragBounds,
) -> f64 {
    let max_start = sing_end - MIN_DURATION;
    (orig_start + delta_secs).clamp(bounds.min_start, max_start.max(bounds.min_start))
}

/// Trim the right edge by `delta_secs`, keeping `start` fixed. Returns the
/// new `sing_end`.
pub fn apply_right_edge_drag(
    start: f64,
    orig_sing_end: f64,
    delta_secs: f64,
    bounds: DragBounds,
) -> f64 {
    let min_sing_end = start + MIN_DURATION;
    (orig_sing_end + delta_secs).clamp(min_sing_end.min(bounds.max_sing_end), bounds.max_sing_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_to_x_and_back_round_trip() {
        let view = View {
            px_per_sec: 50.0,
            scroll_secs: 10.0,
        };
        let x = view.time_to_x(100.0, 12.5);
        let t = view.x_to_time(100.0, x);
        assert!((t - 12.5).abs() < 1e-4);
    }

    #[test]
    fn zoom_at_keeps_anchor_time_under_the_same_pixel() {
        let mut view = View {
            px_per_sec: 40.0,
            scroll_secs: 5.0,
        };
        let area_left = 20.0;
        let anchor_time = 8.0;
        let anchor_x_before = view.time_to_x(area_left, anchor_time);
        view.zoom_at(2.0, area_left, anchor_time);
        let anchor_x_after = view.time_to_x(area_left, anchor_time);
        assert!((anchor_x_before - anchor_x_after).abs() < 0.01);
        assert_eq!(view.px_per_sec, 80.0);
    }

    #[test]
    fn zoom_is_clamped_to_bounds() {
        let mut view = View {
            px_per_sec: MAX_PX_PER_SEC,
            scroll_secs: 0.0,
        };
        view.zoom_at(10.0, 0.0, 0.0);
        assert_eq!(view.px_per_sec, MAX_PX_PER_SEC);

        view.px_per_sec = MIN_PX_PER_SEC;
        view.zoom_at(0.01, 0.0, 0.0);
        assert_eq!(view.px_per_sec, MIN_PX_PER_SEC);
    }

    #[test]
    fn scroll_never_goes_negative() {
        let mut view = View {
            px_per_sec: 40.0,
            scroll_secs: 1.0,
        };
        // Dragging content far to the right pushes scroll well past t=0.
        view.pan_by_pixels(1000.0);
        assert_eq!(view.scroll_secs, 0.0);
    }

    #[test]
    fn tick_interval_grows_as_you_zoom_out() {
        let zoomed_in = tick_interval_secs(200.0, 50.0);
        let zoomed_out = tick_interval_secs(5.0, 50.0);
        assert!(zoomed_out > zoomed_in);
        // Every candidate must actually satisfy the minimum spacing.
        assert!(zoomed_in as f32 * 200.0 >= 50.0);
    }

    #[test]
    fn classify_drag_picks_edges_near_the_boundary() {
        assert_eq!(classify_drag(0.0, 100.0), DragMode::LeftEdge);
        assert_eq!(classify_drag(3.0, 100.0), DragMode::LeftEdge);
        assert_eq!(classify_drag(100.0, 100.0), DragMode::RightEdge);
        assert_eq!(classify_drag(95.0, 100.0), DragMode::RightEdge);
        assert_eq!(classify_drag(50.0, 100.0), DragMode::Body);
    }

    #[test]
    fn classify_drag_on_a_narrow_bubble_prefers_an_edge() {
        // A bubble narrower than 2*EDGE_GRAB_PX has no "body" region at all -
        // every point should resolve to whichever edge it's closer to,
        // rather than the left edge's threshold swallowing the whole thing.
        let width = EDGE_GRAB_PX; // narrower than 2*EDGE_GRAB_PX
        assert_eq!(classify_drag(0.0, width), DragMode::LeftEdge);
        assert_eq!(classify_drag(width, width), DragMode::RightEdge);
        assert_ne!(
            classify_drag(0.0, width),
            classify_drag(width, width),
            "the two ends of a narrow bubble must resolve to different edges"
        );
    }

    #[test]
    fn body_drag_preserves_duration_and_respects_bounds() {
        let bounds = DragBounds {
            min_start: 0.0,
            max_sing_end: 100.0,
        };
        let (start, sing_end) = apply_body_drag(10.0, 13.0, 5.0, bounds);
        assert_eq!(start, 15.0);
        assert_eq!(sing_end, 18.0);
        assert!((sing_end - start - 3.0).abs() < 1e-9);
    }

    #[test]
    fn body_drag_cannot_cross_previous_or_next_neighbor() {
        let bounds = DragBounds {
            min_start: 10.0,
            max_sing_end: 20.0,
        };
        // Try to drag far left of the previous neighbor's boundary.
        let (start, sing_end) = apply_body_drag(12.0, 15.0, -100.0, bounds);
        assert_eq!(start, 10.0);
        assert_eq!(sing_end, 13.0); // duration (3.0) preserved

        // Try to drag far right of the next neighbor's boundary.
        let (start, sing_end) = apply_body_drag(12.0, 15.0, 100.0, bounds);
        assert_eq!(sing_end, 20.0);
        assert_eq!(start, 17.0); // duration (3.0) preserved
    }

    #[test]
    fn left_edge_drag_keeps_sing_end_fixed() {
        let bounds = DragBounds {
            min_start: 0.0,
            max_sing_end: 100.0,
        };
        let new_start = apply_left_edge_drag(10.0, 15.0, 2.0, bounds);
        assert_eq!(new_start, 12.0);
        // Can't push past sing_end.
        let new_start = apply_left_edge_drag(10.0, 15.0, 100.0, bounds);
        assert!(new_start < 15.0);
    }

    #[test]
    fn right_edge_drag_keeps_start_fixed() {
        let bounds = DragBounds {
            min_start: 0.0,
            max_sing_end: 100.0,
        };
        let new_end = apply_right_edge_drag(10.0, 15.0, 3.0, bounds);
        assert_eq!(new_end, 18.0);
        // Can't shrink past start, and can't exceed the next neighbor.
        let new_end = apply_right_edge_drag(10.0, 15.0, -100.0, bounds);
        assert!(new_end > 10.0);
        let new_end = apply_right_edge_drag(10.0, 15.0, 1000.0, bounds);
        assert_eq!(new_end, 100.0);
    }
}
