//! Multi-window manager: the three NaVinaMP OS windows ("main" player,
//! "playlist" editor, "library" browser), their persisted geometry,
//! Winamp-style edge snapping + linked dragging, and the queue-event relay
//! between windows.
//!
//! Layout on disk: `<app-config-dir>/windows.json` — `{ label: WinInfo }`
//! with `WinInfo { visible, layout, x, y, w, h }` (logical/CSS px). Saved
//! (debounced) after every geometry/visibility/layout change and flushed
//! synchronously on exit.
//!
//! Structure: [`geom`] is a **pure** integer-math layer (snap detection,
//! link derivation, drag planning, restore clamping) with unit tests; the
//! manager above it is a thin Tauri event plumber.
//!
//! ## Event relay semantics (frozen contract, documented)
//!
//! A frontend window emits `queue-intent` / `queue-state-request` with
//! `from: <own label>` inside the payload (any emit mechanism — plain
//! `emit` or `emitTo`). Rust relays the payload **as-is plus
//! `relayed: true`** to every window except the one matching `from`.
//! Receivers must ignore payloads whose `from` equals their own label
//! (their own echo) — the payload keeps `from` exactly so they can.
//! The `relayed: true` marker stops Rust from re-relaying its own output
//! (backend listeners on `Any` see every emit, including ours).
//!
//! ## Snapping / linked dragging semantics
//!
//! - Any Moved event that is not an echo of our own `set_position` is
//!   treated as a user drag.
//! - The dragged window's **free** position is cursor-anchored (global
//!   cursor position minus the grab offset), so it is independent of the
//!   snap corrections we apply to the window itself.
//! - While an edge of the dragged window is within `SNAP_PX` (8) of
//!   another window's edge (perpendicular overlap required), the dragged
//!   window is pinned to exact alignment via `set_position` and a **link**
//!   is recorded.
//! - Linked partners follow the dragged window's movement delta
//!   (`set_position` per Moved) — a docked cluster moves as one unit.
//! - A link **breaks** when the free position sits more than `BREAK_PX`
//!   (24) beyond the partner's current snap line — in practice: a fast
//!   yank (per-event jump > 24 px) or a partner that failed to follow.
//!   Because lockstep following keeps the deviation near zero, slow
//!   deliberate cluster drags never trip the break; separating windows
//!   requires a quick pull (Winamp-style "yank to undock"). Broken pairs
//!   never re-link during the same drag.
//! - Layout flips, hide/show and resizes clear drag state; links are
//!   re-derived lazily from geometry at the next drag start.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{
    AppHandle, Emitter, EventTarget, Listener, LogicalPosition, LogicalSize, Manager,
    PhysicalPosition, State, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent,
};

use self::geom::{BREAK_PX, DragInput, LINK_EPS_PX, Rect, SNAP_PX};

/// The three window labels, in `WindowsInfo` field order.
pub const LABELS: [&str; 3] = ["main", "playlist", "library"];

/// Persisted-state file name inside the app config dir.
const PERSIST_FILE: &str = "windows.json";

/// A drag whose last Moved event is older than this is considered over
/// (there is no explicit drag-end event) and its state is dropped.
const DRAG_IDLE: Duration = Duration::from_millis(400);

/// How long a `set_position` echo suppression stays valid.
const SUPPRESS_TTL: Duration = Duration::from_millis(400);

/// Tolerance when matching a Moved event against the echo we expect.
const ECHO_TOLERANCE_PX: i32 = 2;

/// Debounce window for the persistence/IO thread: wait for this much
/// quiet after a change, …
const IO_QUIET: Duration = Duration::from_millis(120);
/// …but never longer than this before flushing.
const IO_MAX_LATENCY: Duration = Duration::from_millis(400);

// ---------------------------------------------------------------------------
// Pure geometry
// ---------------------------------------------------------------------------

pub mod geom {
    //! Pure window geometry: rects, snap detection, dock-link derivation,
    //! drag planning and restore clamping. Integer math over logical (CSS)
    //! pixels, no Tauri types — everything here is unit-testable.

    /// Snap activation distance (px). Winamp default.
    pub const SNAP_PX: i32 = 8;
    /// A dock link breaks once the dragged window's free position sits
    /// this far beyond the partner's snap line (px).
    pub const BREAK_PX: i32 = 24;
    /// Edge-alignment tolerance when deriving links from geometry at drag
    /// start (px).
    pub const LINK_EPS_PX: i32 = 2;
    /// Minimum visible sliver for a restored window (px).
    pub const MIN_VISIBLE_W: i32 = 60;
    pub const MIN_VISIBLE_H: i32 = 40;

    /// Axis-aligned rectangle in logical px.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub struct Rect {
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
    }

    impl Rect {
        pub fn at(x: i32, y: i32, w: i32, h: i32) -> Self {
            Self { x, y, w, h }
        }
        pub fn pos(&self) -> (i32, i32) {
            (self.x, self.y)
        }
        pub fn right(&self) -> i32 {
            self.x + self.w
        }
        pub fn bottom(&self) -> i32 {
            self.y + self.h
        }
        pub fn edge(&self, axis: Axis, edge: Edge) -> i32 {
            match (axis, edge) {
                (Axis::X, Edge::Start) => self.x,
                (Axis::X, Edge::End) => self.right(),
                (Axis::Y, Edge::Start) => self.y,
                (Axis::Y, Edge::End) => self.bottom(),
            }
        }
        /// (start, end) range on an axis.
        fn range(&self, axis: Axis) -> (i32, i32) {
            match axis {
                Axis::X => (self.x, self.right()),
                Axis::Y => (self.y, self.bottom()),
            }
        }
        /// Ranges overlap with positive length on `axis`.
        fn overlaps(&self, other: &Rect, axis: Axis) -> bool {
            let (a0, a1) = self.range(axis);
            let (b0, b1) = other.range(axis);
            a0 < b1 && b0 < a1
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Axis {
        X,
        Y,
    }

    impl Axis {
        pub fn idx(self) -> usize {
            match self {
                Axis::X => 0,
                Axis::Y => 1,
            }
        }
        pub fn other(self) -> Axis {
            match self {
                Axis::X => Axis::Y,
                Axis::Y => Axis::X,
            }
        }
    }

    /// Edge of a rect along an axis (left/top = Start, right/bottom = End).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Edge {
        Start,
        End,
    }

    /// An alignment between the dragged window (`a`) and a partner (`b`):
    /// `a.axis[a_edge]` lines up with `b.axis[b_edge]`.
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct Link {
        pub axis: Axis,
        pub a_edge: Edge,
        pub b_edge: Edge,
    }

    impl Link {
        /// Signed distance between the paired edges; 0 = aligned.
        pub fn deviation(&self, dragged: &Rect, partner: &Rect) -> i32 {
            dragged.edge(self.axis, self.a_edge) - partner.edge(self.axis, self.b_edge)
        }
    }

    /// One snap candidate: shifting `moving` by `shift` on `link.axis`
    /// aligns it with window `other` (index into the candidates slice).
    #[derive(Clone, Debug)]
    pub struct SnapDetail {
        pub shift: i32,
        pub other: usize,
        pub link: Link,
    }

    /// Best snap candidate per axis (index 0 = X, 1 = Y): the minimal
    /// non-zero shift (within `snap_px`) aligning any edge pair, requiring
    /// positive overlap on the perpendicular axis (Winamp never snaps
    /// windows that pass each other by).
    pub fn snap_candidates(
        moving: &Rect,
        others: &[Rect],
        snap_px: i32,
    ) -> [Option<SnapDetail>; 2] {
        let mut best: [Option<SnapDetail>; 2] = [None, None];
        for (i, other) in others.iter().enumerate() {
            for axis in [Axis::X, Axis::Y] {
                if !moving.overlaps(other, axis.other()) {
                    continue;
                }
                for a_edge in [Edge::Start, Edge::End] {
                    for b_edge in [Edge::Start, Edge::End] {
                        let shift = other.edge(axis, b_edge) - moving.edge(axis, a_edge);
                        if shift == 0 || shift.abs() > snap_px {
                            continue;
                        }
                        let better = best[axis.idx()]
                            .as_ref()
                            .is_none_or(|b| (shift.abs(), shift) < (b.shift.abs(), b.shift));
                        if better {
                            best[axis.idx()] = Some(SnapDetail {
                                shift,
                                other: i,
                                link: Link {
                                    axis,
                                    a_edge,
                                    b_edge,
                                },
                            });
                        }
                    }
                }
            }
        }
        best
    }

    /// Links between `moving` and every other window already aligned
    /// within `eps` on some axis (with perpendicular overlap) — the
    /// "docked" set that must drag together. At most one link per axis
    /// per pair.
    pub fn derive_links(moving: &Rect, others: &[(String, Rect)], eps: i32) -> Vec<(String, Link)> {
        let mut links = Vec::new();
        for (label, other) in others {
            for axis in [Axis::X, Axis::Y] {
                if !moving.overlaps(other, axis.other()) {
                    continue;
                }
                for a_edge in [Edge::Start, Edge::End] {
                    for b_edge in [Edge::Start, Edge::End] {
                        let link = Link {
                            axis,
                            a_edge,
                            b_edge,
                        };
                        if link.deviation(moving, other).abs() <= eps {
                            links.push((label.clone(), link));
                            break; // one link per axis per pair is enough
                        }
                    }
                }
            }
        }
        links
    }

    /// Everything the drag planner needs about one Moved event.
    pub struct DragInput<'a> {
        /// Free (cursor-anchored) position of the dragged window.
        pub free: (i32, i32),
        /// Position the OS reported for the dragged window.
        pub reported: (i32, i32),
        /// Position the previous event left the dragged window at.
        pub prev_reported: (i32, i32),
        pub size: (i32, i32),
        /// Currently docked partners (from drag start + formed mid-drag).
        pub partners: &'a [(String, Link)],
        /// Partners already broken during this drag (never re-snap).
        pub broken: &'a [String],
        /// The other visible windows.
        pub others: &'a [(String, Rect)],
    }

    /// What to apply after one Moved event.
    #[derive(Default, Debug)]
    pub struct DragPlan {
        /// Pin position for the dragged window (snap / re-align
        /// correction applied); `None` = keep the OS-reported position.
        pub drag_move: Option<(i32, i32)>,
        /// Absolute positions partners must be moved to (linked drag).
        pub partner_moves: Vec<(String, (i32, i32))>,
        /// Links formed by snapping this event.
        pub links_added: Vec<(String, Link)>,
        /// Partners whose link broke this event.
        pub links_broken: Vec<String>,
        /// Where the dragged window ends up (state bookkeeping).
        pub applied: (i32, i32),
    }

    /// Plan the response to one Moved event of a drag.
    ///
    /// 1. **Break**: partners whose alignment the *free* position violates
    ///    by more than `break_px` are dropped (a fast yank — a per-event
    ///    jump past the snap line; or a partner that failed to follow).
    ///    Because lockstep following keeps the free-vs-partner deviation
    ///    near zero for smooth drags, slow cluster moves never break and
    ///    separating windows requires a quick pull (Winamp-style yank).
    ///    Broken pairs never re-link during the same drag.
    /// 2. **Snap**: unlinked, unbroken others within `snap_px` are pinned
    ///    to exact alignment (edge pairs incl. adjacency, perpendicular
    ///    overlap required), forming links. The pin is applied once, at
    ///    formation; afterwards the partner's delta-following preserves
    ///    alignment by construction, so the planner never fights the OS
    ///    drag loop (which on Windows re-anchors to the cursor anyway).
    /// 3. **Follow**: surviving partners move by the dragged window's
    ///    movement delta (including snap corrections, so a docked cluster
    ///    stays rigid when it picks up a new member mid-drag).
    pub fn drag_step(input: &DragInput, snap_px: i32, break_px: i32) -> DragPlan {
        let reported_rect = Rect::at(
            input.reported.0,
            input.reported.1,
            input.size.0,
            input.size.1,
        );
        let free_rect = Rect::at(input.free.0, input.free.1, input.size.0, input.size.1);
        let others = |label: &str| {
            input
                .others
                .iter()
                .find(|(l, _)| l == label)
                .map(|(_, r)| *r)
        };

        // 1. Break pass — judged against the free (cursor-anchored) rect.
        let mut plan = DragPlan::default();
        let mut active: Vec<&(String, Link)> = Vec::new();
        for partner in input.partners {
            let Some(rect) = others(&partner.0) else {
                plan.links_broken.push(partner.0.clone());
                continue;
            };
            if partner.1.deviation(&free_rect, &rect).abs() > break_px {
                plan.links_broken.push(partner.0.clone());
            } else {
                active.push(partner);
            }
        }

        // 2. Snap pass against unlinked, unbroken others.
        let candidates: Vec<(String, Rect)> = input
            .others
            .iter()
            .filter(|(l, _)| {
                !active.iter().any(|(p, _)| p == l) && !input.broken.iter().any(|b| b == l)
            })
            .cloned()
            .collect();
        let candidate_rects: Vec<Rect> = candidates.iter().map(|(_, r)| *r).collect();
        let snaps = snap_candidates(&reported_rect, &candidate_rects, snap_px);
        let mut snap = [0i32, 0i32];
        for (axis_idx, detail) in snaps.iter().enumerate() {
            if let Some(d) = detail {
                snap[axis_idx] = d.shift;
                plan.links_added
                    .push((candidates[d.other].0.clone(), d.link.clone()));
            }
        }

        let applied = (input.reported.0 + snap[0], input.reported.1 + snap[1]);
        plan.applied = applied;
        if snap != [0, 0] {
            plan.drag_move = Some(applied);
        }

        // 3. Partners follow (reported + snap corrections) − prev.
        if !active.is_empty() {
            let delta = (
                input.reported.0 + snap[0] - input.prev_reported.0,
                input.reported.1 + snap[1] - input.prev_reported.1,
            );
            for (label, _) in &active {
                if let Some(rect) = others(label) {
                    plan.partner_moves
                        .push((label.clone(), (rect.x + delta.0, rect.y + delta.1)));
                }
            }
        }

        plan
    }

    /// Clamp a restored rect so a usable sliver of it remains on screen:
    /// if at least `min_w` x `min_h` of it is visible on any screen it is
    /// kept as-is; otherwise it is moved (margin-offset) onto the screen
    /// nearest to its center. An offscreen window must come back visible.
    /// With no known screens the rect is kept as-is (a monitor query
    /// failure must not teleport windows).
    pub fn clamp_to_screens(rect: Rect, screens: &[Rect], min_w: i32, min_h: i32) -> Rect {
        if screens.is_empty() {
            return rect;
        }
        let visible = |s: &Rect| {
            let vw = rect.right().min(s.right()) - rect.x.max(s.x);
            let vh = rect.bottom().min(s.bottom()) - rect.y.max(s.y);
            vw >= min_w.min(rect.w) && vh >= min_h.min(rect.h)
        };
        if screens.iter().any(visible) {
            return rect;
        }
        let distance = |s: &Rect| -> i64 {
            let dx = (s.x + s.w / 2 - (rect.x + rect.w / 2)) as i64;
            let dy = (s.y + s.h / 2 - (rect.y + rect.h / 2)) as i64;
            dx * dx + dy * dy
        };
        let target = screens
            .iter()
            .min_by_key(|s| distance(s))
            .copied()
            .unwrap_or_default();
        Rect {
            x: target.x + 40,
            y: target.y + 40,
            ..rect
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
            Rect::at(x, y, w, h)
        }

        #[test]
        fn snap_adjacency_right_to_left() {
            // moving ends 5px left of other's left edge -> snap right edge to it
            let moving = rect(0, 0, 100, 50);
            let other = rect(105, 10, 100, 50);
            let [dx, _] = snap_candidates(&moving, &[other], SNAP_PX);
            let dx = dx.expect("x snap");
            assert_eq!(dx.shift, 5);
            assert_eq!(dx.link.a_edge, Edge::End);
            assert_eq!(dx.link.b_edge, Edge::Start);
        }

        #[test]
        fn snap_alignment_left_to_left() {
            let moving = rect(3, 0, 100, 50);
            let other = rect(0, 10, 100, 50);
            let [dx, dy] = snap_candidates(&moving, &[other], SNAP_PX);
            let dx = dx.expect("x snap");
            assert_eq!(dx.shift, -3);
            assert!(dy.is_none(), "top vs top+10 is 10px > snap");
        }

        #[test]
        fn snap_corner_picks_min_per_axis() {
            // 5px off on x (right-right) and 2px off on y (top-top)
            let moving = rect(5, 2, 100, 50);
            let other = rect(0, 0, 120, 60);
            let [dx, dy] = snap_candidates(&moving, &[other], SNAP_PX);
            assert_eq!(dx.expect("x snap").shift, -5);
            assert_eq!(dy.expect("y snap").shift, -2);
        }

        #[test]
        fn snap_needs_perpendicular_overlap() {
            // edges within 5px on x, but no y overlap at all
            let moving = rect(0, 0, 100, 50);
            let other = rect(95, 100, 100, 50);
            let [dx, dy] = snap_candidates(&moving, &[other], SNAP_PX);
            assert!(dx.is_none());
            assert!(dy.is_none());
        }

        #[test]
        fn snap_ignores_far_and_exact() {
            let moving = rect(0, 0, 100, 50);
            let far = rect(500, 0, 100, 50);
            let exact = rect(0, 10, 100, 50); // shift 0 == already aligned
            let [dx, _] = snap_candidates(&moving, &[far, exact], SNAP_PX);
            assert!(dx.is_none());
        }

        #[test]
        fn derive_links_docked_pair() {
            let player = rect(0, 0, 275, 280);
            let playlist = rect(0, 280, 275, 232); // directly below, touching
            let links = derive_links(&player, &[("playlist".into(), playlist)], LINK_EPS_PX);
            assert!(links.iter().any(|(l, k)| l == "playlist"
                && k.axis == Axis::Y
                && k.a_edge == Edge::End
                && k.b_edge == Edge::Start));
        }

        #[test]
        fn derive_links_requires_alignment() {
            let player = rect(0, 0, 275, 280);
            let floating = rect(300, 15, 275, 232);
            assert!(derive_links(&player, &[("x".into(), floating)], LINK_EPS_PX).is_empty());
        }

        #[test]
        fn derive_links_aligned_but_disjoint_is_not_docked() {
            // same top, but horizontally disjoint (no x overlap)
            let a = rect(0, 0, 100, 50);
            let b = rect(300, 0, 100, 50);
            assert!(derive_links(&a, &[("b".into(), b)], LINK_EPS_PX).is_empty());
        }

        fn drag_input<'a>(
            reported: (i32, i32),
            prev: (i32, i32),
            partners: &'a [(String, Link)],
            others: &'a [(String, Rect)],
        ) -> DragInput<'a> {
            DragInput {
                free: reported,
                reported,
                prev_reported: prev,
                size: (100, 50),
                partners,
                broken: &[],
                others,
            }
        }

        #[test]
        fn drag_far_away_is_free() {
            let others = vec![("b".to_string(), rect(1000, 1000, 100, 50))];
            let plan = drag_step(
                &drag_input((10, 10), (5, 10), &[], &others),
                SNAP_PX,
                BREAK_PX,
            );
            assert!(plan.drag_move.is_none());
            assert!(plan.partner_moves.is_empty());
            assert!(plan.links_added.is_empty());
            assert_eq!(plan.applied, (10, 10));
        }

        #[test]
        fn drag_snaps_and_links() {
            let others = vec![("b".to_string(), rect(104, 0, 100, 50))];
            let plan = drag_step(&drag_input((0, 0), (0, 0), &[], &others), SNAP_PX, BREAK_PX);
            assert_eq!(plan.drag_move, Some((4, 0)));
            assert_eq!(plan.links_added.len(), 1);
            assert_eq!(plan.links_added[0].0, "b");
            assert_eq!(plan.applied, (4, 0));
            assert!(
                plan.partner_moves.is_empty(),
                "snapping must not move the partner"
            );
        }

        #[test]
        fn drag_moves_docked_partner_by_delta() {
            let link = Link {
                axis: Axis::X,
                a_edge: Edge::End,
                b_edge: Edge::Start,
            };
            let partners = vec![("b".to_string(), link)];
            let others = vec![("b".to_string(), rect(110, 0, 100, 50))];
            // dragged window moved from (0,0) to (10,4), right edge still
            // exactly on b's left edge
            let plan = drag_step(
                &drag_input((10, 4), (0, 0), &partners, &others),
                SNAP_PX,
                BREAK_PX,
            );
            assert!(plan.drag_move.is_none(), "already aligned, no correction");
            assert_eq!(plan.partner_moves, vec![("b".to_string(), (120, 4))]);
        }

        #[test]
        fn drag_never_fights_the_os_when_linked() {
            // linked window reported 5px of OS-side drift past the line:
            // no re-pin (partner following preserves alignment by
            // construction); the partner simply follows the delta
            let link = Link {
                axis: Axis::X,
                a_edge: Edge::End,
                b_edge: Edge::Start,
            };
            let partners = vec![("b".to_string(), link)];
            let others = vec![("b".to_string(), rect(100, 0, 100, 50))];
            let plan = drag_step(
                &drag_input((5, 0), (0, 0), &partners, &others),
                SNAP_PX,
                BREAK_PX,
            );
            assert!(
                plan.drag_move.is_none(),
                "linked: window runs with the cursor"
            );
            assert_eq!(plan.partner_moves, vec![("b".to_string(), (105, 0))]);
            assert_eq!(plan.applied, (5, 0));
        }

        #[test]
        fn drag_breaks_when_yanked() {
            let link = Link {
                axis: Axis::X,
                a_edge: Edge::End,
                b_edge: Edge::Start,
            };
            let partners = vec![("b".to_string(), link)];
            let others = vec![("b".to_string(), rect(100, 0, 100, 50))];
            // free position jumped 30px past the line in one event
            let mut input = drag_input((30, 0), (0, 0), &partners, &others);
            input.free = (30, 0);
            let plan = drag_step(&input, SNAP_PX, BREAK_PX);
            assert_eq!(plan.links_broken, vec!["b".to_string()]);
            assert!(plan.partner_moves.is_empty());
            assert!(plan.drag_move.is_none(), "broken -> window runs free");
            assert_eq!(plan.applied, (30, 0));
        }

        #[test]
        fn drag_slow_cluster_move_does_not_break() {
            let link = Link {
                axis: Axis::Y,
                a_edge: Edge::End,
                b_edge: Edge::Start,
            };
            let partners = vec![("b".to_string(), link)];
            let mut others = vec![("b".to_string(), rect(0, 100, 100, 50))];
            // several small deltas: partner follows each event, deviation
            // stays within the break threshold -> cluster stays glued
            let mut prev = (0, 50);
            for step in 1..10 {
                let reported = (0, 50 + step * 10);
                let mut input = drag_input(reported, prev, &partners, &others);
                input.free = reported;
                let plan = drag_step(&input, SNAP_PX, BREAK_PX);
                assert!(plan.links_broken.is_empty(), "step {step} must not break");
                assert_eq!(plan.partner_moves.len(), 1);
                assert_eq!(
                    plan.partner_moves[0].1.1,
                    100 + step * 10,
                    "partner follows in lockstep"
                );
                // emulate the manager: apply the move so the next event
                // sees the partner where we put it
                others[0].1.y = plan.partner_moves[0].1.1;
                prev = reported;
            }
        }

        #[test]
        fn drag_snap_correction_is_passed_to_partners() {
            // a is docked to b (aligned tops); while dragging it also snaps
            // to c -> b must follow the snap correction to stay docked
            let link = Link {
                axis: Axis::Y,
                a_edge: Edge::Start,
                b_edge: Edge::Start,
            };
            let partners = vec![("b".to_string(), link)];
            let others = vec![
                ("b".to_string(), rect(0, 0, 100, 50)),
                ("c".to_string(), rect(103, 0, 100, 50)),
            ];
            // reported moved to (2, 0): x is 1px off c's left (103-100-2=1)
            let plan = drag_step(
                &drag_input((2, 0), (0, 0), &partners, &others),
                SNAP_PX,
                BREAK_PX,
            );
            assert_eq!(plan.drag_move, Some((3, 0)));
            assert_eq!(plan.links_added[0].0, "c");
            assert_eq!(
                plan.partner_moves,
                vec![("b".to_string(), (3, 0))],
                "b follows a's snap correction"
            );
        }

        #[test]
        fn clamp_keeps_partially_visible_rect() {
            let screens = [rect(0, 0, 1000, 800)];
            let r = clamp_to_screens(rect(940, 100, 100, 50), &screens, 60, 40);
            assert_eq!(r, rect(940, 100, 100, 50), "60px sliver meets the minimum");
        }

        #[test]
        fn clamp_pulls_offscreen_rect_back() {
            let screens = [rect(0, 0, 1000, 800)];
            let r = clamp_to_screens(rect(2000, 100, 100, 50), &screens, 60, 40);
            assert_eq!(r, rect(40, 40, 100, 50));
        }

        #[test]
        fn clamp_uses_nearest_screen() {
            let screens = [rect(0, 0, 1000, 800), rect(1000, 0, 1000, 800)];
            let r = clamp_to_screens(rect(2500, -500, 100, 50), &screens, 60, 40);
            assert_eq!(r, rect(1040, 40, 100, 50), "nearest = right screen");
        }

        #[test]
        fn clamp_handles_rect_larger_than_screen() {
            let screens = [rect(0, 0, 800, 600)];
            let r = clamp_to_screens(rect(5000, 5000, 2000, 2000), &screens, 60, 40);
            assert_eq!(r, rect(40, 40, 2000, 2000));
        }

        #[test]
        fn clamp_without_screens_is_noop() {
            let r = clamp_to_screens(rect(-500, -500, 100, 50), &[], 60, 40);
            assert_eq!(r, rect(-500, -500, 100, 50));
        }
    }
}

// ---------------------------------------------------------------------------
// Contract types
// ---------------------------------------------------------------------------

/// Window layout mode: "normal" (full) or "shade" (collapsed bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Normal,
    Shade,
}

/// Per-window state exposed to the frontend (camelCase) — doubles as the
/// persisted format in `windows.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinInfo {
    pub visible: bool,
    pub layout: Layout,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl WinInfo {
    fn rect(&self) -> Rect {
        Rect::at(self.x, self.y, self.w, self.h)
    }
}

/// `windows_get()` result: always all three windows, field names are the
/// window labels.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct WindowsInfo {
    pub main: WinInfo,
    pub playlist: WinInfo,
    pub library: WinInfo,
}

/// Payload of the `windows-changed` event.
#[derive(Serialize, Clone, Copy)]
struct WindowsChanged {
    windows: WindowsInfo,
}

/// Internal per-window state (the rect is logical px).
#[derive(Debug, Clone)]
struct WinState {
    visible: bool,
    layout: Layout,
    rect: Rect,
}

impl WinState {
    fn info(&self) -> WinInfo {
        WinInfo {
            visible: self.visible,
            layout: self.layout,
            x: self.rect.x,
            y: self.rect.y,
            w: self.rect.w,
            h: self.rect.h,
        }
    }
}

/// Initial window geometry (logical px): Winamp-ish default stack with the
/// playlist docked right below the player; the frontend corrects sizes via
/// `window_set_size` once its content mounts.
fn default_state(label: &str) -> WinState {
    let (rect, visible) = match label {
        "main" => (Rect::at(60, 60, 275, 280), true),
        "playlist" => (Rect::at(60, 340, 275, 232), true),
        _ => (Rect::at(120, 120, 900, 620), false),
    };
    WinState {
        visible,
        layout: Layout::Normal,
        rect,
    }
}

/// Merge persisted entries into default state, clamping every restored
/// rect into the current virtual screen area. Unknown labels and
/// malformed entries are dropped. (Pure; unit-tested.)
fn merged_defaults(
    persisted: &HashMap<String, WinInfo>,
    screens: &[Rect],
) -> BTreeMap<String, WinState> {
    LABELS
        .iter()
        .map(|label| {
            let mut state = default_state(label);
            if let Some(saved) = persisted.get(*label) {
                state.visible = saved.visible;
                state.layout = saved.layout;
                state.rect = geom::clamp_to_screens(
                    saved.rect(),
                    screens,
                    geom::MIN_VISIBLE_W,
                    geom::MIN_VISIBLE_H,
                );
            }
            (label.to_string(), state)
        })
        .collect()
}

/// Parse the persisted file tolerantly: a single bad entry must not wipe
/// the whole layout. (Pure; unit-tested.)
fn parse_persisted(json: &str) -> HashMap<String, WinInfo> {
    let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(json) else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(label, v)| {
            serde_json::from_value::<WinInfo>(v)
                .ok()
                .map(|info| (label, info))
        })
        .collect()
}

/// Serialize the current state to the on-disk format. (Pure; unit-tested.)
fn persist_json(wins: &BTreeMap<String, WinState>) -> String {
    let map: BTreeMap<&str, WinInfo> = LABELS
        .iter()
        .filter_map(|l| wins.get(*l).map(|s| (*l, s.info())))
        .collect();
    serde_json::to_string_pretty(&map).unwrap_or_else(|_| "{}".to_string())
}

fn write_persist(path: &std::path::Path, json: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// Event relay (pure classification + thin emit glue)
// ---------------------------------------------------------------------------

/// What to do with a frontend-emitted relay event.
#[derive(Debug, PartialEq)]
enum RelayAction {
    Ignore,
    Relay { from: String, payload: Value },
}

/// Decide whether a `queue-intent` / `queue-state-request` payload should
/// be relayed: it must be an object with a valid `from` label and must not
/// carry our own `relayed` marker (our echo — otherwise backend listeners
/// on `Any` would re-relay forever). The relayed payload keeps everything
/// (including `from`) and gains `relayed: true`. (Pure; unit-tested.)
fn classify_relay(payload: &str) -> RelayAction {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return RelayAction::Ignore;
    };
    let Some(obj) = value.as_object() else {
        return RelayAction::Ignore;
    };
    if obj.get("relayed").and_then(Value::as_bool) == Some(true) {
        return RelayAction::Ignore;
    }
    let Some(from) = obj.get("from").and_then(Value::as_str) else {
        return RelayAction::Ignore;
    };
    if !LABELS.contains(&from) {
        return RelayAction::Ignore;
    }
    let mut out = obj.clone();
    out.insert("relayed".to_string(), Value::Bool(true));
    RelayAction::Relay {
        from: from.to_string(),
        payload: Value::Object(out),
    }
}

fn targets_label(target: &EventTarget, label: &str) -> bool {
    match target {
        EventTarget::Window { label: l }
        | EventTarget::Webview { label: l }
        | EventTarget::WebviewWindow { label: l }
        | EventTarget::AnyLabel { label: l } => l == label,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Per-window drag bookkeeping.
#[derive(Debug)]
struct DragRuntime {
    /// Free (cursor-anchored) position, immune to our own snap pins.
    free: (i32, i32),
    /// Where the previous event left the dragged window.
    prev_reported: (i32, i32),
    /// cursor − window-origin at drag start (logical px), when the global
    /// cursor position is available.
    grab: Option<(f64, f64)>,
    partners: Vec<(String, geom::Link)>,
    broken: HashSet<String>,
    last_moved: Instant,
}

impl DragRuntime {
    fn new(
        start: (i32, i32),
        size: (i32, i32),
        cursor: Option<(f64, f64)>,
        others: &[(String, Rect)],
    ) -> Self {
        let moving = Rect::at(start.0, start.1, size.0, size.1);
        DragRuntime {
            free: start,
            prev_reported: start,
            grab: cursor.map(|c| (c.0 - start.0 as f64, c.1 - start.1 as f64)),
            partners: geom::derive_links(&moving, others, LINK_EPS_PX),
            broken: HashSet::new(),
            last_moved: Instant::now(),
        }
    }
}

/// Expected echo of a `set_position` we issued (logical px).
#[derive(Debug)]
struct Suppress {
    expected: (i32, i32),
    set_at: Instant,
}

struct Inner {
    wins: BTreeMap<String, WinState>,
    drags: HashMap<String, DragRuntime>,
    suppress: HashMap<String, Suppress>,
    /// Set whenever geometry changed; consumed by the IO thread to decide
    /// whether to broadcast `windows-changed`.
    pending_broadcast: bool,
}

/// Managed multi-window state. All Tauri interaction funnels through here
/// so the geometry itself stays pure (see [`geom`]).
pub struct WindowManager {
    inner: Mutex<Inner>,
    io: Mutex<Option<Sender<()>>>,
}

impl Default for WindowManager {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                wins: LABELS
                    .iter()
                    .map(|l| (l.to_string(), default_state(l)))
                    .collect(),
                drags: HashMap::new(),
                suppress: HashMap::new(),
                pending_broadcast: false,
            }),
            io: Mutex::new(None),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl WindowManager {
    // -- lifecycle ---------------------------------------------------------

    /// Called from the app's `setup` hook: loads persisted geometry,
    /// creates the playlist/library windows, applies state to all three
    /// and starts the persistence/IO thread.
    pub fn init(&self, app: &AppHandle) {
        let screens = logical_screens(app);
        let path = persist_path(app);

        let persisted = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|json| parse_persisted(&json))
            .unwrap_or_default();
        if path.is_none() {
            eprintln!("navinamp windows: cannot resolve config dir; geometry will not persist");
        }

        {
            let mut inner = lock(&self.inner);
            inner.wins = merged_defaults(&persisted, &screens);
        }

        // Create the secondary windows (main comes from tauri.conf.json,
        // hidden; we show everything after applying geometry so nothing
        // flashes at the wrong position).
        for label in LABELS {
            if label == "main" {
                continue;
            }
            if app.get_webview_window(label).is_some() {
                continue;
            }
            let (rect, resizable, title) = match label {
                "playlist" => (
                    Rect::at(60, 340, 275, 232),
                    true,
                    "NaVinaMP — Playlist Editor",
                ),
                _ => (Rect::at(120, 120, 900, 620), true, "NaVinaMP — Media Library"),
            };
            if let Err(e) =
                WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
                    .title(title)
                    .decorations(false)
                    .resizable(resizable)
                    .skip_taskbar(false)
                    .always_on_top(false)
                    .inner_size(rect.w as f64, rect.h as f64)
                    .position(rect.x as f64, rect.y as f64)
                    .visible(false)
                    .build()
            {
                eprintln!("navinamp windows: cannot create '{label}' window: {e}");
            }
        }

        // Apply persisted geometry + visibility to all three windows.
        for label in LABELS {
            let Some(win) = app.get_webview_window(label) else {
                continue;
            };
            let state = {
                let inner = lock(&self.inner);
                inner.wins.get(label).cloned()
            };
            let Some(state) = state else { continue };
            let rect = geom::clamp_to_screens(
                state.rect,
                &screens,
                geom::MIN_VISIBLE_W,
                geom::MIN_VISIBLE_H,
            );
            {
                let mut inner = lock(&self.inner);
                if let Some(st) = inner.wins.get_mut(label) {
                    st.rect = rect;
                }
                record_suppress(&mut inner, label, rect.pos());
            }
            let _ = win.set_position(LogicalPosition::new(rect.x as f64, rect.y as f64));
            let _ = win.set_size(LogicalSize::new(rect.w as f64, rect.h as f64));
            if state.visible {
                let _ = win.show();
            }
        }

        // Relay listeners (queue events between windows).
        for event in ["queue-intent", "queue-state-request"] {
            let handle = app.clone();
            app.listen(event, move |e| {
                if let RelayAction::Relay { from, payload } = classify_relay(e.payload()) {
                    let _ = handle.emit_filter(event, payload, |t| !targets_label(t, &from));
                }
            });
        }

        // Persistence / broadcast thread.
        let (tx, rx) = channel::<()>();
        *lock(&self.io) = Some(tx);
        spawn_io_thread(app.clone(), rx);
    }

    /// Synchronous final save (app exit / player-window close).
    pub fn flush(&self, app: &AppHandle) {
        let json = {
            let inner = lock(&self.inner);
            persist_json(&inner.wins)
        };
        if let Some(path) = persist_path(app)
            && let Err(e) = write_persist(&path, &json)
        {
            eprintln!("navinamp windows: cannot save geometry: {e}");
        }
    }

    // -- commands ------------------------------------------------------------

    /// Current `WindowsInfo`, synced from the OS windows (positions may
    /// drift from our bookkeeping if a move slipped past the event loop).
    pub fn info(&self, app: &AppHandle) -> WindowsInfo {
        {
            let mut inner = lock(&self.inner);
            for label in LABELS {
                let Some(win) = app.get_webview_window(label) else {
                    continue;
                };
                let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
                    continue;
                };
                let scale = win.scale_factor().unwrap_or(1.0);
                let Some(st) = inner.wins.get_mut(label) else {
                    continue;
                };
                st.rect.x = (pos.x as f64 / scale).round() as i32;
                st.rect.y = (pos.y as f64 / scale).round() as i32;
                st.rect.w = (size.width as f64 / scale).round() as i32;
                st.rect.h = (size.height as f64 / scale).round() as i32;
            }
        }
        self.info_from_state()
    }

    fn info_from_state(&self) -> WindowsInfo {
        let inner = lock(&self.inner);
        let get = |label: &str| {
            inner
                .wins
                .get(label)
                .map(WinState::info)
                .unwrap_or_else(|| default_state(label).info())
        };
        WindowsInfo {
            main: get("main"),
            playlist: get("playlist"),
            library: get("library"),
        }
    }

    pub fn set_visible(
        &self,
        app: &AppHandle,
        label: &str,
        visible: bool,
    ) -> Result<WindowsInfo, String> {
        if !LABELS.contains(&label) {
            return Err(format!("unknown window '{label}'"));
        }
        {
            let mut inner = lock(&self.inner);
            let Some(st) = inner.wins.get_mut(label) else {
                return Err(format!("unknown window '{label}'"));
            };
            st.visible = visible;
            invalidate(&mut inner);
            inner.pending_broadcast = true;
        }
        if let Some(win) = app.get_webview_window(label) {
            if visible {
                let _ = win.show();
                let _ = win.set_focus();
            } else {
                let _ = win.hide();
            }
        }
        self.tick();
        Ok(self.info_from_state())
    }

    pub fn toggle_layout(&self, label: &str) -> Result<WindowsInfo, String> {
        {
            let mut inner = lock(&self.inner);
            let Some(st) = inner.wins.get_mut(label) else {
                return Err(format!("unknown window '{label}'"));
            };
            st.layout = match st.layout {
                Layout::Normal => Layout::Shade,
                Layout::Shade => Layout::Normal,
            };
            invalidate(&mut inner);
            inner.pending_broadcast = true;
        }
        self.tick();
        Ok(self.info_from_state())
        // Note: the frontend follows up with window_set_size once its
        // (possibly shade-sized) content has mounted.
    }

    pub fn set_size(&self, app: &AppHandle, label: &str, w: f64, h: f64) -> Result<(), String> {
        if !LABELS.contains(&label) {
            return Err(format!("unknown window '{label}'"));
        }
        let w = w.round() as i32;
        let h = h.round() as i32;
        {
            let mut inner = lock(&self.inner);
            if let Some(st) = inner.wins.get_mut(label) {
                st.rect.w = w;
                st.rect.h = h;
            }
            invalidate(&mut inner);
            inner.pending_broadcast = true;
        }
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.set_size(LogicalSize::new(w as f64, h as f64));
        }
        self.tick();
        Ok(())
    }

    // -- events ---------------------------------------------------------------

    /// Route a `WindowEvent` for one of our windows.
    pub fn on_window_event(&self, window: &Window, event: &WindowEvent) {
        let label = window.label();
        if !LABELS.contains(&label) {
            return;
        }
        let app = window.app_handle();
        match event {
            WindowEvent::CloseRequested { api, .. } => match label {
                "main" => {
                    // Closing the player quits the app (the other windows
                    // only ever hide, so they would keep it alive).
                    self.flush(app);
                    app.exit(0);
                }
                _ => {
                    api.prevent_close();
                    if let Err(e) = self.set_visible(app, label, false) {
                        eprintln!("navinamp windows: hiding '{label}' failed: {e}");
                    }
                }
            },
            WindowEvent::Moved(pos) => self.on_moved(app, label, *pos),
            WindowEvent::Resized(size) => {
                let Some(win) = app.get_webview_window(label) else {
                    return;
                };
                let scale = win.scale_factor().unwrap_or(1.0);
                let w = (size.width as f64 / scale).round() as i32;
                let h = (size.height as f64 / scale).round() as i32;
                let changed = {
                    let mut inner = lock(&self.inner);
                    let Some(st) = inner.wins.get_mut(label) else {
                        return;
                    };
                    let changed = st.rect.w != w || st.rect.h != h;
                    st.rect.w = w;
                    st.rect.h = h;
                    changed
                };
                if changed {
                    self.tick_broadcast();
                }
            }
            _ => {}
        }
    }

    /// One Moved event: suppression check, drag planning, linked partner
    /// moves. Lock is only held for pure bookkeeping; OS calls happen
    /// after release.
    fn on_moved(&self, app: &AppHandle, label: &str, pos: PhysicalPosition<i32>) {
        let Some(win) = app.get_webview_window(label) else {
            return;
        };
        let scale = win.scale_factor().unwrap_or(1.0);
        let logical = (
            (pos.x as f64 / scale).round() as i32,
            (pos.y as f64 / scale).round() as i32,
        );
        let cursor = app
            .cursor_position()
            .ok()
            .map(|c| (c.x / scale, c.y / scale));
        let now = Instant::now();

        let (actions, changed) = {
            let mut inner = lock(&self.inner);

            // Drop drag state whose drag has gone idle (drag end).
            inner
                .drags
                .retain(|_, d| now.duration_since(d.last_moved) < DRAG_IDLE);

            let Some(st) = inner.wins.get(label) else {
                return;
            };
            let size = (st.rect.w, st.rect.h);
            let unchanged = st.rect.x == logical.0 && st.rect.y == logical.1;

            // Echo of a position we set ourselves?
            if let Some(sup) = inner.suppress.get(label) {
                let is_echo = now.duration_since(sup.set_at) <= SUPPRESS_TTL
                    && (logical.0 - sup.expected.0).abs() <= ECHO_TOLERANCE_PX
                    && (logical.1 - sup.expected.1).abs() <= ECHO_TOLERANCE_PX;
                inner.suppress.remove(label);
                if is_echo {
                    if let Some(st) = inner.wins.get_mut(label) {
                        st.rect.x = logical.0;
                        st.rect.y = logical.1;
                    }
                    if let Some(d) = inner.drags.get_mut(label) {
                        d.last_moved = now;
                    }
                    return;
                }
            }

            if unchanged {
                if let Some(d) = inner.drags.get_mut(label) {
                    d.last_moved = now;
                }
                return;
            }

            let others: Vec<(String, Rect)> = inner
                .wins
                .iter()
                .filter(|(l, s)| *l != label && s.visible)
                .map(|(l, s)| (l.clone(), s.rect))
                .collect();

            {
                let drag = inner
                    .drags
                    .entry(label.to_string())
                    .or_insert_with(|| DragRuntime::new(logical, size, cursor, &others));
                drag.last_moved = now;
                let prev = drag.prev_reported;
                // Cursor-anchored free position (falls back to delta
                // tracking when the global cursor position is unavailable).
                drag.free = match (cursor, drag.grab) {
                    (Some(c), Some(g)) => ((c.0 - g.0).round() as i32, (c.1 - g.1).round() as i32),
                    _ => (
                        drag.free.0 + logical.0 - prev.0,
                        drag.free.1 + logical.1 - prev.1,
                    ),
                };
            }
            let (free, prev, partners, broken) = {
                let drag = &inner.drags[label];
                (
                    drag.free,
                    drag.prev_reported,
                    drag.partners.clone(),
                    drag.broken.iter().cloned().collect::<Vec<_>>(),
                )
            };
            let input = DragInput {
                free,
                reported: logical,
                prev_reported: prev,
                size,
                partners: &partners,
                broken: &broken,
                others: &others,
            };
            let plan = geom::drag_step(&input, SNAP_PX, BREAK_PX);

            // Bookkeeping first…
            {
                let drag = inner.drags.get_mut(label).unwrap();
                drag.prev_reported = plan.applied;
                for (l, link) in &plan.links_added {
                    drag.partners.push((l.clone(), link.clone()));
                }
                if !plan.links_broken.is_empty() {
                    drag.broken.extend(plan.links_broken.iter().cloned());
                    drag.partners.retain(|(l, _)| !drag.broken.contains(l));
                }
            }
            if let Some(st) = inner.wins.get_mut(label) {
                st.rect.x = plan.applied.0;
                st.rect.y = plan.applied.1;
            }

            // …then queue the OS moves and their echo suppressions.
            let mut actions: Vec<(String, (i32, i32))> = Vec::new();
            if let Some(pin) = plan.drag_move {
                record_suppress(&mut inner, label, pin);
                actions.push((label.to_string(), pin));
            }
            for (l, target) in &plan.partner_moves {
                if let Some(st) = inner.wins.get_mut(l) {
                    st.rect.x = target.0;
                    st.rect.y = target.1;
                }
                record_suppress(&mut inner, l, *target);
                actions.push((l.clone(), *target));
            }
            (actions, true)
        };

        for (l, (x, y)) in actions {
            if let Some(win) = app.get_webview_window(&l) {
                let _ = win.set_position(LogicalPosition::new(x as f64, y as f64));
            }
        }
        if changed {
            self.tick_broadcast();
        }
    }

    // -- io -----------------------------------------------------------------

    fn tick(&self) {
        if let Some(tx) = lock(&self.io).as_ref() {
            let _ = tx.send(());
        }
    }

    fn tick_broadcast(&self) {
        {
            let mut inner = lock(&self.inner);
            inner.pending_broadcast = true;
        }
        self.tick();
    }
}

/// Clear all drag/suppression state — any geometry invalidation (hide,
/// show, resize, layout flip) makes snap links stale; they re-derive
/// lazily at the next drag start.
fn invalidate(inner: &mut Inner) {
    inner.drags.clear();
    inner.suppress.clear();
}

fn record_suppress(inner: &mut Inner, label: &str, expected: (i32, i32)) {
    inner.suppress.insert(
        label.to_string(),
        Suppress {
            expected,
            set_at: Instant::now(),
        },
    );
}

/// Debounced persistence + `windows-changed` broadcast: coalesce ticks
/// until `IO_QUIET` of quiet (max `IO_MAX_LATENCY`), then write
/// `windows.json` and, if anything visible to the frontend changed,
/// emit the new `WindowsInfo` to all windows.
fn spawn_io_thread(app: AppHandle, rx: Receiver<()>) {
    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            let start = Instant::now();
            loop {
                if start.elapsed() >= IO_MAX_LATENCY {
                    break;
                }
                match rx.recv_timeout(IO_QUIET) {
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            let state = app.state::<WindowManager>();
            let (json, broadcast) = {
                let mut inner = lock(&state.inner);
                let broadcast = inner.pending_broadcast;
                inner.pending_broadcast = false;
                (persist_json(&inner.wins), broadcast)
            };
            if let Some(path) = persist_path(&app)
                && let Err(e) = write_persist(&path, &json)
            {
                eprintln!("navinamp windows: cannot save geometry: {e}");
            }
            if broadcast {
                let info = state.info_from_state();
                let _ = app.emit("windows-changed", WindowsChanged { windows: info });
            }
        }
    });
}

fn persist_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(PERSIST_FILE))
}

/// All monitors as logical-px rects (clamping domain).
fn logical_screens(app: &AppHandle) -> Vec<Rect> {
    let Ok(monitors) = app.available_monitors() else {
        return vec![];
    };
    monitors
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            Rect::at(
                (m.position().x as f64 / scale).round() as i32,
                (m.position().y as f64 / scale).round() as i32,
                (m.size().width as f64 / scale).round() as i32,
                (m.size().height as f64 / scale).round() as i32,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// IPC commands (frozen contract)
// ---------------------------------------------------------------------------

/// Always returns all three windows' geometry/visibility/layout.
#[tauri::command]
pub fn windows_get(app: AppHandle, state: State<WindowManager>) -> WindowsInfo {
    state.info(&app)
}

/// Show/hide a window; hides broadcast `windows-changed`. Closing-hides
/// route here too (`CloseRequested` on playlist/library).
#[tauri::command]
pub fn window_set_visible(
    app: AppHandle,
    state: State<WindowManager>,
    label: String,
    visible: bool,
) -> Result<WindowsInfo, String> {
    state.set_visible(&app, &label, visible)
}

/// Flip a window between "normal" and "shade" layout. The frontend is
/// expected to call [`window_set_size`] right after with the new content
/// size; broadcasts `windows-changed`.
#[tauri::command]
pub fn window_toggle_layout(
    state: State<WindowManager>,
    label: String,
) -> Result<WindowsInfo, String> {
    state.toggle_layout(&label)
}

/// The frontend reports its mounted content size (CSS px == logical px);
/// Rust applies it to the OS window, updates state and broadcasts.
#[tauri::command]
pub fn window_set_size(
    app: AppHandle,
    state: State<WindowManager>,
    label: String,
    w: f64,
    h: f64,
) -> Result<(), String> {
    state.set_size(&app, &label, w, h)
}

/// No-op hook: dragging itself works via the `data-tauri-drag-region`
/// attribute on skinned titlebars (needs `core:window:allow-start-dragging`).
#[tauri::command]
pub fn window_drag_region_ready(_label: String) {}

// ---------------------------------------------------------------------------
// Tests (pure layers)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_valid_payload() {
        let action = classify_relay(r#"{"from":"playlist","intent":"play-index","index":3}"#);
        match action {
            RelayAction::Relay { from, payload } => {
                assert_eq!(from, "playlist");
                assert_eq!(payload["intent"], "play-index");
                assert_eq!(payload["index"], 3);
                assert_eq!(payload["from"], "playlist", "from is kept for receivers");
                assert_eq!(payload["relayed"], true, "echo marker added");
            }
            other => panic!("expected relay, got {other:?}"),
        }
    }

    #[test]
    fn relay_ignores_echo_marker() {
        let action = classify_relay(r#"{"from":"main","relayed":true,"intent":"x"}"#);
        assert_eq!(action, RelayAction::Ignore);
    }

    #[test]
    fn relay_ignores_missing_or_unknown_from() {
        assert_eq!(classify_relay(r#"{"intent":"x"}"#), RelayAction::Ignore);
        assert_eq!(
            classify_relay(r#"{"from":"ghost","intent":"x"}"#),
            RelayAction::Ignore
        );
        assert_eq!(classify_relay("not json"), RelayAction::Ignore);
        assert_eq!(classify_relay(r#"[1,2]"#), RelayAction::Ignore);
    }

    #[test]
    fn persisted_round_trip() {
        let persisted: HashMap<String, WinInfo> = HashMap::from([
            (
                "main".to_string(),
                WinInfo {
                    visible: true,
                    layout: Layout::Shade,
                    x: 10,
                    y: 20,
                    w: 275,
                    h: 14,
                },
            ),
            (
                "playlist".to_string(),
                WinInfo {
                    visible: false,
                    layout: Layout::Normal,
                    x: 10,
                    y: 34,
                    w: 275,
                    h: 232,
                },
            ),
        ]);
        let screens = [Rect::at(0, 0, 1000, 800)];
        let merged = merged_defaults(&persisted, &screens);
        let json = persist_json(&merged);
        let parsed = parse_persisted(&json);
        assert_eq!(parsed.len(), 3, "always all three windows");
        assert_eq!(parsed["main"].layout, Layout::Shade);
        assert_eq!(parsed["main"].x, 10);
        assert!(parsed["library"].visible == parsed.get("library").unwrap().visible);
        // re-merge is idempotent within the screen area
        let merged2 = merged_defaults(&parsed, &screens);
        assert_eq!(merged["main"].info(), merged2["main"].info());
    }

    #[test]
    fn merged_defaults_clamps_offscreen_window() {
        let persisted = HashMap::from([(
            "library".to_string(),
            WinInfo {
                visible: true,
                layout: Layout::Normal,
                x: 5000,
                y: -900,
                w: 900,
                h: 620,
            },
        )]);
        let screens = [Rect::at(0, 0, 1440, 900)];
        let merged = merged_defaults(&persisted, &screens);
        let lib = &merged["library"];
        assert!(lib.rect.x >= 0 && lib.rect.x < 1440);
        assert!(lib.rect.y >= 0 && lib.rect.y < 900);
    }

    #[test]
    fn parse_persisted_drops_bad_entries_only() {
        let json = r#"{
            "main": {"visible": true, "layout": "normal", "x": 1, "y": 2, "w": 3, "h": 4},
            "ghost": {"visible": true, "layout": "normal", "x": 1, "y": 2, "w": 3, "h": 4},
            "broken": {"visible": true}
        }"#;
        let parsed = parse_persisted(json);
        assert!(parsed.contains_key("main"));
        assert!(!parsed.contains_key("broken"));
        // ghost parses fine but merged_defaults ignores unknown labels
        let merged = merged_defaults(&parsed, &[Rect::at(0, 0, 1000, 800)]);
        assert_eq!(merged.len(), 3);
        assert!(!merged.contains_key("ghost"));
    }

    #[test]
    fn win_info_serializes_camel_case() {
        let info = WinInfo {
            visible: true,
            layout: Layout::Shade,
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"visible\""));
        assert!(json.contains("\"layout\":\"shade\""));
        assert!(!json.contains("\"layout\":\"Shade\""));
        let wins = WindowsInfo {
            main: info,
            playlist: info,
            library: info,
        };
        let json = serde_json::to_string(&wins).unwrap();
        assert!(json.contains("\"main\""));
        assert!(json.contains("\"playlist\""));
        assert!(json.contains("\"library\""));
    }

    #[test]
    fn targets_label_matches_label_variants() {
        assert!(targets_label(
            &EventTarget::WebviewWindow {
                label: "main".into()
            },
            "main"
        ));
        assert!(targets_label(
            &EventTarget::AnyLabel {
                label: "main".into()
            },
            "main"
        ));
        assert!(!targets_label(
            &EventTarget::AnyLabel {
                label: "main".into()
            },
            "playlist"
        ));
        assert!(!targets_label(&EventTarget::App, "main"));
    }
}
