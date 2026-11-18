//! Manual pointer routing: hover/click detection from raw pointer position
//! and press/release events, bypassing egui hit-testing entirely.
//!
//! Only proven-live signals are used: `pointer_hover_pos`,
//! `button_down`, and `pointer_events` (press/release positions).
//! Widget `Response`s are never consulted for input.
//!
//! Protocol per frame:
//! 1. `begin()` snapshots input (call once, at the top of `update`).
//! 2. Layout code records hit rects (background → foreground order).
//! 3. Input is matched against LAST frame's rects is wrong — instead the
//!    caller processes rects recorded THIS frame at the END of layout,
//!    in reverse (topmost first), with click consumption.

use egui::{Pos2, Rect};
use std::time::Instant;

const CLICK_DRIFT: f32 = 12.0;
const DOUBLE_MS: u128 = 500;
const DOUBLE_DIST: f32 = 24.0;

#[derive(Default)]
pub struct ManState {
    pos: Option<Pos2>,
    down_l: bool,
    down_r: bool,
    rose_l: bool,
    rose_r: bool,
    org_l: Option<Pos2>,
    org_r: Option<Pos2>,
    rel_l: Option<Pos2>,
    rel_r: Option<Pos2>,
    /// Where the press released this frame began.
    rel_org_l: Option<Pos2>,
    rel_org_r: Option<Pos2>,
    consumed: bool,
    last_click_at: Option<Instant>,
    last_click_pos: Option<Pos2>,
    double_pending: bool,
    /// How many click/secondary matches fired (proves dispatch path).
    pub fires: u64,
    // key diagnostics (proves keyboard delivery for Phase 2 text fields)
    pub key_count: u64,
    pub text_count: u64,
    pub last_key: String,
}

impl ManState {
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.consumed = false;
        self.rel_l = None;
        self.rel_r = None;
        self.rel_org_l = None;
        self.rel_org_r = None;
        self.rose_l = false;
        self.rose_r = false;
        self.double_pending = false;
        ctx.input(|i| {
            self.pos = i.pointer.hover_pos();
            // Presses and releases come from the input events, in order, with
            // their own positions: a click whose press and release land
            // between two frames still counts (polling the button state once
            // a frame lost such clicks while the window was idle, right-clicks
            // most of all).
            let mut seen_l = false;
            let mut seen_r = false;
            for e in &i.events {
                match e {
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        let (down, org, rel, rel_org, rose, seen) = match button {
                            egui::PointerButton::Primary => (
                                &mut self.down_l,
                                &mut self.org_l,
                                &mut self.rel_l,
                                &mut self.rel_org_l,
                                &mut self.rose_l,
                                &mut seen_l,
                            ),
                            egui::PointerButton::Secondary => (
                                &mut self.down_r,
                                &mut self.org_r,
                                &mut self.rel_r,
                                &mut self.rel_org_r,
                                &mut self.rose_r,
                                &mut seen_r,
                            ),
                            _ => continue,
                        };
                        *seen = true;
                        if *pressed {
                            *down = true;
                            *org = Some(*pos);
                            *rose = true;
                        } else {
                            *down = false;
                            *rel = Some(*pos);
                            *rel_org = org.take().or(Some(*pos));
                        }
                    }
                    // Focus or pointer gone (minimized, parked to the tray,
                    // another window on top): a release may never arrive, and
                    // a button left "held" blocked hover, clicks and drags
                    // until restart (issue #24).
                    egui::Event::WindowFocused(false) | egui::Event::PointerGone => {
                        self.down_l = false;
                        self.down_r = false;
                        self.org_l = None;
                        self.org_r = None;
                    }
                    _ => {}
                }
            }
            // egui's own button state only ever clears ours (a release we
            // missed). It never sets it: egui keeps a button "held" when its
            // release was lost while the window was away, and copying that
            // back is exactly what froze hover, clicks and drags (#24). Every
            // real press arrives as an event above.
            for (button, down, org, seen) in [
                (egui::PointerButton::Primary, &mut self.down_l, &mut self.org_l, seen_l),
                (egui::PointerButton::Secondary, &mut self.down_r, &mut self.org_r, seen_r),
            ] {
                if !seen && *down && !i.pointer.button_down(button) {
                    *down = false;
                    *org = None;
                }
            }
            // keyboard delivery proof (Phase 2 text fields need this)
            for e in &i.events {
                match e {
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => {
                        self.key_count += 1;
                        self.last_key = format!("{key:?}");
                    }
                    egui::Event::Text(_) => {
                        self.text_count += 1;
                    }
                    _ => {}
                }
            }
        });
    }

    pub fn pos(&self) -> Option<Pos2> {
        self.pos
    }

    pub fn down_left(&self) -> bool {
        self.down_l
    }

    #[allow(dead_code)]
    pub fn down(&self) -> bool {
        self.down_l || self.down_r
    }

    /// Hover with no buttons held.
    pub fn hover(&self, r: &Rect) -> bool {
        matches!(self.pos, Some(p) if r.contains(p)) && !self.down_l && !self.down_r
    }

    fn fire(&mut self, r: &Rect, rel: Option<Pos2>, org: Option<Pos2>) -> bool {
        if self.consumed {
            return false;
        }
        // Origin unknown (e.g. press arrived without position): fall back
        // to the release position so a clean release-inside still counts.
        let org = org.or(rel);
        match (rel, org) {
            (Some(q), Some(o))
                if r.contains(q) && r.contains(o) && q.distance(o) <= CLICK_DRIFT =>
            {
                self.consumed = true;
                self.fires += 1;
                true
            }
            _ => false,
        }
    }

    /// Left-click (press + release inside, minimal drift). Also arms
    /// double-click detection, read via `take_double` right after.
    pub fn clicked(&mut self, r: &Rect) -> bool {
        let (rel, org) = (self.rel_l, self.rel_org_l);
        if !self.fire(r, rel, org) {
            return false;
        }
        let now = Instant::now();
        self.double_pending = matches!(
            (self.last_click_at, self.last_click_pos, rel),
            (Some(t), Some(p), Some(q))
                if now.duration_since(t).as_millis() <= DOUBLE_MS && p.distance(q) <= DOUBLE_DIST
        );
        self.last_click_at = Some(now);
        self.last_click_pos = rel;
        true
    }

    /// Consume the armed double-click (call right after `clicked`).
    pub fn take_double(&mut self) -> bool {
        let d = self.double_pending;
        self.double_pending = false;
        d
    }

    /// Right-click.
    pub fn secondary(&mut self, r: &Rect) -> bool {
        let (rel, org) = (self.rel_r, self.rel_org_r);
        self.fire(r, rel, org)
    }

    /// Where the held left press began (None when the button is up).
    pub fn press_origin(&self) -> Option<Pos2> {
        if self.down_l { self.org_l } else { None }
    }

    /// Rising left-press edge inside `r` (for drag starts).
    /// Consumes, so only one drag starts per press.
    pub fn press_started_in(&mut self, r: &Rect) -> bool {
        if self.consumed {
            return false;
        }
        // Where the press happened (the pointer may have moved on since).
        if self.rose_l && matches!(self.org_l.or(self.pos), Some(p) if r.contains(p)) {
            self.consumed = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of `events` through a real egui context, then `begin`.
    fn frame(ctx: &egui::Context, man: &mut ManState, events: Vec<egui::Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| man.begin(ctx));
    }

    fn button(pos: Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button, pressed, modifiers: Default::default() }
    }

    const ROW: Rect = Rect { min: Pos2 { x: 100.0, y: 100.0 }, max: Pos2 { x: 300.0, y: 130.0 } };

    #[test]
    fn a_click_inside_one_frame_counts() {
        let (ctx, mut man) = (egui::Context::default(), ManState::default());
        let p = Pos2::new(150.0, 115.0);
        frame(&ctx, &mut man, vec![
            egui::Event::PointerMoved(p),
            button(p, egui::PointerButton::Primary, true),
            button(p, egui::PointerButton::Primary, false),
        ]);
        assert!(man.clicked(&ROW));
    }

    #[test]
    fn a_right_click_inside_one_frame_counts() {
        let (ctx, mut man) = (egui::Context::default(), ManState::default());
        let p = Pos2::new(150.0, 115.0);
        frame(&ctx, &mut man, vec![
            egui::Event::PointerMoved(p),
            button(p, egui::PointerButton::Secondary, true),
            button(p, egui::PointerButton::Secondary, false),
        ]);
        assert!(man.secondary(&ROW));
    }

    #[test]
    fn a_click_over_several_frames_counts_once() {
        let (ctx, mut man) = (egui::Context::default(), ManState::default());
        let p = Pos2::new(150.0, 115.0);
        frame(&ctx, &mut man, vec![egui::Event::PointerMoved(p), button(p, egui::PointerButton::Primary, true)]);
        assert!(!man.clicked(&ROW));
        assert!(man.down_left());
        frame(&ctx, &mut man, vec![]);
        assert!(!man.clicked(&ROW));
        frame(&ctx, &mut man, vec![button(p, egui::PointerButton::Primary, false)]);
        assert!(man.clicked(&ROW));
        frame(&ctx, &mut man, vec![]);
        assert!(!man.clicked(&ROW));
    }

    #[test]
    fn a_release_lost_while_away_does_not_block_input() {
        // Pressed, then the window lost focus (minimized, parked to the tray)
        // and the release never came: hover and the next click still work.
        let (ctx, mut man) = (egui::Context::default(), ManState::default());
        let p = Pos2::new(150.0, 115.0);
        frame(&ctx, &mut man, vec![egui::Event::PointerMoved(p), button(p, egui::PointerButton::Primary, true)]);
        frame(&ctx, &mut man, vec![egui::Event::WindowFocused(false)]);
        frame(&ctx, &mut man, vec![egui::Event::WindowFocused(true), egui::Event::PointerMoved(p)]);
        assert!(man.hover(&ROW), "hover must work again");
        frame(&ctx, &mut man, vec![
            button(p, egui::PointerButton::Primary, true),
            button(p, egui::PointerButton::Primary, false),
        ]);
        assert!(man.clicked(&ROW));
    }

    #[test]
    fn a_press_starts_a_drag_where_it_happened() {
        let (ctx, mut man) = (egui::Context::default(), ManState::default());
        let p = Pos2::new(150.0, 115.0);
        frame(&ctx, &mut man, vec![
            egui::Event::PointerMoved(p),
            button(p, egui::PointerButton::Primary, true),
            egui::Event::PointerMoved(Pos2::new(400.0, 400.0)),
        ]);
        assert!(man.press_started_in(&ROW));
    }
}
