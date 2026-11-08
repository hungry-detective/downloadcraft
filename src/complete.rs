//! IDM-style standalone "Download complete" dialog.
//!
//! The main app pops this up when a download finishes:
//! `download_craft.exe --complete <id>`. The surface is the card drawn in
//! `complete_window.html` (measured in Chrome): frameless and transparent, so
//! that card *is* the window — its own title bar, the file summary, the
//! Address / "The file saved as" fields, the four actions and the
//! "Don't show this dialog again" opt-out.
//!
//! Unlike the progress window this is a snapshot: the finished item is read
//! once from `queue.json` and never polled, because nothing about a completed
//! transfer changes any more.

use crate::app::pl;
use crate::dm::*;
use crate::fonts::{reg, semi};
use crate::model::{claim_window, config_dir, format_bytes, DownloadItem};
use std::time::{Duration, Instant};

// ================= concept geometry =================
// `complete_window.html` (measured in Chrome, border-box), tightened: the
// concept's generous vertical rhythm left ~20 px of dead space between every
// block, so the card is both narrower and shorter while keeping the same
// proportions.
//   card     460x236    (458x234 content + 2x1px border), bg #1b1b1b, r4
//   header   1..32      check-in-circle 14 at x=13, title 13/600 at x=33,
//                       controls 12, right-aligned (13 off right, 24 apart)
//   content  32..235    padding 15/16, content x 16..444
//   summary  40..72     32px doc icon, "Download complete" 13/600, bytes 10
//   address  82..123    label 10 at y=82, box (16,97,428,26)
//   saved    128..169   label 10 at y=128, box (16,143,428,26)
//   buttons  178..204   four 101x26 buttons, 8px gaps
//   opt-out  212..226   14px checkbox at x=16, 10px label at x=34
const CW: f32 = 460.0;
const CH: f32 = 236.0;
const RAD: f32 = 4.0;

const HDR_H: f32 = 32.0;
/// Vertical centre of the title bar (and of its controls).
const HDR_CY: f32 = 16.0;
const MARK_X: f32 = 13.0;
const MARK_S: f32 = 14.0;
const TITLE_X: f32 = 33.0;
const CTLS_X: [f32; 3] = [387.0, 411.0, 435.0];
const CTL_S: f32 = 12.0;

/// Content gutter and right edge (padding 16 inside the 1px border).
const PAD_X: f32 = 16.0;
const IN_R: f32 = 444.0;
const DOC_S: f32 = 32.0;
const DOC_Y: f32 = 40.0;
/// Left edge of the summary's two text lines (16 + 32 + 12).
const SUM_X: f32 = 60.0;
const SUM_SUB_Y: f32 = 58.0;

/// Field blocks: 10px label, then a 26px box 15px below it (label 11 + gap 4).
const LBL_1_Y: f32 = 82.0;
const LBL_2_Y: f32 = 128.0;
const LBL_BOX_GAP: f32 = 15.0;
const BOX_H: f32 = 26.0;
const BOX_PAD: f32 = 10.0;

const BTN_Y: f32 = 178.0;
const BTN_H: f32 = 26.0;
const BTN_GAP: f32 = 8.0;

const CHK_Y: f32 = 212.0;
const CHK_S: f32 = 14.0;
const CHK_LBL_X: f32 = 34.0;
const CHK_TXT: &str = "Don't show this dialog again";

/// The dialog's own palette (the concept is a neutral dark card, not the
/// app's blue-violet theme): accent, though, is the same #3b82f6.
/// Title-bar control glyphs.

fn at(o: egui::Pos2, x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(o.x + x, o.y + y), egui::vec2(w, h))
}

fn read_item(id: u64) -> Option<DownloadItem> {
    let data = std::fs::read_to_string(config_dir()?.join("queue.json")).ok()?;
    let items: Vec<DownloadItem> = serde_json::from_str(&data).ok()?;
    items.into_iter().find(|d| d.id == id)
}

/// Close bulletproofing: viewport Close is async and has been observed to
/// no-op; back it with a delayed process exit (harmless if the window already
/// closed). Dropping the claim file happens in the OS, so it is safe either
/// way.
fn close_soon(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(400));
        std::process::exit(0);
    });
}

struct CompleteApp {
    item: DownloadItem,
    /// "Don't show this dialog again" (the inverse of ui.json's
    /// `show_complete` preference).
    dont_show: bool,
    placed: bool,
    /// Theme key last applied, so a switch in the main app repaints this
    /// dialog while it stays open.
    theme_seen: String,
    theme_check: Instant,
}

impl CompleteApp {
    fn paint(&mut self, ui: &mut egui::Ui, o: egui::Pos2) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let item = self.item.clone();

        // The card *is* the window.
        p.rect_filled(at(o, 0.0, 0.0, CW, CH), RAD, pl().cg_bg);
        p.rect_stroke(
            at(o, 0.0, 0.0, CW, CH),
            RAD,
            egui::Stroke::new(1.0, pl().cg_bd),
            egui::StrokeKind::Inside,
        );
        self.title_bar(&ctx, ui, &p, o);

        // ---- file summary: 40px doc icon + two lines ----
        // Thin standard-weight outline: the 40px icon scales the 24-unit
        // viewBox, so 2.2 units used to become a chunky 3.7px stroke.
        dm_icon(&p, at(o, PAD_X, DOC_Y, DOC_S, DOC_S), DmIcon::Doc, DM_ACC, 1.35);
        p.text(
            egui::pos2(o.x + SUM_X, o.y + DOC_Y),
            egui::Align2::LEFT_TOP,
            crate::i18n::t("Download complete").to_string(),
            semi(13.0),
            pl().cg_txt,
        );
        p.text(
            egui::pos2(o.x + SUM_X, o.y + SUM_SUB_Y),
            egui::Align2::LEFT_TOP,
            if item.resolution.is_empty() {
                format!(
                    "Downloaded {} ({} Bytes)",
                    format_bytes(item.downloaded_bytes),
                    item.downloaded_bytes
                )
            } else {
                format!(
                    "Downloaded {} ({} Bytes) · {}",
                    format_bytes(item.downloaded_bytes),
                    item.downloaded_bytes,
                    item.resolution
                )
            },
            reg(10.0),
            pl().cg_dim,
        );

        // ---- address + destination ----
        field(&p, o, LBL_1_Y, "Address", &item.url);
        field(
            &p,
            o,
            LBL_2_Y,
            "The file saved as",
            &item.dest_path().to_string_lossy(),
        );

        // ---- actions ----
        let path = item.dest_path();
        let mut acts = vec![("Open", "open"), ("Open folder", "folder"), ("Close", "close")];
        #[cfg(windows)]
        acts.insert(1,("Open with...", "open-with"));
        let bw = (IN_R - PAD_X - BTN_GAP * (acts.len() as f32 - 1.0)) / acts.len() as f32;
        for (i, (label, act)) in acts.iter().enumerate() {
            let r = at(o, PAD_X + i as f32 * (bw + BTN_GAP), BTN_Y, bw, BTN_H);
            let b = ui.interact(r, egui::Id::new(("cw-act", *act)), egui::Sense::click());
            if b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            dlg_btn(&p, r, b.hovered(), label);
            if b.clicked() {
                match *act {
                    "open" => crate::app::open_path(&path, false),
                    "open-with" => open_with(&path),
                    "folder" => crate::app::open_path(&path, true),
                    _ => {}
                }
                // As in IDM, every action ends the dialog: the opened file,
                // picker or folder is its own process and outlives it.
                close_soon(&ctx);
            }
        }

        // ---- opt-out ----
        let cbr = at(o, PAD_X, CHK_Y, CHK_S, CHK_S);
        let lw = p
            .layout_no_wrap(crate::i18n::t(CHK_TXT).to_string(), reg(10.0), pl().cg_dim)
            .size()
            .x;
        let hit = at(
            o,
            PAD_X,
            CHK_Y,
            (CHK_LBL_X - PAD_X) + lw,
            CHK_S,
        );
        let b = ui.interact(hit, egui::Id::new("cw-dontshow"), egui::Sense::click());
        if b.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if self.dont_show {
            p.rect_filled(cbr, 3.0, DM_ACC);
            dm_icon(
                &p,
                egui::Rect::from_center_size(cbr.center(), egui::Vec2::splat(9.0)),
                DmIcon::Check,
                egui::Color32::WHITE,
                2.2,
            );
        } else {
            p.rect_filled(cbr, 3.0, pl().cg_box_bg);
            p.rect_stroke(
                cbr,
                3.0,
                egui::Stroke::new(1.0, pl().cg_btn_bd),
                egui::StrokeKind::Inside,
            );
        }
        p.text(
            egui::pos2(o.x + CHK_LBL_X, cbr.center().y),
            egui::Align2::LEFT_CENTER,
            crate::i18n::t(CHK_TXT).to_string(),
            reg(10.0),
            pl().cg_dim,
        );
        if b.clicked() {
            self.dont_show = !self.dont_show;
            crate::model::ui_pref_set_bool("show_complete", !self.dont_show);
        }
    }

    /// Card title bar: mark, title, the three window controls and the
    /// drag-anywhere strip (the concept's own chrome, so the OS title bar is
    /// off).
    fn title_bar(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        p: &egui::Painter,
        o: egui::Pos2,
    ) {
        let c = egui::pos2(o.x + MARK_X + MARK_S / 2.0, o.y + HDR_CY);
        p.circle_stroke(c, 5.5, egui::Stroke::new(1.3, DM_ACC));
        dm_icon(
            p,
            egui::Rect::from_center_size(c, egui::Vec2::splat(8.0)),
            DmIcon::Check,
            DM_ACC,
            1.6,
        );
        p.text(
            egui::pos2(o.x + TITLE_X, o.y + HDR_CY),
            egui::Align2::LEFT_CENTER,
            crate::i18n::t("Download complete").to_string(),
            semi(13.0),
            pl().cg_txt,
        );
        if ui
            .interact(
                at(o, 0.0, 0.0, CW - 90.0, HDR_H),
                egui::Id::new("cw-header"),
                egui::Sense::click_and_drag(),
            )
            .drag_started()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        // Minimise, the concept's maximise box (nothing to maximise on a
        // fixed-size dialog, so it is drawn but inert) and Close.
        for (i, cx, icon, id, tip) in [
            (0usize, CTLS_X[0], DmIcon::Minimize, "cw-min", "Minimise"),
            (1, CTLS_X[1], DmIcon::Maximize, "cw-max", ""),
            (2, CTLS_X[2], DmIcon::Close, "cw-close", "Close"),
        ] {
            let r = egui::Rect::from_center_size(
                egui::pos2(o.x + cx, o.y + HDR_CY),
                egui::Vec2::splat(CTL_S),
            );
            let b = ui.interact(r.expand(5.0), egui::Id::new(id), egui::Sense::click());
            let b = if tip.is_empty() {
                b
            } else {
                b.on_hover_text(tip)
            };
            let live = i != 1;
            if b.hovered() && live {
                p.rect_filled(r.expand(5.0), 3.0, pl().cg_hover);
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            dm_icon(p, r, icon, if b.hovered() && live { pl().cg_txt } else { pl().cg_ctl }, 2.0);
            if b.clicked() && live {
                if i == 0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                } else {
                    close_soon(ctx);
                }
            }
        }
    }
}

/// One labelled field block: 11px label + a 28px single-line box. The concept
/// wraps (`word-break: break-all`); the card is a fixed height, so long URLs
/// and paths are middle-elided to keep both ends readable.
fn field(p: &egui::Painter, o: egui::Pos2, y: f32, label: &str, value: &str) {
    let label = crate::i18n::t(label);
    p.text(
        egui::pos2(o.x + PAD_X, o.y + y),
        egui::Align2::LEFT_TOP,
        label.to_string(),
        reg(10.0),
        pl().cg_dim,
    );
    let r = at(o, PAD_X, y + LBL_BOX_GAP, IN_R - PAD_X, BOX_H);
    p.rect_filled(r, 3.0, pl().cg_box_bg);
    p.rect_stroke(
        r,
        3.0,
        egui::Stroke::new(1.0, pl().cg_box_bd),
        egui::StrokeKind::Inside,
    );
    let txt = elide_mid(p, value, reg(10.0), r.width() - 2.0 * BOX_PAD);
    // The value is a file name or path, so it is drawn shaped.
    crate::shaped::draw(
        p,
        egui::pos2(r.min.x + BOX_PAD, r.center().y),
        egui::Align2::LEFT_CENTER,
        &txt,
        reg(10.0),
        pl().cg_box_txt,
    );
}

/// The concept's button: outlined box, 11px label.
fn dlg_btn(p: &egui::Painter, r: egui::Rect, hov: bool, label: &str) {
    let label = crate::i18n::t(label);
    p.rect_filled(r, 3.0, if hov { pl().cg_hover } else { pl().cg_box_bg });
    p.rect_stroke(
        r,
        3.0,
        egui::Stroke::new(1.0, if hov { pl().cg_ctl } else { pl().cg_btn_bd }),
        egui::StrokeKind::Inside,
    );
    p.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        label.to_string(),
        reg(10.5),
        pl().cg_txt,
    );
}

/// Windows' own "Open with" picker for one file.
fn open_with(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("rundll32.exe")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(path)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        crate::app::open_path(path, false);
    }
}

/// Entry point for `--complete <id>`.
pub fn run() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let id: u64 = args
        .iter()
        .position(|a| a == "--complete")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("--complete needs <id>");
            std::process::exit(2);
        });
    // One dialog per download: a second spawn (or a Re-download while this one
    // is still up) must not stack duplicates. Windows drops the claim the
    // moment this process ends, so nothing has to clean up after a crash.
    let Some(_slot) = claim_window(&format!("complete_{id}")) else {
        return Ok(());
    };
    // Nothing to announce if the row was removed before we got here.
    let Some(item) = read_item(id) else {
        return Ok(());
    };
    let app = CompleteApp {
        item,
        // The checkbox mirrors the app-wide preference it flips.
        dont_show: !crate::model::ui_pref_bool("show_complete", true),
        placed: false,
        theme_seen: String::new(),
        theme_check: Instant::now(),
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download complete")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([CW, CH])
        .with_resizable(false)
        .with_decorations(crate::platform::system_title_bar())
        .with_transparent(!crate::platform::system_title_bar());
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        crate::model::APP_NAME,
        crate::platform::native_options(viewport),
        Box::new(|cc| {
            // `true`: the whole window is the concept, so Segoe UI also becomes
            // its default proportional face.
            crate::fonts::install(cc, true);
            // Standalone process: follow the theme saved in `ui.json`.
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for CompleteApp {
    /// Transparent and frameless: the concept's rounded card is painted by
    /// hand, so the surface behind it must stay clear.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(text) = crate::i18n::take_font_text() {
            crate::fonts::ensure_text(ctx, &text);
        }
        crate::fonts::ensure_text(ctx, &self.item.filename);
        // Follow theme switches made in the main app while this dialog is up
        // (separate process: ui.json is the only channel).
        if self.theme_check.elapsed() >= Duration::from_secs(1) {
            self.theme_check = Instant::now();
            let theme = crate::app::stored_theme();
            let key = format!("{}:{}", theme, crate::app::theme_is_dark(ctx, &theme));
            if key != self.theme_seen {
                self.theme_seen = key;
                crate::app::apply_theme(ctx, &theme);
            }
        }
        // Come to the front and sit in the middle of the work area: a dialog
        // spawned by a background app must not open behind it, and must not
        // hang over the taskbar.
        if !self.placed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if crate::model::center_window(ctx, egui::vec2(CW, CH)) {
                self.placed = true;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_soon(ctx);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let o = ui.max_rect().min;
                self.paint(ui, o);
            });
    }
}
