//! Paints the seek bar with egui (render thread, see `leaderboard::render_d3d11`).
//! Layout comes from `super::geometry`, in back-buffer pixels; egui works in
//! points, so everything is divided by the pixels-per-point here.

use std::time::Instant;

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Shape, Stroke, Vec2};

use super::{fmt_speed, fmt_time, geometry, Frame, Geom, Part, SPEEDS};
use crate::leaderboard::fonts;

const PANEL: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 160);
const TRACK: Color32 = Color32::from_rgb(110, 110, 110);
/// Same yellow as the race clock's split.
const FILL: Color32 = Color32::from_rgb(0xFF, 0xE6, 0x00);
const WHITE: Color32 = Color32::WHITE;

pub struct SeekUi {
    pub ctx: egui::Context,
    started: Instant,
    has_bold: bool,
}

impl Default for SeekUi {
    fn default() -> Self {
        let ctx = egui::Context::default();
        let has_bold = fonts::install(&ctx);
        SeekUi { ctx, started: Instant::now(), has_bold }
    }
}

impl SeekUi {
    /// One egui frame for a `w` x `h` pixel back buffer. Also returns the
    /// layout, for hit testing.
    pub fn frame(&mut self, w: u32, h: u32, f: &Frame) -> (egui::FullOutput, Geom) {
        let g = geometry(w as f32, h as f32);
        let ppp = g.s;
        let screen = Vec2::new(w as f32 / ppp, h as f32 / ppp);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
            time: Some(self.started.elapsed().as_secs_f64()),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let ctx = self.ctx.clone();
        let has_bold = self.has_bold;
        let out = ctx.run(raw, |ctx| paint(ctx, &g, f, has_bold));
        (out, g)
    }
}

fn pts(r: Rect, ppp: f32) -> Rect {
    Rect::from_min_max(Pos2::new(r.min.x / ppp, r.min.y / ppp), Pos2::new(r.max.x / ppp, r.max.y / ppp))
}

fn paint(ctx: &egui::Context, g: &Geom, f: &Frame, has_bold: bool) {
    let ppp = g.s;
    let a = f.alpha.clamp(0.0, 1.0);
    let dim = if f.replay.interactable { 1.0 } else { 0.45 };
    let col = |c: Color32, k: f32| c.gamma_multiply(a * k);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("seek_bar")));

    // Panel.
    let panel = pts(g.panel, ppp);
    painter.rect_filled(panel, CornerRadius::same((panel.height() / 2.0).min(255.0) as u8), col(PANEL, 1.0));

    // Play / pause button: shows what a click does.
    let b = pts(g.button, ppp);
    let k = dim * if f.hover == Part::PlayPause { 1.0 } else { 0.85 };
    if f.replay.paused {
        let (l, r, t, bt) = (b.min.x + b.width() * 0.2, b.max.x - b.width() * 0.1, b.min.y + b.height() * 0.1, b.max.y - b.height() * 0.1);
        let pts = vec![Pos2::new(l, t), Pos2::new(r, b.center().y), Pos2::new(l, bt)];
        painter.add(Shape::convex_polygon(pts, col(WHITE, k), Stroke::NONE));
    } else {
        let bar_w = b.width() * 0.28;
        let (t, bt) = (b.min.y + b.height() * 0.12, b.max.y - b.height() * 0.12);
        for x in [b.min.x + b.width() * 0.12, b.max.x - b.width() * 0.12 - bar_w] {
            let r = Rect::from_min_max(Pos2::new(x, t), Pos2::new(x + bar_w, bt));
            painter.rect_filled(r, CornerRadius::same(2), col(WHITE, k));
        }
    }

    // Speed: « slower | 1.0x (click = 1.0x) | faster ». Always usable.
    let family = if has_bold { FontFamily::Name(fonts::BOLD.into()) } else { FontFamily::Proportional };
    let speed = f.replay.speed;
    let hover_bg = |r: Rect| painter.rect_filled(r, CornerRadius::same(6), col(WHITE, 0.15));
    for (part, rect, dir, at_end) in [
        (Part::Slower, g.slower, -1.0, speed <= SPEEDS[0]),
        (Part::Faster, g.faster, 1.0, speed >= SPEEDS[SPEEDS.len() - 1]),
    ] {
        let r = pts(rect, ppp);
        if f.hover == part && !at_end {
            hover_bg(r);
        }
        let k = if at_end { 0.3 } else if f.hover == part { 1.0 } else { 0.85 };
        double_arrow(&painter, r, dir, col(WHITE, k));
    }
    let label = pts(g.speed, ppp);
    if f.hover == Part::Speed {
        hover_bg(label);
    }
    let speed_color = if speed == 1.0 { WHITE } else { FILL };
    painter.text(label.center(), Align2::CENTER_CENTER, fmt_speed(speed), FontId::new(17.0, family.clone()), col(speed_color, 1.0));

    // Time.
    let text = format!("{} / {}", fmt_time(f.shown_time), fmt_time(f.replay.total));
    painter.text(
        Pos2::new(g.text_x / ppp, g.track_y / ppp),
        Align2::LEFT_CENTER,
        text,
        FontId::new(19.0, family),
        col(WHITE, dim),
    );

    // Track, filled part, knob.
    let (x0, x1, y) = (g.track_x0 / ppp, g.track_x1 / ppp, g.track_y / ppp);
    let active = f.dragging || f.hover == Part::Track;
    let half = if active { 4.0 } else { 3.0 };
    let track = Rect::from_min_max(Pos2::new(x0, y - half), Pos2::new(x1, y + half));
    painter.rect_filled(track, CornerRadius::same(half as u8), col(TRACK, dim));
    let xk = g.x_at(f.shown_time, f.replay.total) / ppp;
    let filled = Rect::from_min_max(track.min, Pos2::new(xk.max(x0), track.max.y));
    painter.rect_filled(filled, CornerRadius::same(half as u8), col(FILL, dim));
    let radius = if f.dragging { 11.0 } else if active { 9.5 } else { 8.0 };
    painter.circle(Pos2::new(xk, y), radius, col(WHITE, dim), Stroke::new(1.5f32, col(Color32::BLACK, 0.5 * dim)));
}

/// Two small triangles pointing left (`dir` < 0, slower) or right (faster),
/// like rewind / fast-forward.
fn double_arrow(painter: &egui::Painter, r: Rect, dir: f32, color: Color32) {
    let c = r.center();
    let (w, h) = (r.width() * 0.26, r.height() * 0.3);
    for i in [-1.0f32, 1.0] {
        let cx = c.x + i * w * 0.5;
        let (tip, back) = (cx + dir * w / 2.0, cx - dir * w / 2.0);
        let tri = vec![Pos2::new(back, c.y - h), Pos2::new(tip, c.y), Pos2::new(back, c.y + h)];
        painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
    }
}
