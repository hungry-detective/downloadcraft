//! Complex-script text: shaping, glyph fallback and rasterisation.
//!
//! epaint maps one codepoint to one glyph and never applies OpenType shaping.
//! Latin is unaffected by that, but the scripts Windows ships fonts for came out
//! broken in two different ways:
//!
//! * **Shaping.** Bengali (and the other Indic scripts in `Nirmala.ttc`) had the
//!   e/i matras drawn *after* their consonant instead of reordered in front of
//!   it, the virama left visible instead of forming a conjunct and no ligatures
//!   at all - a caption that reads "পোস্টে কমেন্ট" on the page was drawn with
//!   loose letters and dangling hasantas in the app.
//! * **Coverage.** Fallbacks are consulted per glyph, but egui's family list has
//!   no emoji face, so a pictograph in a file name was always the ▯ box, and a
//!   variation selector was a box of its own.
//!
//! So a run that needs either goes through here: rustybuzz shapes it (the shaper
//! HarfBuzz uses), glyphs missing from the first font are picked from the next
//! one that has them, ab_glyph rasterises the result and egui paints it as a
//! texture. Only text that needs it takes this path - everything else keeps
//! egui's own layout, hinting and behaviour, so Latin cannot regress.
//!
//! The drawn colour is applied at paint time (`Painter::image` tint), so one
//! raster serves every theme.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

/// Cap on the raster caches: file names churn, and one texture per distinct
/// (text, size) would grow without bound over a long session.
/// A Bangla interface has hundreds of distinct strings on screen at once:
/// with room for only 256, the cache emptied itself every frame and every
/// label was shaped and rasterised again each frame (the app crawled).
const CACHE_MAX: usize = 1500;
/// Widths are tiny; word wrapping measures many partial lines.
const WIDTH_MAX: usize = 20000;

/// Supersampling factor for the raster.
///
/// egui draws with hinted, gamma-corrected coverage; ab_glyph has neither. At
/// UI sizes (9-12px) a 1:1 raster therefore loses thin strokes and sits much
/// lighter than the text around it - a Bengali file name in the list read as
/// broken next to the "Other" line under it. Rasterising above the device
/// resolution and letting the GPU filter down restores the weight and the
/// small-size legibility.
const SS: f32 = 3.0;

/// Coverage to alpha, with the gamma that brings an unhinted raster up to the
/// weight egui's text has.
fn coverage_to_alpha(cov: f32) -> u8 {
    (cov.clamp(0.0, 1.0).powf(0.72) * 255.0 + 0.5) as u8
}

/// Does this text need the shaper?
///
/// The Indic ranges are exactly what `Nirmala.ttc` carries; the pictograph and
/// variation-selector ranges are the ones no font in egui's chain covers. Every
/// other script (Thai, CJK, Korean, Armenian, Georgian, ...) is drawn by egui
/// itself and must *not* come here, so it keeps the app's typography.
pub fn needs_shaping(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{0900}'..='\u{0DFF}'      // Indic scripts (Nirmala)
            | '\u{1F000}'..='\u{1FAFF}'  // emoji and pictographs
            | '\u{FE00}'..='\u{FE0F}'    // variation selectors
            | '\u{200D}'                 // zero-width joiner inside an emoji
        )
    })
}

/// Characters that carry no glyph of their own anywhere: dropping them keeps a
/// variation selector from becoming a box, and the joiner from splitting a run.
fn is_ignorable(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}')
}

/// Quantised size: the same label at 9.49 and 9.51 must share one raster.
fn qsize(size: f32) -> u32 {
    (size.clamp(0.0, 400.0) * 8.0).round() as u32
}

fn hash_key(text: &str, size: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    qsize(size).hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// Read a system font (per-user copy first), cached for the process lifetime.
/// The bytes are handed out as `&'static` so the parsed faces can borrow them.
fn font_bytes(file: &str) -> Option<&'static [u8]> {
    match file {
        "bundled-bengali" => return Some(include_bytes!("../assets/fonts/NotoSansBengali.ttf")),
        "bundled-emoji" => return Some(include_bytes!("../assets/fonts/NotoEmoji.ttf")),
        _ => {}
    }
    {
        type Cache = std::sync::Mutex<HashMap<String, Option<&'static [u8]>>>;
        static CACHE: OnceLock<Cache> = OnceLock::new();
        let mut cache = CACHE
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = cache.get(file) {
            return *hit;
        }
        let bytes = crate::fonts::font_dirs()
            .iter()
            .find_map(|dir| std::fs::read(dir.join(file)).ok())
            .map(|b| &*Box::leak(b.into_boxed_slice()));
        cache.insert(file.to_owned(), bytes);
        bytes
    }
}

struct Loaded {
    rb: rustybuzz::Face<'static>,
    font: ab_glyph::FontRef<'static>,
}

/// One candidate font, parsed the first time a glyph is actually looked for in
/// it - a plain file name only ever loads the first one.
struct Source {
    file: &'static str,
    index: u32,
    loaded: OnceLock<Option<Loaded>>,
}

impl Source {
    fn get(&self) -> Option<&Loaded> {
        self.loaded
            .get_or_init(|| {
                let bytes = font_bytes(self.file)?;
                let font =
                    ab_glyph::FontRef::try_from_slice_and_index(bytes, self.index).ok()?;
                let rb = rustybuzz::Face::from_slice(bytes, self.index)?;
                Some(Loaded { rb, font })
            })
            .as_ref()
    }

    /// The glyph this font gives `c`, if it has one.
    fn glyph(&self, c: char) -> Option<ab_glyph::GlyphId> {
        use ab_glyph::Font as _;
        let id = self.get()?.font.glyph_id(c);
        (id.0 != 0).then_some(id)
    }
}

/// Fonts consulted for a shaped run, in the same order egui falls back in:
/// Segoe UI first (the face the dialogs are drawn in, so the Latin half of a
/// path keeps the dialog's typography), then the script fonts, then the emoji
/// face - the one family that list never had.
fn sources() -> &'static Vec<Source> {
    static SOURCES: OnceLock<Vec<Source>> = OnceLock::new();
    SOURCES.get_or_init(|| {
        let mut v: Vec<Source> = vec![Source {
            file: if cfg!(windows) { "segoeui.ttf" } else if cfg!(target_os="macos") { "Helvetica.ttc" } else { "DejaVuSans.ttf" },
            index: 0,
            loaded: OnceLock::new(),
        }];
        v.extend(crate::fonts::SYSTEM_FONTS.iter().map(|(_, file, index)| Source {
            file,
            index: *index,
            loaded: OnceLock::new(),
        }));
        v.push(Source {
            file: "seguiemj.ttf",
            index: 0,
            loaded: OnceLock::new(),
        });
        // Portable fallbacks also make shaping independent of optional OS fonts.
        for file in ["bundled-bengali", "bundled-emoji"] {
            v.push(Source { file, index: 0, loaded: OnceLock::new() });
        }
        v
    })
}

/// Index of the first font that has a glyph for `c`.
fn source_for(c: char) -> Option<usize> {
    sources()
        .iter()
        .position(|s| s.glyph(c).is_some())
}

// ---------------------------------------------------------------------------
// Shaping
// ---------------------------------------------------------------------------

/// One shaped glyph, positioned relative to the run's origin (pixels).
struct Placed {
    src: usize,
    id: u16,
    x: f32,
    y: f32,
}

pub(crate) struct Shaped {
    glyphs: Vec<Placed>,
    width: f32,
    ascent: f32,
    descent: f32,
}

/// Shape one run with its own font. Returns the placed glyphs and the advance.
fn shape_piece(src: usize, text: &str, px: f32) -> Option<(Vec<Placed>, f32)> {
    let loaded = sources().get(src)?.get()?;
    let upem = loaded.rb.units_per_em() as f32;
    if !(upem > 0.0) {
        return None;
    }
    let scale = px / upem;
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    buf.guess_segment_properties();
    let out = rustybuzz::shape(&loaded.rb, &[], buf);
    let infos = out.glyph_infos();
    let poss = out.glyph_positions();
    let mut glyphs = Vec::with_capacity(infos.len());
    let mut pen = 0.0f32;
    for (info, pos) in infos.iter().zip(poss.iter()) {
        let advance = pos.x_advance as f32 * scale;
        // glyph 0 is .notdef: the font has no outline for it, so skipping beats
        // stamping a box. Uncovered characters are dropped one step earlier.
        if info.glyph_id != 0 {
            glyphs.push(Placed {
                src,
                id: info.glyph_id as u16,
                x: pen + pos.x_offset as f32 * scale,
                y: -(pos.y_offset as f32) * scale,
            });
        }
        pen += advance;
    }
    Some((glyphs, pen))
}

/// Shape `text`: split it into runs by which font covers each character, shape
/// each run with that font and lay them out one after another.
fn shape_run(text: &str, px: f32) -> Option<Shaped> {
    if sources().is_empty() {
        return None;
    }
    let mut glyphs: Vec<Placed> = Vec::new();
    let mut width = 0.0f32;
    let mut run = String::new();
    let mut run_src: Option<usize> = None;

    for c in text.chars() {
        // Ignorables take their neighbours' font: they have no glyph anywhere.
        let src = if is_ignorable(c) {
            run_src
        } else {
            source_for(c)
        };
        if src != run_src && !run.is_empty() {
            let (mut g, w) = shape_piece(run_src?, &run, px)?;
            for p in &mut g {
                p.x += width;
            }
            glyphs.append(&mut g);
            width += w;
            run.clear();
        }
        run_src = src;
        // A variation selector only asks for a presentation the outline font
        // already gives; it has no shape of its own, so it stays out of the run
        // rather than turning into a stray mark or a box.
        let selector = matches!(c, '\u{FE00}'..='\u{FE0F}');
        if src.is_some() && !selector {
            run.push(c);
        }
    }
    if !run.is_empty() {
        let (mut g, w) = shape_piece(run_src?, &run, px)?;
        for p in &mut g {
            p.x += width;
        }
        glyphs.append(&mut g);
        width += w;
    }
    if glyphs.is_empty() {
        return None;
    }

    // Line metrics come from the first font (the em box egui would use), so a
    // name stays on the same baseline as the text around it.
    let primary = sources()[0].get()?;
    let upem = primary.rb.units_per_em() as f32;
    let scale = px / upem.max(1.0);
    Some(Shaped {
        glyphs,
        width,
        ascent: primary.rb.ascender() as f32 * scale,
        descent: -(primary.rb.descender() as f32 * scale),
    })
}

// ---------------------------------------------------------------------------
// Rastering
// ---------------------------------------------------------------------------

/// A rasterised run: white pixels with the coverage in alpha, plus the size in
/// *device* pixels and where the baseline sits from the top.
struct Raster {
    pixels: Vec<egui::Color32>,
    size: [usize; 2],
    /// Logical size (device pixels / `ppp`).
    logical: egui::Vec2,
}

/// Pixel scale for drawing a glyph of `font` so that its em matches `px`.
///
/// ab_glyph sizes a glyph by the font's *line height*, not by its em: passing
/// `px` straight in drew `Nirmala.ttc`'s glyphs at about 0.75 em (its height is
/// 1.33 em) while the advances rustybuzz reports stayed em-true - so every
/// letter arrived small with a gap after it and a Bengali file name read as
/// broken. Scale by the font's own height/em ratio instead.
fn glyph_scale(font: &ab_glyph::FontRef<'static>, px: f32) -> ab_glyph::PxScale {
    use ab_glyph::Font as _;
    let upem = font.units_per_em().unwrap_or(2048.0);
    let height = font.height_unscaled();
    let factor = if upem > 0.0 && height > 0.0 {
        height / upem
    } else {
        1.0
    };
    ab_glyph::PxScale::from(px * factor)
}

/// Draw the shaped glyphs into a coverage bitmap. No `Context` involved, so the
/// tricky part (glyph fallback + placement) is unit-testable.
fn rasterise(s: &Shaped, ppp: f32, px: f32) -> Option<Raster> {
    use ab_glyph::Font as _;
    // The em box from the primary font, widened by whatever actually gets drawn
    // (an emoji sits lower than a Bengali letter does).
    let mut min_x = 0.0f32;
    let mut min_y = -s.ascent;
    let mut max_x = s.width;
    let mut max_y = s.descent;
    let mut boxes: Vec<Option<ab_glyph::Rect>> = Vec::with_capacity(s.glyphs.len());
    for g in &s.glyphs {
        let font = &sources()[g.src].get()?.font;
        let glyph = ab_glyph::Glyph {
            id: ab_glyph::GlyphId(g.id),
            scale: glyph_scale(font, px),
            position: ab_glyph::point(g.x, g.y),
        };
        let Some(og) = font.outline_glyph(glyph) else {
            // Pushed as `None`, never skipped: this list is zipped with the
            // glyphs below, and a missing entry would shift every later glyph.
            boxes.push(None);
            continue;
        };
        let b = og.px_bounds();
        min_x = min_x.min(b.min.x);
        min_y = min_y.min(b.min.y);
        max_x = max_x.max(b.max.x);
        max_y = max_y.max(b.max.y);
        boxes.push(Some(b));
    }
    let pad = 1.0f32;
    let ox = pad - min_x;
    let oy = pad - min_y;
    let w = (max_x + ox + pad).ceil().max(1.0) as usize;
    let h = (max_y + oy + pad).ceil().max(1.0) as usize;
    if w > 8192 || h > 2048 || w * h > 16_000_000 {
        return None;
    }
    let mut alpha = vec![0u8; w * h];
    for (g, b) in s.glyphs.iter().zip(boxes.iter()) {
        let Some(b) = b else {
            continue;
        };
        let font = &sources()[g.src].get()?.font;
        let glyph = ab_glyph::Glyph {
            id: ab_glyph::GlyphId(g.id),
            scale: glyph_scale(font, px),
            position: ab_glyph::point(g.x, g.y),
        };
        let Some(og) = font.outline_glyph(glyph) else {
            continue;
        };
        // `draw` reports pixels relative to the glyph's own top-left, so the
        // image position is its bounds, shifted into the bitmap.
        let (bx, by) = (b.min.x + ox, b.min.y + oy);
        og.draw(|x, y, cov| {
            if cov <= 0.0 {
                return;
            }
            let ix = (bx + x as f32) as usize;
            let iy = (by + y as f32) as usize;
            if ix >= w || iy >= h {
                return;
            }
            let c = coverage_to_alpha(cov);
            let slot = &mut alpha[iy * w + ix];
            if c > *slot {
                *slot = c;
            }
        });
    }
    let pixels: Vec<egui::Color32> = alpha
        .iter()
        .map(|a| egui::Color32::from_white_alpha(*a))
        .collect();
    Some(Raster {
        pixels,
        size: [w, h],
        logical: egui::vec2(w as f32 / ppp, h as f32 / ppp),
    })
}

struct Entry {
    tex: egui::TextureHandle,
    size: egui::Vec2,
}

thread_local! {
    static CACHE: RefCell<HashMap<(String, u32), Entry>> = RefCell::new(HashMap::new());
    static WIDTHS: RefCell<HashMap<(String, u32), f32>> = RefCell::new(HashMap::new());
    /// Wrapped lines per (text, size, width): a paragraph is broken once.
    static LINES: RefCell<HashMap<(String, u32, u32), Vec<String>>> = RefCell::new(HashMap::new());
}

fn build(ctx: &egui::Context, text: &str, size: f32) -> Option<Entry> {
    let ppp = ctx.pixels_per_point().clamp(1.0, 4.0);
    // Rasterise over the device resolution (see `SS`) and let the GPU filter it
    // down: same on-screen size, much better coverage of thin strokes. Small
    // text needs it most; large text would only cost texture memory.
    let ss = if size <= 16.0 { SS } else { 2.0 };
    let px = (size * ppp * ss).clamp(6.0, 900.0);
    let s = shape_run(text, px)?;
    let r = rasterise(&s, ppp * ss, px)?;
    let image = egui::ColorImage::new(r.size, r.pixels);
    let tex = ctx.load_texture(
        format!("dc-shape-{:016x}", hash_key(text, size)),
        image,
        egui::TextureOptions::LINEAR,
    );
    Some(Entry {
        tex,
        size: r.logical,
    })
}

fn cached(ctx: &egui::Context, text: &str, size: f32) -> Option<(egui::TextureId, egui::Vec2)> {
    let key = (text.to_owned(), qsize(size));
    if let Some(e) = CACHE.with(|c| c.borrow().get(&key).map(|e| (e.tex.id(), e.size))) {
        return Some(e);
    }
    let e = build(ctx, text, size)?;
    let id = e.tex.id();
    let sz = e.size;
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        c.insert(key.clone(), e);
    });
    Some((id, sz))
}

/// Shaped width of `text` at `font`'s size. Falls back to egui's own layout for
/// text that needs no shaping, so elision decisions match what is drawn.
pub fn width(p: &egui::Painter, text: &str, font: &egui::FontId) -> f32 {
    if !needs_shaping(text) {
        return p
            .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x;
    }
    let key = (text.to_owned(), qsize(font.size));
    if let Some(w) = WIDTHS.with(|c| c.borrow().get(&key).copied()) {
        return w;
    }
    let ppp = p.ctx().pixels_per_point().clamp(1.0, 4.0);
    let w = shape_run(text, (font.size * ppp).clamp(6.0, 300.0))
        .map(|s| s.width / ppp)
        .unwrap_or(0.0);
    WIDTHS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= WIDTH_MAX {
            c.clear();
        }
        c.insert(key, w);
    });
    w
}

/// The height of one line of `font` (what egui gives a line of Latin text),
/// so shaped lines stack the way egui's own do.
pub fn line_height(p: &egui::Painter, font: &egui::FontId) -> f32 {
    p.layout_no_wrap("Ag".to_owned(), font.clone(), egui::Color32::WHITE).size().y
}

/// `text` broken into lines no wider than `max_w`, at spaces (a word longer
/// than the line stays whole). Measured with the shaper, so Bangla wraps
/// where it really ends.
pub fn wrap_lines(p: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> Vec<String> {
    let key = (text.to_owned(), qsize(font.size), max_w.round().max(0.0) as u32);
    if let Some(lines) = LINES.with(|c| c.borrow().get(&key).cloned()) {
        return lines;
    }
    let out = wrap_lines_uncached(p, text, font, max_w);
    LINES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    out
}

fn wrap_lines_uncached(p: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && width(p, &candidate, font) > max_w {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line);
    }
    out
}

/// The size `text` takes: one line, or wrapped at `max_w`.
pub fn measure(p: &egui::Painter, text: &str, font: &egui::FontId, max_w: Option<f32>) -> egui::Vec2 {
    if !needs_shaping(text) {
        let g = match max_w {
            Some(w) => p.layout(text.to_owned(), font.clone(), egui::Color32::WHITE, w),
            None => p.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE),
        };
        return g.size();
    }
    let lh = line_height(p, font);
    match max_w {
        Some(w) => {
            let lines = wrap_lines(p, text, font, w);
            let widest = lines.iter().map(|l| width(p, l, font)).fold(0.0_f32, f32::max);
            egui::vec2(widest, lh * lines.len() as f32)
        }
        None => egui::vec2(width(p, text, font), lh),
    }
}

/// Paint `text` with its top-left at `pos`: one line, or wrapped at `max_w`.
/// Shaped where it needs to be, egui's own galley otherwise. Returns its size.
pub fn paint(
    p: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
    max_w: Option<f32>,
) -> egui::Vec2 {
    if !needs_shaping(text) {
        let g = match max_w {
            Some(w) => p.layout(text.to_owned(), font.clone(), color, w),
            None => p.layout_no_wrap(text.to_owned(), font.clone(), color),
        };
        let size = g.size();
        p.galley(pos, g, color);
        return size;
    }
    let lh = line_height(p, font);
    let lines = match max_w {
        Some(w) => wrap_lines(p, text, font, w),
        None => vec![text.to_string()],
    };
    let mut widest = 0.0_f32;
    for (i, l) in lines.iter().enumerate() {
        let y = pos.y + lh * i as f32 + lh / 2.0;
        let s = draw(p, egui::pos2(pos.x, y), egui::Align2::LEFT_CENTER, l, font.clone(), color);
        widest = widest.max(s.x);
    }
    egui::vec2(widest, lh * lines.len() as f32)
}

/// A label in a layout: allocates its size and paints it (wrapped at the
/// width the layout has when `wrap`).
pub fn label(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: egui::Color32, wrap: bool) -> egui::Response {
    let max_w = wrap.then(|| ui.available_width().max(40.0));
    let size = measure(ui.painter(), text, &font, max_w);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint(ui.painter(), rect.min, text, &font, color, max_w);
    resp
}

/// `Painter::text` for every window: text in a script egui cannot shape
/// (Bengali and the other Indic ones, emoji) goes through [`draw`]; anything
/// else is exactly `Painter::text`, so Latin text cannot change. Used for the
/// whole interface, so a Bangla interface reads as it should, not only file
/// names.
pub trait ShapeText {
    fn stext(
        &self,
        pos: egui::Pos2,
        align: egui::Align2,
        text: impl ToString,
        font: egui::FontId,
        color: egui::Color32,
    ) -> egui::Rect;
}

impl ShapeText for egui::Painter {
    fn stext(
        &self,
        pos: egui::Pos2,
        align: egui::Align2,
        text: impl ToString,
        font: egui::FontId,
        color: egui::Color32,
    ) -> egui::Rect {
        let s = text.to_string();
        if needs_shaping(&s) {
            let size = draw(self, pos, align, &s, font, color);
            return align.anchor_size(pos, size);
        }
        self.text(pos, align, s, font, color)
    }
}

/// Drop-in replacement for `Painter::text` that shapes (and font-falls-back)
/// the scripts that need it. Returns the drawn size, like the text it replaces.
pub fn draw(
    p: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) -> egui::Vec2 {
    if needs_shaping(text) {
        if let Some((id, size)) = cached(p.ctx(), text, font.size) {
            let rect = align.anchor_size(pos, size);
            p.image(
                id,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                color,
            );
            return size;
        }
    }
    // No shaping needed, or no font available: egui's own path.
    let galley = p.layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size();
    p.galley(align.anchor_size(pos, size).min, galley, color);
    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font as _;

    /// A file dialog holds a *path*: Latin prefix plus a Bengali name. Both
    /// halves have to come out of one shaper run sequence, the Latin from the
    /// UI's own face (so a field still looks like the dialog around it) and the
    /// Bengali from the Indic one - never a tofu box for either.
    #[test]
    fn a_path_with_a_bengali_name_draws_both_scripts() {
        let path = r"C:\Users\moham\Downloads\Video\বাংলাদেশের রাজধানী.mp4";
        let g = glyphs(path);
        assert!(!g.is_empty(), "nothing shaped");
        assert!(g.iter().all(|x| x.1 != 0), "tofu in {g:?}");
        let used: std::collections::BTreeSet<usize> = g.iter().map(|x| x.0).collect();
        assert!(used.len() >= 2, "expected a Latin and an Indic face, got {used:?}");
        assert!(ink(path) > 0);
    }

    fn glyphs(text: &str) -> Vec<(usize, u16)> {
        shape_run(text, 16.0)
            .map(|s| s.glyphs.iter().map(|g| (g.src, g.id)).collect())
            .unwrap_or_default()
    }

    fn ink(text: &str) -> usize {
        let s = shape_run(text, 16.0).expect("a font for this text");
        let Some(r) = rasterise(&s, 1.0, 16.0) else {
            return 0;
        };
        r.pixels.iter().filter(|p| p.a() > 0).count()
    }

    #[test]
    fn only_text_that_needs_it_takes_the_shaped_path() {
        assert!(needs_shaping("আপনার পোস্টে"));
        assert!(needs_shaping("नमस्ते"));
        assert!(needs_shaping("🎙 voice")); // emoji: no font in egui's chain
        // Thai, CJK, Korean and Arabic are egui's own job (unchanged behaviour).
        assert!(!needs_shaping("สวัสดี"));
        assert!(!needs_shaping("日本語"));
        assert!(!needs_shaping("My Clip 2026.mp4"));
    }

    /// The reason the module exists: Bengali shaping must be real. Without it the
    /// virama survives as a visible glyph and the run keeps one glyph per
    /// codepoint; with it the conjunct swallows the virama.
    #[test]
    fn shaping_forms_conjuncts() {
        // A conjunct can use one ligature or several positioned components,
        // depending on the font. The unshaped virama must disappear either way.
        let ids: Vec<u16> = glyphs("স্ট").into_iter().map(|g| g.1).collect();
        assert!(!ids.is_empty(), "no glyphs from the system font");
        let font = &sources()[source_for('্').expect("Indic font")].get().unwrap().font;
        let virama = font.glyph_id('্').0;
        assert!(!ids.contains(&virama), "unshaped virama in {ids:?}");
        assert!(ids.iter().all(|id| *id != 0), "tofu in {ids:?}");
    }

    #[test]
    fn shaping_reorders_the_e_matra() {
        // "কে" = ka + U+09C7; the shaper puts the pre-base matra first.
        let ids: Vec<u16> = glyphs("কে").into_iter().map(|g| g.1).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        let ka = sources()[source_for('ক').expect("an Indic font")]
            .get()
            .expect("Indic font")
            .font
            .glyph_id('ক')
            .0;
        assert_eq!(ids[1], ka, "{ids:?}");
        assert_ne!(ids[0], ka, "{ids:?}");
    }

    /// Emoji used to be the ▯ box (egui has no emoji family) and a variation
    /// selector a second one. Both must now come out as ink from the emoji face.
    #[test]
    fn emoji_render_as_ink_without_a_selector_box() {
        for s in ["🎙\u{fe0f}", "🌰", "🖤"] {
            assert!(ink(s) > 0, "{s} drew nothing");
        }
        // The selector alone carries no glyph and must not add one.
        let with = glyphs("🎙\u{fe0f}").len();
        let without = glyphs("🎙").len();
        assert_eq!(with, without, "variation selector became a glyph");
    }

    /// A name that mixes scripts must draw every part: each run picks the font
    /// that covers it, and the run order is preserved.
    #[test]
    fn mixed_scripts_pick_a_font_per_run() {
        let g = glyphs("বাংলা 🌰 Clip");
        assert!(!g.is_empty());
        let used: std::collections::BTreeSet<usize> = g.iter().map(|x| x.0).collect();
        assert!(used.len() >= 2, "expected several fonts, got {used:?}");
        assert!(ink("বাংলা 🌰 Clip") > 0);
    }

    /// A glyph must be drawn at the size its advance assumes.
    ///
    /// ab_glyph sizes by the font's line height while the advances are em-based;
    /// when the two disagreed, `Nirmala.ttc`'s glyphs came out at ~0.75 em and
    /// every Bengali name looked letter-spaced and broken. 'U' is a capital, so
    /// its ink should be a cap-height tall (0.6-0.8 em) at any size.
    #[test]
    fn glyphs_are_drawn_at_the_size_their_advances_assume() {
        for px in [23.0f32, 46.0] {
            let s = shape_run("U", px).expect("shaped");
            let r = rasterise(&s, 1.0, px).expect("raster");
            let ink = r
                .pixels
                .iter()
                .enumerate()
                .filter(|(_, p)| p.a() > 40)
                .map(|(i, _)| i / r.size[0])
                .collect::<Vec<_>>();
            let h = ink.iter().max().unwrap() - ink.iter().min().unwrap() + 1;
            let ratio = h as f32 / px;
            assert!(
                (0.6..=0.8).contains(&ratio),
                "cap height {h}px at {px}px = {ratio:.2} em"
            );
        }
    }

    #[test]
    fn width_is_positive_and_cache_is_keyed_by_size() {        assert!(shape_run("আপনার", 16.0).map(|s| s.width).unwrap_or(0.0) > 0.0);
        assert_ne!(qsize(9.0), qsize(12.0));
    }
}
