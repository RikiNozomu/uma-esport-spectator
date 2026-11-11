//! Live leaderboard overlay: one card per runner in a right-aligned row,
//! leader on the right. Each card shows the character image, the gate number
//! in its bracket colour, and the owner's trainer name at the bottom.
//!
//! Cards belong to runners (keyed by gate), not to positions: when two
//! runners swap, their cards slide past each other. Size changes (entering or
//! leaving the top 3) are animated too, and a runner that gains a place gets a
//! short scale "pop".
//!
//! Platform-independent: this module produces egui shapes. `render_d3d11`
//! puts them on screen from Hachimi's present callback (Windows).

mod anim;
mod fonts;
pub mod layout;
#[cfg(windows)]
pub mod render_d3d11;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Shape, Stroke, Vec2};

use crate::race::{self, Runner};
use crate::{settings, tracer};
use anim::Animator;
use layout::{CardGeometry, Placement};

pub const KEY_ENABLED: &str = "leaderboard";
pub const KEY_SCALE: &str = "leaderboard_scale";
pub const KEY_Y: &str = "leaderboard_y";

pub const DEFAULT_SCALE: f32 = 1.0;
/// Bottom edge of the row, as a fraction of the screen height.
pub const DEFAULT_Y: f32 = 0.96;

/// How often the game is asked for fresh positions.
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);
/// A snapshot older than this means the race is over (or not loaded).
const SNAPSHOT_MAX_AGE: Duration = Duration::from_millis(1500);
/// Folder (under the plugin dir) with `<card id>.png` / `<chara id>.png`.
pub const IMAGE_DIR: &str = "chara";

static ENABLED: AtomicBool = AtomicBool::new(false);
static PREVIEW: AtomicBool = AtomicBool::new(false);
static VIEW: Mutex<View> = Mutex::new(View { scale: DEFAULT_SCALE, y: DEFAULT_Y });

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f32,
    pub y: f32,
}

pub fn load_settings() {
    ENABLED.store(settings::get_bool(KEY_ENABLED, false), Ordering::Relaxed);
    *VIEW.lock().unwrap_or_else(|e| e.into_inner()) = View {
        scale: settings::get_f32(KEY_SCALE, DEFAULT_SCALE).clamp(0.4, 2.5),
        y: settings::get_f32(KEY_Y, DEFAULT_Y).clamp(0.1, 1.0),
    };
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    #[cfg(windows)]
    if on {
        render_d3d11::retry();
    }
    settings::set(KEY_ENABLED, on);
}

pub fn is_preview() -> bool {
    PREVIEW.load(Ordering::Relaxed)
}

/// Sample data that reshuffles every second, for checking the layout
/// outside a race. Not saved.
pub fn set_preview(on: bool) {
    PREVIEW.store(on, Ordering::Relaxed);
    #[cfg(windows)]
    if on {
        render_d3d11::retry();
    }
}

pub fn view() -> View {
    *VIEW.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set_view(v: View) {
    *VIEW.lock().unwrap_or_else(|e| e.into_inner()) = v;
    settings::set(KEY_SCALE, format!("{:.2}", v.scale));
    settings::set(KEY_Y, format!("{:.3}", v.y));
}

pub fn image_dir() -> PathBuf {
    tracer::plugin_dir().join(IMAGE_DIR)
}

/// Called every frame from the render thread. Returns the runners to draw,
/// or None when there is nothing to show (skip all rendering work).
pub fn runners_to_draw() -> Option<Vec<Runner>> {
    if !is_enabled() && !is_preview() {
        return None;
    }
    if is_preview() {
        return Some(preview::runners());
    }
    race::request_snapshot(SNAPSHOT_INTERVAL);
    race::latest(SNAPSHOT_MAX_AGE).map(|s| s.runners)
}

/// Text shown under the checkboxes in the plugin menu.
pub fn status() -> String {
    if is_preview() {
        return "showing sample data".to_owned();
    }
    if !is_enabled() {
        return "off".to_owned();
    }
    match race::latest(SNAPSHOT_MAX_AGE) {
        Some(s) => format!("live: {} runners", s.runners.len()),
        None => "waiting for a race".to_owned(),
    }
}

// ---------------------------------------------------------------- drawing

/// Per-swap-chain drawing state: egui context, card animations, textures.
pub struct Board {
    pub ctx: egui::Context,
    anim: Animator,
    images: HashMap<(i32, i32), Option<egui::TextureHandle>>,
    image_dir: PathBuf,
    started: Instant,
    has_bold: bool,
}

impl Board {
    pub fn new(image_dir: PathBuf) -> Board {
        let ctx = egui::Context::default();
        let has_bold = fonts::install(&ctx);
        Board {
            ctx,
            anim: Animator::default(),
            images: HashMap::new(),
            image_dir,
            started: Instant::now(),
            has_bold,
        }
    }

    /// Runs one egui frame for a `width_px` x `height_px` back buffer.
    pub fn frame(&mut self, width_px: u32, height_px: u32, runners: &[Runner], view: View) -> egui::FullOutput {
        let time = self.started.elapsed().as_secs_f64();
        self.frame_at(width_px, height_px, runners, view, time)
    }

    /// `frame` with an explicit clock (seconds), for tests and previews.
    pub fn frame_at(&mut self, width_px: u32, height_px: u32, runners: &[Runner], view: View, time: f64) -> egui::FullOutput {
        let ppp = (height_px as f32 / 1080.0 * view.scale).max(0.1);
        let screen = Vec2::new(width_px as f32 / ppp, height_px as f32 / ppp);

        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
            time: Some(time),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);

        let ctx = self.ctx.clone();
        ctx.run(raw, |ctx| self.paint(ctx, screen, runners, view, time))
    }

    fn paint(&mut self, ctx: &egui::Context, screen: Vec2, runners: &[Runner], view: View, time: f64) {
        let placements = layout::place(runners, screen, view.y);
        let cards = self.anim.update(&placements, time);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("leaderboard")));
        let field = runners.len();

        // Cards that are rising are drawn last so they pass over the others.
        let mut order: Vec<usize> = (0..cards.len()).collect();
        order.sort_by(|&a, &b| cards[a].z.total_cmp(&cards[b].z));
        for i in order {
            let card = &cards[i];
            let Some(runner) = runners.iter().find(|r| r.gate == card.gate) else { continue };
            let texture = self.texture(ctx, runner);
            draw_card(&painter, card, runner, field, texture, self.has_bold);
        }
    }

    fn texture(&mut self, ctx: &egui::Context, r: &Runner) -> Option<egui::TextureId> {
        let key = (r.card_id, r.chara_id);
        let dir = &self.image_dir;
        self.images
            .entry(key)
            .or_insert_with(|| {
                let candidates = [r.card_id, r.chara_id];
                candidates
                    .iter()
                    .filter(|&&id| id > 0)
                    .find_map(|id| load_png(&dir.join(format!("{id}.png"))))
                    .map(|img| ctx.load_texture(format!("chara-{}-{}", key.0, key.1), img, egui::TextureOptions::LINEAR))
            })
            .as_ref()
            .map(|t| t.id())
    }
}

fn load_png(path: &std::path::Path) -> Option<egui::ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// The JRA bracket (waku) colours: background and number colour.
pub fn bracket_colors(gate: i32, field: usize) -> (Color32, Color32) {
    let dark = Color32::from_rgb(20, 20, 20);
    let light = Color32::WHITE;
    match layout::bracket(gate, field) {
        1 => (Color32::from_rgb(240, 240, 240), dark),
        2 => (Color32::from_rgb(25, 25, 25), light),
        3 => (Color32::from_rgb(222, 38, 38), light),
        4 => (Color32::from_rgb(35, 90, 205), light),
        5 => (Color32::from_rgb(245, 215, 20), dark),
        6 => (Color32::from_rgb(35, 145, 65), light),
        7 => (Color32::from_rgb(245, 135, 20), dark),
        _ => (Color32::from_rgb(245, 172, 192), dark),
    }
}

fn draw_card(
    painter: &egui::Painter,
    card: &anim::CardState,
    runner: &Runner,
    field: usize,
    texture: Option<egui::TextureId>,
    has_bold: bool,
) {
    let g = CardGeometry::for_width(card.width);
    let rect = Rect::from_min_size(Pos2::new(card.center_x - g.width / 2.0, card.bottom - g.height), Vec2::new(g.width, g.height));
    let alpha = card.opacity;
    let fade = |c: Color32| c.gamma_multiply(alpha);
    let radius = CornerRadius::same((g.width * 0.07).round().clamp(0.0, 255.0) as u8);
    let (bracket_bg, bracket_fg) = bracket_colors(runner.gate, field);
    let bold = if has_bold { FontFamily::Name(fonts::BOLD.into()) } else { FontFamily::Proportional };

    // Body.
    painter.rect(rect, radius, fade(Color32::from_rgba_unmultiplied(44, 44, 46, 215)), Stroke::new(1.0_f32, fade(Color32::from_white_alpha(38))), egui::StrokeKind::Inside);

    // Character image; a plain silhouette until an image is available.
    let img = Rect::from_min_size(rect.min + Vec2::splat(g.pad), Vec2::splat(g.image));
    match texture {
        Some(tex) => {
            painter.image(tex, img, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), fade(Color32::WHITE));
        }
        None => draw_silhouette(painter, img, fade(bracket_bg.gamma_multiply(0.30)), fade(Color32::from_white_alpha(70))),
    }

    // Gate number in its bracket colour, top-left.
    let leg = g.triangle;
    painter.add(Shape::convex_polygon(
        vec![rect.min, rect.min + Vec2::new(leg, 0.0), rect.min + Vec2::new(0.0, leg)],
        fade(bracket_bg),
        Stroke::new(1.0_f32, fade(Color32::from_black_alpha(90))),
    ));
    // Centred on the triangle's thick end, so two digits stay inside it.
    let num_pos = rect.min + Vec2::new(leg * 0.3, leg * 0.26);
    let num_font = FontId::new(g.number_font, bold.clone());
    painter.text(num_pos, Align2::CENTER_CENTER, runner.gate.to_string(), num_font, fade(bracket_fg));

    // Trainer (owner) name, bottom.
    let band = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - g.band), rect.max);
    painter.rect_filled(
        band,
        CornerRadius { nw: 0, ne: 0, sw: radius.sw, se: radius.se },
        fade(Color32::from_black_alpha(175)),
    );
    if !runner.trainer.is_empty() {
        let font = FontId::proportional(g.trainer_font);
        let text = fit_text(painter, &runner.trainer, font.clone(), band.width() - g.pad * 2.0);
        painter.text(band.center(), Align2::CENTER_CENTER, text, font, fade(Color32::from_gray(235)));
    }

    // "LEADER" tag above the first card.
    if card.leader_tag > 0.01 {
        let font = FontId::new(g.width * 0.15, bold);
        let galley = painter.layout_no_wrap("LEADER".to_owned(), font, fade(Color32::from_gray(235)));
        let size = galley.size() + Vec2::new(g.width * 0.18, g.width * 0.06);
        let tag = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y - g.width * 0.06 - size.y / 2.0), size);
        let t = card.leader_tag * alpha;
        painter.rect(tag, CornerRadius::same((size.y / 2.0) as u8), Color32::from_rgba_unmultiplied(35, 35, 38, 225).gamma_multiply(t), Stroke::new(1.0_f32, Color32::from_white_alpha(40).gamma_multiply(t)), egui::StrokeKind::Inside);
        painter.galley(tag.center() - galley.size() / 2.0, galley, Color32::from_gray(235).gamma_multiply(t));
    }

    // Finished runners get a check mark in the top-right corner.
    if runner.finished {
        let c = Pos2::new(rect.max.x - g.triangle * 0.3, rect.min.y + g.triangle * 0.3);
        painter.circle_filled(c, g.triangle * 0.2, fade(Color32::from_rgb(40, 170, 90)));
        painter.text(c, Align2::CENTER_CENTER, "✓", FontId::proportional(g.triangle * 0.28), fade(Color32::WHITE));
    }
}

/// Generic head-and-shoulders shape for runners without an image.
fn draw_silhouette(painter: &egui::Painter, area: Rect, background: Color32, figure: Color32) {
    painter.rect_filled(area, CornerRadius::same(4), background);
    let painter = painter.with_clip_rect(area);
    let s = area.width();
    let head = Pos2::new(area.center().x, area.min.y + s * 0.40);
    painter.circle_filled(head, s * 0.19, figure);
    let shoulders = Rect::from_center_size(Pos2::new(area.center().x, area.max.y + s * 0.12), Vec2::new(s * 0.72, s * 0.62));
    painter.rect_filled(shoulders, CornerRadius::same((s * 0.3).clamp(0.0, 255.0) as u8), figure);
}

/// `text`, cut with "…" so it fits in `max_width`.
fn fit_text(painter: &egui::Painter, text: &str, font: FontId, max_width: f32) -> String {
    let width = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let s: String = chars[..n].iter().collect::<String>() + "…";
        if width(&s) <= max_width {
            return s;
        }
    }
    String::new()
}

impl From<Placement> for anim::Target {
    fn from(p: Placement) -> Self {
        anim::Target { gate: p.gate, order: p.order, center_x: p.center_x, bottom: p.bottom, width: p.width }
    }
}

// ---------------------------------------------------------------- preview

mod preview {
    use super::*;

    const NAMES: [&str; 18] = [
        "Special Week", "Silence Suzuka", "Tokai Teio", "Maruzensky", "Oguri Cap", "Gold Ship",
        "Vodka", "Daiwa Scarlet", "Taiki Shuttle", "Grass Wonder", "El Condor Pasa", "Symboli Rudolf",
        "Rice Shower", "Mejiro McQueen", "Agnes Tachyon", "Kitasan Black", "Satono Diamond", "Haru Urara",
    ];
    const IDS: [i32; 18] = [
        1001, 1002, 1003, 1004, 1006, 1007, 1008, 1009, 1010, 1011, 1012, 1017, 1030, 1013, 1032, 1068, 1067, 1053,
    ];

    /// 18 runners; every second two neighbours swap places.
    pub fn runners() -> Vec<Runner> {
        static START: Mutex<Option<Instant>> = Mutex::new(None);
        let step = {
            let mut s = START.lock().unwrap_or_else(|e| e.into_inner());
            s.get_or_insert_with(Instant::now).elapsed().as_secs()
        };
        let mut order: Vec<usize> = (0..NAMES.len()).collect();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..step {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let i = ((seed >> 33) as usize) % (order.len() - 1);
            order.swap(i, i + 1);
        }
        order
            .iter()
            .enumerate()
            .map(|(pos, &i)| Runner {
                gate: i as i32 + 1,
                order: pos as i32,
                chara_id: IDS[i],
                card_id: IDS[i] * 100 + 1,
                name: NAMES[i].to_owned(),
                trainer: format!("Trainer{:02}", i + 1),
                finished: false,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_a_valid_field() {
        let rs = preview::runners();
        assert_eq!(rs.len(), 18);
        let mut orders: Vec<i32> = rs.iter().map(|r| r.order).collect();
        orders.sort();
        assert_eq!(orders, (0..18).collect::<Vec<_>>());
    }

    #[test]
    fn frame_draws_every_card() {
        let mut board = Board::new(PathBuf::from("/nonexistent"));
        let runners = preview::runners();
        let out = board.frame(1920, 1080, &runners, View { scale: 1.0, y: DEFAULT_Y });
        assert!(out.shapes.len() >= runners.len() * 4, "got {} shapes", out.shapes.len());
        // Fonts are uploaded on the first frame.
        assert!(!out.textures_delta.set.is_empty());
    }
}
