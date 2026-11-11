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
use crate::{clock, settings, tracer};
use anim::Animator;
use layout::{CardGeometry, Placement};

pub const KEY_ENABLED: &str = "leaderboard";
pub const KEY_SCALE: &str = "leaderboard_scale";
pub const KEY_Y: &str = "leaderboard_y";
pub const KEY_HIDE_BEFORE_GOAL: &str = "leaderboard_hide_before_goal";

pub const DEFAULT_SCALE: f32 = 1.0;
/// Bottom edge of the row, as a fraction of the screen height.
pub const DEFAULT_Y: f32 = 0.96;
/// Hide when the leader has this many metres left (0 = never).
pub const DEFAULT_HIDE_BEFORE_GOAL: f32 = 200.0;
pub const MAX_HIDE_BEFORE_GOAL: f32 = 1000.0;

/// The leaderboard appears this many seconds into the race.
pub const SHOW_AFTER: f32 = 10.0;
/// Entrance: fade in while rising this far (points) into place.
const ENTRANCE_TIME: f64 = 0.6;
const ENTRANCE_RISE: f32 = 36.0;
/// Exit: after the board should hide, it keeps the last order on screen
/// and fades out while sinking this far (points).
const EXIT_TIME: f64 = 0.45;
const EXIT_DROP: f32 = 24.0;
/// How fast the LEADER tag glides to a new leader.
const TAG_RATE: f32 = 14.0;
/// A gap this long between frames means the board was hidden: the next frame
/// starts a new entrance.
const REAPPEAR_GAP: f64 = 0.3;

/// How often the game is asked for fresh positions.
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);
/// A snapshot older than this means the race is over (or not loaded).
const SNAPSHOT_MAX_AGE: Duration = Duration::from_millis(1500);
/// Part of a 256 px game icon (`chr_icon_*`) that is the framed picture: the
/// icon has a transparent margin of ~10 px left/right and its frame ends at
/// y = 250. Square, so heads that rise above the frame keep most of their room.
const ICON_UV: Rect = Rect { min: Pos2 { x: 10.0 / 256.0, y: 14.0 / 256.0 }, max: Pos2 { x: 246.0 / 256.0, y: 250.0 / 256.0 } };
/// The frame's top edge inside `ICON_UV`, as a fraction of the card width
/// (heads poke out above it); the gate badge sits on the frame's corner.
const ICON_FRAME_TOP: f32 = (33.0 - 14.0) / 236.0;

/// Folder (under the plugin dir) with `<card id>.png` / `<chara id>.png`.
pub const IMAGE_DIR: &str = "chara";

static ENABLED: AtomicBool = AtomicBool::new(false);
static PREVIEW: AtomicBool = AtomicBool::new(false);
static VIEW: Mutex<View> = Mutex::new(View { scale: DEFAULT_SCALE, y: DEFAULT_Y });
/// Metres before the goal at which the board hides, as f32 bits.
static HIDE_BEFORE_GOAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
    let hide = settings::get_f32(KEY_HIDE_BEFORE_GOAL, DEFAULT_HIDE_BEFORE_GOAL).clamp(0.0, MAX_HIDE_BEFORE_GOAL);
    HIDE_BEFORE_GOAL.store(hide.to_bits(), Ordering::Relaxed);
}

pub fn hide_before_goal() -> f32 {
    f32::from_bits(HIDE_BEFORE_GOAL.load(Ordering::Relaxed))
}

pub fn set_hide_before_goal(m: f32) {
    let m = m.clamp(0.0, MAX_HIDE_BEFORE_GOAL).round();
    HIDE_BEFORE_GOAL.store(m.to_bits(), Ordering::Relaxed);
    settings::set_value(KEY_HIDE_BEFORE_GOAL, m);
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
    settings::set_value(KEY_ENABLED, on);
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
    settings::set_value(KEY_SCALE, format!("{:.2}", v.scale));
    settings::set_value(KEY_Y, format!("{:.3}", v.y));
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
    // From SHOW_AFTER seconds into the race until the result board; not in
    // the paddock, gate-in or fanfare. If the clock's hooks are unavailable,
    // fall back to "whenever there is race data".
    if clock::race_live_available()
        && !should_show(clock::race_time(), clock::remaining_distance(), hide_before_goal())
    {
        return None;
    }
    race::request_snapshot(SNAPSHOT_INTERVAL);
    race::latest(SNAPSHOT_MAX_AGE).map(|s| s.runners)
}

/// What to draw this frame.
pub struct Frame {
    pub runners: Vec<Runner>,
    /// 0 = shown, rising to 1 while fading out after the board should hide.
    pub leaving: f32,
}

/// Last runners shown, and when the board started leaving (None = shown).
static LAST: Mutex<Option<(Vec<Runner>, Option<Instant>)>> = Mutex::new(None);

/// Called every frame from the render thread. Like `runners_to_draw`, but
/// keeps the last order on screen for `EXIT_TIME` while it fades out.
pub fn next_frame() -> Option<Frame> {
    let now = runners_to_draw();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(runners) = now {
        *last = Some((runners.clone(), None));
        return Some(Frame { runners, leaving: 0.0 });
    }
    let (runners, since) = last.as_mut()?;
    let since = *since.get_or_insert_with(Instant::now);
    let leaving = (since.elapsed().as_secs_f64() / EXIT_TIME) as f32;
    if leaving >= 1.0 {
        *last = None;
        return None;
    }
    Some(Frame { runners: runners.clone(), leaving })
}

/// `race_time` = seconds into the race while it is live, None otherwise;
/// `remaining` = metres the leader has left; `hide_before_goal` = hide when
/// that is within this many metres (0 = never). Once hidden near the goal it
/// stays hidden: the leader's remaining distance only goes down.
pub fn should_show(race_time: Option<f32>, remaining: Option<f32>, hide_before_goal: f32) -> bool {
    let near_goal = hide_before_goal > 0.0 && remaining.is_some_and(|left| left <= hide_before_goal);
    race_time.is_some_and(|t| t >= SHOW_AFTER) && !near_goal
}

/// Exit progress 0..1 (ease-in) from `leaving` 0..1.
pub fn exit(leaving: f32) -> f32 {
    let p = leaving.clamp(0.0, 1.0);
    p * p
}

/// Entrance progress 0..1 (ease-out) at `elapsed` seconds after appearing.
pub fn entrance(elapsed: f64) -> f32 {
    let p = (elapsed / ENTRANCE_TIME).clamp(0.0, 1.0) as f32;
    1.0 - (1.0 - p).powi(3)
}

/// Text shown under the checkboxes in the plugin menu.
pub fn status() -> String {
    if is_preview() {
        return "showing sample data".to_owned();
    }
    if !is_enabled() {
        return "off".to_owned();
    }
    if clock::race_live_available() && !clock::race_live() {
        return "waiting for a race to start".to_owned();
    }
    if clock::race_live_available() {
        let hide = hide_before_goal();
        if clock::remaining_distance().is_some_and(|left| hide > 0.0 && left <= hide) {
            return format!("hidden for the last {hide:.0} m");
        }
        if !should_show(clock::race_time(), clock::remaining_distance(), hide) {
            return format!("race started, shows at {SHOW_AFTER:.0} s");
        }
    }
    match race::latest(SNAPSHOT_MAX_AGE) {
        Some(s) => format!("live: {} runners", s.runners.len()),
        None => "race started, reading runners...".to_owned(),
    }
}

// ---------------------------------------------------------------- drawing

/// A runner's image: loaded, or missing (checked again after `IMAGE_RETRY`,
/// so icons extracted while the game runs show up without a restart).
enum Image {
    Ready(egui::TextureHandle),
    Missing(Instant),
}

const IMAGE_RETRY: Duration = Duration::from_secs(5);

/// Per-swap-chain drawing state: egui context, card animations, textures.
pub struct Board {
    pub ctx: egui::Context,
    anim: Animator,
    images: HashMap<(i32, i32, i32), Image>,
    image_dir: PathBuf,
    started: Instant,
    has_bold: bool,
    /// Time of the previous frame and of the current entrance (frame clock).
    last_frame: Option<f64>,
    shown_at: f64,
    /// The LEADER tag: (centre x, bottom y, card width), glides between leaders.
    tag: Option<(f32, f32, f32)>,
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
            last_frame: None,
            shown_at: 0.0,
            tag: None,
        }
    }

    /// Runs one egui frame for a `width_px` x `height_px` back buffer.
    pub fn frame(&mut self, width_px: u32, height_px: u32, frame: &Frame, view: View) -> egui::FullOutput {
        let time = self.started.elapsed().as_secs_f64();
        self.frame_at(width_px, height_px, &frame.runners, frame.leaving, view, time)
    }

    /// `frame` with an explicit clock (seconds), for tests and previews.
    pub fn frame_at(
        &mut self,
        width_px: u32,
        height_px: u32,
        runners: &[Runner],
        leaving: f32,
        view: View,
        time: f64,
    ) -> egui::FullOutput {
        let ppp = (height_px as f32 / 1080.0 * view.scale).max(0.1);
        let screen = Vec2::new(width_px as f32 / ppp, height_px as f32 / ppp);

        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
            time: Some(time),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);

        // Hidden for a while (race end, menu toggle): start a new entrance,
        // with cards placed directly in their slots.
        let dt = match self.last_frame {
            Some(t) if time >= t && time - t <= REAPPEAR_GAP => (time - t) as f32,
            _ => {
                self.shown_at = time;
                self.anim = Animator::default();
                self.tag = None;
                0.0
            }
        };
        self.last_frame = Some(time);

        let ctx = self.ctx.clone();
        ctx.run(raw, |ctx| self.paint(ctx, screen, runners, leaving, view, time, dt))
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(&mut self, ctx: &egui::Context, screen: Vec2, runners: &[Runner], leaving: f32, view: View, time: f64, dt: f32) {
        let placements = layout::place(runners, screen, view.y);
        let mut cards = self.anim.update(&placements, time);
        // Whole board: fade in and rise on entrance, fade out and sink on exit.
        let e = entrance(time - self.shown_at);
        let x = exit(leaving);
        let board_alpha = e * (1.0 - x);
        let board_shift = (1.0 - e) * ENTRANCE_RISE + x * EXIT_DROP;
        for c in &mut cards {
            c.opacity *= board_alpha;
            c.bottom += board_shift;
        }
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

        // One LEADER tag that glides to whoever leads (the per-card tags used
        // to cross-fade, showing two at once during a lead change).
        let leader = placements.iter().find(|p| p.order == 0).and_then(|p| cards.iter().find(|c| c.gate == p.gate));
        if let Some(c) = leader {
            let target = (c.center_x, c.bottom, c.width);
            let tag = self.tag.get_or_insert(target);
            tag.0 = anim::approach(tag.0, target.0, TAG_RATE, dt);
            tag.1 = anim::approach(tag.1, target.1, TAG_RATE, dt);
            tag.2 = anim::approach(tag.2, target.2, TAG_RATE, dt);
            draw_leader_tag(&painter, *tag, board_alpha, self.has_bold);
        }
    }

    fn texture(&mut self, ctx: &egui::Context, r: &Runner) -> Option<egui::TextureId> {
        let key = (r.card_id, r.chara_id, r.mob_id);
        let stale = match self.images.get(&key) {
            Some(Image::Ready(t)) => return Some(t.id()),
            Some(Image::Missing(since)) => since.elapsed() >= IMAGE_RETRY,
            None => true,
        };
        if stale {
            let loaded = image_candidates(r).iter().find_map(|name| load_png(&self.image_dir.join(name)));
            let entry = match loaded {
                Some(img) => Image::Ready(ctx.load_texture(
                    format!("chara-{}-{}-{}", key.0, key.1, key.2),
                    img,
                    egui::TextureOptions::LINEAR,
                )),
                None => Image::Missing(Instant::now()),
            };
            self.images.insert(key, entry);
        }
        match self.images.get(&key) {
            Some(Image::Ready(t)) => Some(t.id()),
            _ => None,
        }
    }
}

/// Image files to try for a runner, best first: the outfit of their card,
/// the character's default icon, then (generic NPC runners) the mob icon.
/// Mob ids overlap character ids, hence the `mob_` prefix.
pub fn image_candidates(r: &Runner) -> Vec<String> {
    let mut v = Vec::new();
    if r.card_id > 0 {
        v.push(format!("{}.png", r.card_id));
    }
    if r.chara_id > 0 {
        v.push(format!("{}.png", r.chara_id));
    }
    if r.mob_id > 0 {
        v.push(format!("mob_{}.png", r.mob_id));
    }
    v
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

    // No card background: the image itself is the card.
    // Character image; a plain silhouette until an image is available.
    let img = Rect::from_min_size(rect.min, Vec2::splat(g.image));
    match texture {
        Some(tex) => {
            painter.image(tex, img, ICON_UV, fade(Color32::WHITE));
        }
        None => draw_silhouette(painter, img, fade(bracket_bg.gamma_multiply(0.30)), fade(Color32::from_white_alpha(70))),
    }

    // Gate number in its bracket colour, on the frame's top-left corner.
    let leg = g.triangle;
    let corner = if texture.is_some() { rect.min + Vec2::new(0.0, g.width * ICON_FRAME_TOP) } else { rect.min };
    painter.add(Shape::convex_polygon(
        vec![corner, corner + Vec2::new(leg, 0.0), corner + Vec2::new(0.0, leg)],
        fade(bracket_bg),
        Stroke::new(1.0_f32, fade(Color32::from_black_alpha(90))),
    ));
    // Centred on the triangle's thick end, so two digits stay inside it.
    let num_pos = corner + Vec2::new(leg * 0.28, leg * 0.33);
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

    // Finished runners get a check mark in the top-right corner.
    if runner.finished {
        let c = Pos2::new(rect.max.x - g.triangle * 0.3, rect.min.y + g.triangle * 0.3);
        painter.circle_filled(c, g.triangle * 0.2, fade(Color32::from_rgb(40, 170, 90)));
        painter.text(c, Align2::CENTER_CENTER, "✓", FontId::proportional(g.triangle * 0.28), fade(Color32::WHITE));
    }
}

/// The LEADER tag above a card at (centre x, bottom y, width).
fn draw_leader_tag(painter: &egui::Painter, (x, bottom, width): (f32, f32, f32), alpha: f32, has_bold: bool) {
    if alpha <= 0.01 {
        return;
    }
    let g = CardGeometry::for_width(width);
    let family = if has_bold { FontFamily::Name(fonts::BOLD.into()) } else { FontFamily::Proportional };
    // Text colour is baked into the galley, so the fade goes in here too.
    let text = Color32::from_gray(235).gamma_multiply(alpha);
    let galley = painter.layout_no_wrap("LEADER".to_owned(), FontId::new(width * 0.15, family), text);
    let size = galley.size() + Vec2::new(width * 0.18, width * 0.06);
    let card_top = bottom - g.height;
    let tag = Rect::from_center_size(Pos2::new(x, card_top - width * 0.06 - size.y / 2.0), size);
    painter.rect(
        tag,
        CornerRadius::same((size.y / 2.0) as u8),
        Color32::from_rgba_unmultiplied(35, 35, 38, 225).gamma_multiply(alpha),
        Stroke::new(1.0_f32, Color32::from_white_alpha(40).gamma_multiply(alpha)),
        egui::StrokeKind::Inside,
    );
    painter.galley(tag.center() - galley.size() / 2.0, galley, text);
}

/// Generic head-and-shoulders shape for runners without an image.
fn draw_silhouette(painter: &egui::Painter, area: Rect, background: Color32, figure: Color32) {
    painter.rect_filled(area, CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, background);
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
                mob_id: 0,
                finished: false,
                finish_order: -1,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_lookup_order() {
        let named = Runner { card_id: 100101, chara_id: 1001, ..Default::default() };
        assert_eq!(image_candidates(&named), vec!["100101.png", "1001.png"]);
        let mob = Runner { mob_id: 8001, ..Default::default() };
        assert_eq!(image_candidates(&mob), vec!["mob_8001.png"]);
    }

    #[test]
    fn missing_image_is_picked_up_later() {
        let dir = std::env::temp_dir().join(format!("uma-lb-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut board = Board::new(dir.clone());
        let ctx = board.ctx.clone();
        let r = Runner { chara_id: 1001, ..Default::default() };
        assert!(board.texture(&ctx, &r).is_none());

        // Extracted while the game runs: found once the retry interval has passed.
        image::RgbaImage::new(4, 4).save(dir.join("1001.png")).unwrap();
        assert!(board.texture(&ctx, &r).is_none(), "not re-checked every frame");
        board.images.insert((0, 1001, 0), Image::Missing(Instant::now() - IMAGE_RETRY));
        assert!(board.texture(&ctx, &r).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shows_from_ten_seconds_into_the_race() {
        let far = Some(1500.0);
        assert!(!should_show(None, None, 200.0), "no live race");
        assert!(!should_show(Some(0.5), far, 200.0));
        assert!(!should_show(Some(9.9), far, 200.0));
        assert!(should_show(Some(10.0), far, 200.0));
        assert!(should_show(Some(95.0), far, 200.0));
    }

    #[test]
    fn hides_before_the_goal() {
        assert!(should_show(Some(80.0), Some(200.1), 200.0));
        assert!(!should_show(Some(80.0), Some(200.0), 200.0), "at 200 m left");
        assert!(!should_show(Some(90.0), Some(-15.0), 200.0), "stays hidden after the finish");
        assert!(should_show(Some(90.0), Some(50.0), 0.0), "0 = never hide");
        assert!(should_show(Some(80.0), None, 200.0), "distance unknown: keep showing");
    }

    #[test]
    fn entrance_eases_in() {
        assert_eq!(entrance(0.0), 0.0);
        assert!(entrance(0.1) > 0.3 && entrance(0.1) < 0.7, "fast start: {}", entrance(0.1));
        assert!(entrance(0.3) < 1.0);
        assert_eq!(entrance(ENTRANCE_TIME), 1.0);
        assert_eq!(entrance(5.0), 1.0);
    }

    #[test]
    fn reappearing_restarts_the_entrance() {
        let mut board = Board::new(PathBuf::from("/nonexistent"));
        let runners = preview::runners();
        let v = View { scale: 1.0, y: DEFAULT_Y };
        board.frame_at(1920, 1080, &runners, 0.0, v, 1.0);
        assert_eq!(board.shown_at, 1.0);
        board.frame_at(1920, 1080, &runners, 0.0, v, 1.016);
        assert_eq!(board.shown_at, 1.0, "continuous frames: same entrance");
        board.frame_at(1920, 1080, &runners, 0.0, v, 5.0);
        assert_eq!(board.shown_at, 5.0, "after a gap: new entrance");
    }

    #[test]
    fn exit_eases_out() {
        assert_eq!(exit(0.0), 0.0);
        assert!(exit(0.3) < 0.3, "slow start");
        assert_eq!(exit(1.0), 1.0);
    }

    #[test]
    fn leader_tag_glides_to_the_new_leader() {
        let mut board = Board::new(PathBuf::from("/nonexistent"));
        let v = View { scale: 1.0, y: DEFAULT_Y };
        let field = |leader: i32| -> Vec<Runner> {
            (1..=3)
                .map(|g| Runner { gate: g, order: if g == leader { 0 } else if g < leader { g } else { g - 1 }, ..Default::default() })
                .collect()
        };
        let mut t = 0.0;
        for _ in 0..60 {
            board.frame_at(1920, 1080, &field(1), 0.0, v, t);
            t += 1.0 / 60.0;
        }
        let before = board.tag.unwrap().0;
        // Gate 2 takes the lead: one frame later the tag is on its way, not jumped.
        board.frame_at(1920, 1080, &field(2), 0.0, v, t);
        let after = board.tag.unwrap().0;
        // New leader's card is one slot (~100 pt) to the left of the old one.
        let moved = before - after;
        assert!(moved > 0.0 && moved < 40.0, "glides, does not jump: moved {moved}");
        for _ in 0..90 {
            t += 1.0 / 60.0;
            board.frame_at(1920, 1080, &field(2), 0.0, v, t);
        }
        let settled = board.tag.unwrap().0;
        // The leader slot is the rightmost; the tag ends over it again.
        assert!((settled - before).abs() < 1.0, "back over the leader slot: {settled} vs {before}");
    }

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
        let out = board.frame_at(1920, 1080, &runners, 0.0, View { scale: 1.0, y: DEFAULT_Y }, 0.0);
        assert!(out.shapes.len() >= runners.len() * 4, "got {} shapes", out.shapes.len());
        // Fonts are uploaded on the first frame.
        assert!(!out.textures_delta.set.is_empty());
    }
}
