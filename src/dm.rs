//! Shared "Compact Blue" design system.
//!
//! `popup_concepts.html` is the source of truth for both IDM-style dialogs —
//! the in-canvas "Add download" modal in `app.rs` and the standalone capture
//! popup in `confirm.rs`. Its card renders 562x253 border-box in Chrome, so
//! every offset below is that layout in egui space: origin = card top-left,
//! card border included, top-to-bottom order
//! header (39) | URL | folder + size | filename | actions.
//!
//! `Downloading gui concepts.html` is the source of truth for the standalone
//! progress window in `progress.rs`; it adds the same palette plus the tokens
//! and glyphs that concept draws (segmented tabs, part table, pause/play).
//!
//! Everything here is pure painting: hit testing stays with the caller (the
//! manager window drives a manual hit table, the popup uses egui responses),
//! so hover state arrives as a plain `bool`.

use crate::app::pl;

// ================= tokens =================

pub const DM_W: f32 = 562.0;
/// Card height with the advanced panel closed — the concept's exact size.
pub const DM_H: f32 = 253.0;

/// The active look's accent (blue in Download Craft's own), and the second
/// colour of its gradient (violet; the flat looks repeat the accent).
#[allow(non_snake_case)]
#[inline]
pub fn DM_ACC() -> egui::Color32 {
    crate::app::pl().acc
}
#[allow(non_snake_case)]
#[inline]
pub fn DM_ACC2() -> egui::Color32 {
    crate::app::pl().acc2
}
pub const DM_OK: egui::Color32 = egui::Color32::from_rgb(0x22, 0xc5, 0x5e);
/// "Connecting..." amber (#eab308).
pub const DM_YEL: egui::Color32 = egui::Color32::from_rgb(0xea, 0xb3, 0x08);
pub const DM_RAD: f32 = 6.0;
/// Field box height including its 1px stroke (the concept's 36px + 2px).
pub const DM_BOX_H: f32 = 38.0;

// Content gutters inside the card's 1px border (concept padding 16 / 14 / 10).
pub const DM_X: f32 = 17.0;
pub const DM_W_IN: f32 = 528.0;
pub const DM_ROW1: f32 = 54.0;
pub const DM_ROW2: f32 = 102.0;
pub const DM_ROW3: f32 = 150.0;
pub const DM_ACT: f32 = 202.0;
pub const DM_PATH_W: f32 = 364.0;
pub const DM_SIZE_W: f32 = 156.0;
pub const DM_FILE_W: f32 = 436.0;

pub fn reg(size: f32) -> egui::FontId {
    crate::fonts::reg(size)
}

pub fn semi(size: f32) -> egui::FontId {
    crate::fonts::semi(size)
}

// ================= primitives =================

/// Design box: #1e293b fill, 1px #334155 border, 6px radius.
pub fn dm_box(p: &egui::Painter, r: egui::Rect) {
    p.rect(
        r,
        DM_RAD,
        pl().dm_line,
        egui::Stroke::new(1.0, pl().dm_box_bd),
        egui::StrokeKind::Inside,
    );
}

/// One concept row: painted box plus a frameless editor inside it (the concept
/// draws its values dim, but a real input reads better in the design's own
/// content colour; unknown text stays `pl().dm_dim` as the hint). `text_r` bounds
/// the editor so in-box icon buttons keep their room. Returns the box rect.
///
/// epaint lays text out one codepoint at a time, so a Bengali file name shows
/// every matra after its consonant and keeps the virama visible - the letters
/// read as broken even though the font is there. When the value needs the
/// shaper the editor is left with what only it can do (caret, selection, IME,
/// clipboard, keyboard) and its own glyphs are made invisible; the run is
/// painted from `shaped` on top, in the text origin egui uses (top-left of the
/// inner rect, no margin).
pub fn dm_field(
    ui: &mut egui::Ui,
    key: &str,
    rect: egui::Rect,
    text_r: f32,
    buf: &mut String,
    hint: &str,
    password: bool,
) -> egui::Rect {
    let hint = crate::i18n::t(hint);
    dm_box(ui.painter(), rect);
    let inner = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 13.0, rect.min.y + 1.0),
        egui::pos2(text_r.max(rect.min.x + 40.0), rect.max.y - 1.0),
    );
    let line = egui::Rect::from_center_size(inner.center(), egui::vec2(inner.width(), 18.0));
    let font = crate::fonts::reg(11.0);
    // A password never needs shaping, and masking a shaped run is not a thing.
    let shape = !password && crate::shaped::needs_shaping(buf);
    let te = egui::TextEdit::singleline(buf)
        .id(egui::Id::new(key))
        .hint_text(hint)
        .password(password)
        .frame(false)
        .margin(egui::Margin::ZERO)
        .font(font.clone())
        .desired_width(line.width());
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        v.override_text_color = Some(if shape {
            egui::Color32::TRANSPARENT
        } else {
            pl().dm_txt
        });
        v.weak_text_color = Some(pl().dm_dim);
        v.selection.bg_fill = egui::Color32::from_rgba_premultiplied(59, 130, 246, 90);
        ui.put(line, te);
    });
    if shape {
        crate::shaped::draw(
            &ui.painter().with_clip_rect(line.intersect(ui.clip_rect())),
            line.min,
            egui::Align2::LEFT_TOP,
            buf,
            font,
            pl().dm_txt,
        );
    }
    rect
}

/// Concept mark: outlined accent circle with a blue download arrow (18px).
pub fn dm_mark(p: &egui::Painter, r: egui::Rect) {
    let s = r.width() / 24.0;
    let m = |x: f32, y: f32| egui::pos2(r.min.x + x * s, r.min.y + y * s);
    let st = egui::Stroke::new(2.0 * s, DM_ACC());
    p.circle_stroke(m(12.0, 12.0), 9.0 * s, st);
    p.line_segment([m(12.0, 8.0), m(12.0, 15.0)], st);
    p.line_segment([m(8.5, 11.5), m(12.0, 15.0)], st);
    p.line_segment([m(12.0, 15.0), m(15.5, 11.5)], st);
}

/// The concept's own stroke icons, traced from its inline SVG (24x24 viewBox).
#[derive(Clone, Copy)]
pub enum DmIcon {
    Minimize,
    Maximize,
    /// Two offset squares — the maximize button while the window is maximized.
    Restore,
    Close,
    Clipboard,
    Folder,
    Chevron,
    Grid,
    Check,
    Refresh,
    Settings,
    // ---- progress concept (Downloading gui concepts.html) ----
    /// Circled "i" — the Info tab.
    Info,
    /// Rising line over a baseline — the Speed tab.
    Chart,
    /// Two rails with knobs — the Settings tab.
    Sliders,
    /// Two filled bars.
    Pause,
    /// Filled right-pointing triangle (resume).
    Play,
    /// Chevron pointing up (the part section's collapse toggle).
    ChevronUp,
    /// Sheet with a folded corner (open the finished file).
    Doc,
}

/// Polyline arc, degrees, screen space (0 = right, 90 = down).
pub fn dm_arc(p: &egui::Painter, c: egui::Pos2, r: f32, a0: f32, a1: f32, st: egui::Stroke) {
    let n = 14;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            egui::pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, st));
}

/// Paint `icon` inside `r`; `w` is the concept's stroke width in viewBox units.
pub fn dm_icon(p: &egui::Painter, r: egui::Rect, icon: DmIcon, color: egui::Color32, w: f32) {
    let s = r.width().min(r.height()) / 24.0;
    let m = |x: f32, y: f32| egui::pos2(r.min.x + x * s, r.min.y + y * s);
    let st = egui::Stroke::new((w * s).max(1.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([m(a.0, a.1), m(b.0, b.1)], st);
    };
    match icon {
        DmIcon::Minimize => line((5.0, 12.0), (19.0, 12.0)),
        DmIcon::Maximize => {
            p.rect_stroke(
                egui::Rect::from_min_size(m(5.0, 5.0), egui::vec2(14.0 * s, 14.0 * s)),
                egui::CornerRadius::same(1),
                st,
                egui::StrokeKind::Inside,
            );
        }
        DmIcon::Restore => {
            // The front sheet, then the one behind it peeking out top-left.
            p.rect_stroke(
                egui::Rect::from_min_size(m(8.0, 8.0), egui::vec2(11.0 * s, 11.0 * s)),
                egui::CornerRadius::same(1),
                st,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                egui::Rect::from_min_size(m(5.0, 5.0), egui::vec2(11.0 * s, 11.0 * s)),
                egui::CornerRadius::same(1),
                st,
                egui::StrokeKind::Inside,
            );
        }
        DmIcon::Close => {
            line((6.0, 6.0), (18.0, 18.0));
            line((18.0, 6.0), (6.0, 18.0));
        }
        DmIcon::Clipboard => {
            p.rect_stroke(
                egui::Rect::from_min_size(m(8.0, 8.0), egui::vec2(12.0 * s, 12.0 * s)),
                egui::CornerRadius::same(2),
                st,
                egui::StrokeKind::Inside,
            );
            line((4.0, 16.0), (4.0, 4.0));
            line((4.0, 4.0), (16.0, 4.0));
        }
        DmIcon::Folder => {
            p.add(egui::Shape::line(
                vec![
                    m(3.0, 7.0),
                    m(6.0, 3.0),
                    m(18.0, 3.0),
                    m(21.0, 7.0),
                    m(21.0, 20.0),
                    m(3.0, 20.0),
                    m(3.0, 7.0),
                ],
                st,
            ));
            line((3.0, 7.0), (21.0, 7.0));
        }
        DmIcon::Chevron => {
            line((6.0, 9.0), (12.0, 15.0));
            line((12.0, 15.0), (18.0, 9.0));
        }
        DmIcon::Grid => {
            // The concept draws these at 30% opacity.
            let g = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 77);
            for (x, y) in [(3.0, 3.0), (14.0, 3.0), (3.0, 14.0), (14.0, 14.0)] {
                p.rect_filled(
                    egui::Rect::from_min_size(m(x, y), egui::vec2(7.0 * s, 7.0 * s)),
                    1.0,
                    g,
                );
            }
        }
        DmIcon::Check => {
            line((5.0, 13.0), (9.0, 17.0));
            line((9.0, 17.0), (19.0, 7.0));
        }
        DmIcon::Refresh => {
            // Two arcs of one circle plus the concept's corner brackets.
            dm_arc(p, r.center(), 8.0 * s, -12.0, -152.0, st);
            dm_arc(p, r.center(), 8.0 * s, 168.0, 28.0, st);
            line((4.0, 4.0), (4.0, 10.0));
            line((4.0, 10.0), (10.0, 10.0));
            line((20.0, 20.0), (20.0, 14.0));
            line((20.0, 14.0), (14.0, 14.0));
        }
        DmIcon::Settings => {
            // The hand-traced "cog" (a ring with eight inward spokes) read as a
            // sunburst at 14 px, not as settings. Use the bundled Material gear
            // glyph instead - it is drawn as a real cog outline.
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                "\u{e8b8}".to_string(),
                crate::fonts::icon(r.width().min(r.height()) * 1.02),
                color,
            );
        }
        DmIcon::Info => {
            p.circle_stroke(m(12.0, 12.0), 9.0 * s, st);
            line((12.0, 10.0), (12.0, 16.0));
            // Traced dot: the concept's near-zero "M12 7v.5" cap.
            p.circle_filled(m(12.0, 7.2), (w * s / 2.0).max(0.7), color);
        }
        DmIcon::Chart => {
            p.add(egui::Shape::line(
                vec![
                    m(3.0, 17.0),
                    m(8.0, 11.0),
                    m(12.0, 14.0),
                    m(18.0, 6.0),
                    m(21.0, 10.0),
                ],
                st,
            ));
            line((3.0, 21.0), (21.0, 21.0));
        }
        DmIcon::Sliders => {
            line((4.0, 8.0), (10.0, 8.0));
            line((18.0, 8.0), (20.0, 8.0));
            line((4.0, 16.0), (6.0, 16.0));
            line((10.0, 16.0), (20.0, 16.0));
            // Knobs: the concept fills them with the trough colour behind the ring.
            for (cx, cy) in [(16.0, 8.0), (8.0, 16.0)] {
                p.circle_filled(m(cx, cy), 2.0 * s, pl().dm_trough);
                p.circle_stroke(m(cx, cy), 2.0 * s, st);
            }
        }
        DmIcon::Pause => {
            for x in [6.0f32, 14.0] {
                p.rect_filled(
                    egui::Rect::from_min_size(m(x, 5.0), egui::vec2(4.0 * s, 14.0 * s)),
                    egui::CornerRadius::same(1),
                    color,
                );
            }
        }
        DmIcon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![m(7.0, 5.0), m(19.0, 12.0), m(7.0, 19.0)],
                color,
                egui::Stroke::NONE,
            ));
        }
        DmIcon::ChevronUp => {
            line((6.0, 15.0), (12.0, 9.0));
            line((12.0, 9.0), (18.0, 15.0));
        }
        DmIcon::Doc => {
            p.add(egui::Shape::line(
                vec![
                    m(14.0, 3.0),
                    m(6.0, 3.0),
                    m(5.0, 4.0),
                    m(5.0, 20.0),
                    m(6.0, 21.0),
                    m(18.0, 21.0),
                    m(19.0, 20.0),
                    m(19.0, 8.0),
                    m(14.0, 3.0),
                ],
                st,
            ));
            p.add(egui::Shape::line(
                vec![m(14.0, 3.0), m(14.0, 8.0), m(19.0, 8.0)],
                st,
            ));
        }
    }
}

/// The Add URL mark: a crisp vector version of the concept's diagonal chain
/// link (two interlocking capsules). The raster source (a low-contrast
/// screenshot) turned to mush when scaled to toolbar size, so this traces the
/// same shape as paths with a bold stroke — solid at any scale, matching the
/// reference mark's weight.
///
/// Kept for other surfaces; the main toolbar now uses the concept's plain
/// plus (see `MI_ADD`) on its filled primary chip.
#[allow(dead_code)]
pub fn add_link(p: &egui::Painter, r: egui::Rect, color: egui::Color32, w: f32) {
    let s = r.width().min(r.height()) / 24.0;
    let st = egui::Stroke::new((w * s).max(1.0), color);
    // Local capsule runs along +x; rotate -45° so the chain runs bottom-left
    // to top-right like the reference mark.
    let a = -std::f32::consts::FRAC_PI_4;
    let (sa, ca) = a.sin_cos();
    let place = |cx: f32, cy: f32, lx: f32, ly: f32| {
        egui::pos2(
            r.min.x + (cx + ca * lx - sa * ly) * s,
            r.min.y + (cy + sa * lx + ca * ly) * s,
        )
    };
    let link = |cx: f32, cy: f32| {
        let hl = 5.7f32; // half-length of the capsule body
        let rad = 2.7f32;
        let straight = (hl - rad).max(0.0);
        let mut local: Vec<(f32, f32)> = vec![(-straight, -rad), (straight, -rad)];
        let arc = |ccx: f32, a0: f32, a1: f32, out: &mut Vec<(f32, f32)>| {
            let n = 6;
            for i in 0..=n {
                let t = a0 + (a1 - a0) * i as f32 / n as f32;
                out.push((ccx + rad * t.cos(), rad * t.sin()));
            }
        };
        arc(straight, -std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2, &mut local);
        local.push((-straight, rad));
        arc(-straight, std::f32::consts::FRAC_PI_2, 3.0 * std::f32::consts::FRAC_PI_2, &mut local);
        let pts: Vec<egui::Pos2> = local
            .into_iter()
            .map(|(lx, ly)| place(cx, cy, lx, ly))
            .collect();
        p.add(egui::Shape::closed_line(pts, st));
    };
    // Two interlocking links offset along the diagonal.
    link(13.9, 8.1);
    link(8.1, 13.9);
}

/// Concept icon button: hover wash plus glyph. Painting only — the caller owns
/// the hit test and passes `hovered`.
pub fn icon_btn(
    p: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    icon: DmIcon,
    size: f32,
    w: f32,
    color: egui::Color32,
) {
    if hovered {
        p.rect_filled(rect, DM_RAD, pl().dm_hover2);
    }
    dm_icon(
        p,
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(size)),
        icon,
        color,
        w,
    );
}

/// Same icon button, but sitting on the field surface: the same fill, border
/// and radius as `dm_box`, so a button beside an input reads as part of the
/// same row instead of a bare glyph.
pub fn icon_btn_box(
    p: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    icon: DmIcon,
    size: f32,
    w: f32,
    color: egui::Color32,
) {
    p.rect(
        rect,
        DM_RAD,
        if hovered { pl().dm_hover2 } else { pl().dm_line },
        egui::Stroke::new(1.0, pl().dm_box_bd),
        egui::StrokeKind::Inside,
    );
    dm_icon(
        p,
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(size)),
        icon,
        color,
        w,
    );
}

/// Outline button (concept "Add" / "Cancel").
pub fn btn_flat(p: &egui::Painter, r: egui::Rect, hov: bool, label: &str) {
    btn_flat_c(p, r, hov, label, if hov { pl().dm_txt } else { pl().dm_dim });
}

/// Same outline button with an explicit label colour (destructive actions).
pub fn btn_flat_c(
    p: &egui::Painter,
    r: egui::Rect,
    hov: bool,
    label: &str,
    col: egui::Color32,
) {
    let label = crate::i18n::t(label);
    if hov {
        p.rect_filled(r, DM_RAD, pl().dm_hover);
    }
    p.rect_stroke(
        r,
        DM_RAD,
        egui::Stroke::new(1.0, pl().dm_box_bd),
        egui::StrokeKind::Inside,
    );
    p.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        label.to_string(),
        crate::fonts::reg(12.0),
        col,
    );
}

/// Fill a rounded rect with the concept's 135 degree accent gradient. egui has
/// no gradient paint, so build a fan mesh over the rounded outline: every
/// vertex is tinted by (x + y), and linear interpolation across the triangles
/// reproduces the diagonal exactly.
pub fn dm_gradient(p: &egui::Painter, r: egui::Rect, rad: f32, a: egui::Color32, b: egui::Color32) {
    grad_mesh(p, r, rad, a, b, true);
}

/// Horizontal (90 degree) cut of the same gradient — the progress bar's fill.
pub fn dm_gradient_x(p: &egui::Painter, r: egui::Rect, rad: f32, a: egui::Color32, b: egui::Color32) {
    grad_mesh(p, r, rad, a, b, false);
}

fn grad_mesh(
    p: &egui::Painter,
    r: egui::Rect,
    rad: f32,
    a: egui::Color32,
    b: egui::Color32,
    diagonal: bool,
) {
    let mix = |t: f32| {
        egui::Color32::from_rgb(
            (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
            (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
            (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
        )
    };
    let span = if diagonal {
        (r.width() + r.height()).max(1.0)
    } else {
        r.width().max(1.0)
    };
    let tint = |q: egui::Pos2| {
        let d = if diagonal {
            (q.x - r.min.x) + (q.y - r.min.y)
        } else {
            q.x - r.min.x
        };
        mix((d / span).clamp(0.0, 1.0))
    };
    let mut pts: Vec<egui::Pos2> = Vec::new();
    for (cx, cy, a0, a1) in [
        (r.right() - rad, r.top() + rad, -90.0f32, 0.0f32),
        (r.right() - rad, r.bottom() - rad, 0.0, 90.0),
        (r.left() + rad, r.bottom() - rad, 90.0, 180.0),
        (r.left() + rad, r.top() + rad, 180.0, 270.0),
    ] {
        for i in 0..=6 {
            let ang = (a0 + (a1 - a0) * i as f32 / 6.0).to_radians();
            pts.push(egui::pos2(cx + rad * ang.cos(), cy + rad * ang.sin()));
        }
    }
    let mut mesh = egui::Mesh::default();
    let c = r.center();
    let ci = mesh.vertices.len() as u32;
    mesh.vertices.push(egui::epaint::Vertex {
        pos: c,
        uv: egui::epaint::WHITE_UV,
        color: tint(c),
    });
    for q in &pts {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *q,
            uv: egui::epaint::WHITE_UV,
            color: tint(*q),
        });
    }
    let first = ci + 1;
    for i in 0..pts.len() as u32 {
        mesh.add_triangle(ci, first + i, first + (i + 1) % pts.len() as u32);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Concept slider painted from explicit track geometry, so the caller's drag
/// handling and the painting can never disagree.
pub fn slider(
    p: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    frac: f32,
    track_x: f32,
    track_w: f32,
) {
    let ty = rect.center().y;
    let track = egui::Rect::from_min_size(egui::pos2(track_x, ty - 2.0), egui::vec2(track_w, 4.0));
    p.rect_filled(track, 2.0, pl().dm_box_bd);
    p.rect_filled(
        egui::Rect::from_min_size(track.min, egui::vec2(track_w * frac, 4.0)),
        2.0,
        DM_ACC(),
    );
    let kx = track_x + track_w * frac;
    if hovered {
        p.circle_filled(
            egui::pos2(kx, ty),
            9.0,
            egui::Color32::from_rgba_unmultiplied(59, 130, 246, 50),
        );
    }
    p.circle_filled(egui::pos2(kx, ty), 6.0, DM_ACC());
}

/// Concept checkbox (16px): accent fill plus a white tick when checked.
pub fn dm_check(p: &egui::Painter, r: egui::Rect, checked: bool) {
    if checked {
        p.rect_filled(r, 4.0, DM_ACC());
        dm_icon(
            p,
            egui::Rect::from_center_size(r.center(), egui::Vec2::splat(12.0)),
            DmIcon::Check,
            egui::Color32::WHITE,
            2.5,
        );
    } else {
        p.rect(
            r,
            4.0,
            pl().dm_card,
            egui::Stroke::new(1.0, pl().dm_box_bd),
            egui::StrokeKind::Inside,
        );
    }
}

/// Recent-folders menu geometry, derived from the caret button so the render
/// and the outside-click test can never disagree. (The caret sits 308px right
/// of the path box's left edge: 364 - 28 - 28.)
pub fn dm_menu_rect(caret: egui::Rect, rows: usize) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(caret.min.x - 308.0, caret.max.y + 6.0),
        egui::vec2(320.0, rows as f32 * 24.0 + 12.0),
    )
}

/// Left-elided path for the recent-folders menu.
pub fn elide_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(n - max + 1).collect::<String>())
    }
}

/// Single line that measures no wider than `max_w`, elided in the middle so a
/// long filename keeps both its head and its extension visible.
///
/// Widths come from `shaped`, so the decision matches what actually gets drawn
/// for scripts egui cannot shape itself, and the cut lands on grapheme
/// boundaries: slicing inside a Bengali cluster (a consonant plus its matra, or
/// a consonant plus virama) would leave a dangling mark on screen.
pub fn elide_mid(
    p: &egui::Painter,
    s: &str,
    font: egui::FontId,
    max_w: f32,
) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let width = |t: &str| crate::shaped::width(p, t, &font);
    if width(s) <= max_w {
        return s.to_string();
    }
    let parts: Vec<&str> = s.graphemes(true).collect();
    let mut keep = parts.len().saturating_sub(1);
    while keep > 3 {
        let head = keep.div_ceil(2);
        let tail = keep - head;
        let cut: String = parts[..head].concat() + "…" + &parts[parts.len() - tail..].concat();
        if width(&cut) <= max_w {
            return cut;
        }
        keep -= 1;
    }
    "…".to_string()
}
