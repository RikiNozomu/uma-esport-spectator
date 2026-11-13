//! Paints the track map (render thread). Layout in 1080p pixels at the top-left
//! corner, scaled to the back buffer: egui runs at `height / 1080 * size` pixels
//! per point, and the layer is shifted to the menu's position (`View`).

use std::time::Instant;

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke, Vec2};

use super::{fmt_length, placement, Frame, Ground, Projection, View};
use crate::leaderboard::{bracket_colors, fonts};

// Panel and map box (1080p points).
/// Panel corner and width; its height follows the drawn course.
const PANEL_MIN: Pos2 = Pos2 { x: 16.0, y: 14.0 };
const PANEL_WIDTH: f32 = 388.0;
/// The box the course is fitted into (the labels go right under the course).
const MAP: Rect = Rect { min: Pos2 { x: 36.0, y: 30.0 }, max: Pos2 { x: 384.0, y: 168.0 } };
/// Track band width and the depth of its 3D side.
const BAND: f32 = 11.0;
const DEPTH: f32 = 10.0;

const PANEL_BG: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 120);
const TURF: Color32 = Color32::from_rgb(98, 176, 72);
const TURF_EDGE: Color32 = Color32::from_rgb(34, 82, 34);
const TURF_SIDE: Color32 = Color32::from_rgb(18, 44, 20);
const DIRT: Color32 = Color32::from_rgb(200, 162, 104);
const DIRT_EDGE: Color32 = Color32::from_rgb(110, 80, 44);
const DIRT_SIDE: Color32 = Color32::from_rgb(58, 40, 22);
const RAIL: Color32 = Color32::from_rgba_premultiplied(235, 235, 235, 220);
const GOAL: Color32 = Color32::from_rgb(230, 36, 36);
const LEADER: Color32 = Color32::from_rgb(230, 36, 36);
const TEXT: Color32 = Color32::WHITE;
const DIM: Color32 = Color32::from_rgb(200, 200, 200);

pub struct MapUi {
    pub ctx: egui::Context,
    started: Instant,
    has_bold: bool,
}

impl Default for MapUi {
    fn default() -> Self {
        let ctx = egui::Context::default();
        let has_bold = fonts::install(&ctx);
        MapUi { ctx, started: Instant::now(), has_bold }
    }
}

impl MapUi {
    /// One egui frame for a `w` x `h` pixel back buffer.
    pub fn frame(&mut self, w: u32, h: u32, f: &Frame) -> egui::FullOutput {
        self.frame_with(w, h, f, super::view())
    }

    pub fn frame_with(&mut self, w: u32, h: u32, f: &Frame, view: View) -> egui::FullOutput {
        let ppp = (h as f32 / 1080.0 * view.scale).max(0.1);
        let screen = Vec2::new(w as f32 / ppp, h as f32 / ppp);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
            time: Some(self.started.elapsed().as_secs_f64()),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let ctx = self.ctx.clone();
        let has_bold = self.has_bold;
        ctx.run(raw, |ctx| paint(ctx, f, has_bold, screen, view))
    }
}

fn offset(points: &[Pos2], dy: f32) -> Vec<Pos2> {
    points.iter().map(|p| Pos2::new(p.x, p.y + dy)).collect()
}

/// A short line across the track at `points[i]` (direction from its neighbour).
fn tick_across(points: &[Pos2], i: usize, half: f32) -> [Pos2; 2] {
    let a = points[i];
    let b = if i + 1 < points.len() { points[i + 1] } else { points[i.saturating_sub(1)] };
    let d = b - a;
    let len = d.length().max(1e-3);
    let n = Vec2::new(-d.y / len, d.x / len) * half;
    [a - n, a + n]
}

fn paint(ctx: &egui::Context, f: &Frame, has_bold: bool, screen: Vec2, view: View) {
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("track_map"));
    let painter = ctx.layer_painter(layer);
    let s = &f.state;
    let Some(proj) = Projection::fit(&s.course, MAP.min.x, MAP.min.y, MAP.width(), MAP.height() - DEPTH) else { return };
    let course: Vec<Pos2> = s.course.iter().map(|&(x, z)| proj.project(x, z)).map(|(x, y)| Pos2::new(x, y)).collect();

    // Labels right under the course; the panel ends under the stats row.
    let bottom = course.iter().map(|p| p.y).fold(MAP.min.y, f32::max) + DEPTH + BAND / 2.0;
    let label_y = bottom + 20.0;
    let line_y = label_y + 24.0;
    let row_y = line_y + 26.0;
    let panel = Rect::from_min_max(PANEL_MIN, Pos2::new(PANEL_MIN.x + PANEL_WIDTH, row_y + 24.0));
    // Everything below is laid out at the top-left corner, then shifted.
    let (dx, dy) = placement(
        (panel.min.x, panel.min.y),
        (panel.width(), panel.height()),
        (screen.x, screen.y),
        (PANEL_MIN.x, PANEL_MIN.y),
        view,
    );
    ctx.set_transform_layer(layer, egui::emath::TSTransform::from_translation(Vec2::new(dx, dy)));
    painter.rect_filled(panel, CornerRadius::same(14), PANEL_BG);

    // The track: dark side below, edge, surface, rail.
    let (surface, edge, side) = match s.ground {
        Some(Ground::Dirt) => (DIRT, DIRT_EDGE, DIRT_SIDE),
        _ => (TURF, TURF_EDGE, TURF_SIDE),
    };
    for dy in [DEPTH, DEPTH * 0.66, DEPTH * 0.33] {
        painter.add(egui::Shape::line(offset(&course, dy), Stroke::new(BAND + 4.0, side)));
    }
    painter.add(egui::Shape::line(course.clone(), Stroke::new(BAND + 4.0, edge)));
    painter.add(egui::Shape::line(course.clone(), Stroke::new(BAND, surface)));
    painter.add(egui::Shape::line(course.clone(), Stroke::new(1.2f32, RAIL)));

    // Start (white) and goal (red) across the track.
    painter.line_segment(tick_across(&course, 0, BAND * 0.7), Stroke::new(2.0f32, Color32::WHITE));
    painter.line_segment(tick_across(&course, course.len() - 1, BAND * 0.9), Stroke::new(3.0f32, GOAL));

    // Runners, back to front; the leader last, as the red marker.
    let mut dots: Vec<_> = s.dots.iter().collect();
    dots.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let leader = dots.last().map(|d| d.index);
    for d in &dots {
        let (x, y) = proj.project(d.x, d.z);
        let p = Pos2::new(x, y);
        if Some(d.index) == leader {
            painter.circle(p, 6.5, LEADER, Stroke::new(2.2f32, Color32::WHITE));
            continue;
        }
        let fill = f
            .gates
            .iter()
            .find(|(i, _)| *i == d.index)
            .map(|&(_, gate)| bracket_colors(gate, f.field).0)
            .unwrap_or(Color32::from_rgb(220, 220, 220));
        painter.circle(p, 4.2, fill, Stroke::new(1.2f32, Color32::from_rgb(20, 20, 20)));
    }

    // "TURF 2,000m", right-aligned under the map.
    let bold = if has_bold { FontFamily::Name(fonts::BOLD.into()) } else { FontFamily::Proportional };
    let length = painter.text(Pos2::new(MAP.max.x, label_y), Align2::RIGHT_CENTER, fmt_length(s.length), FontId::new(30.0, bold.clone()), TEXT);
    if let Some(g) = s.ground {
        let word = if g == Ground::Turf { "TURF" } else { "DIRT" };
        painter.text(Pos2::new(length.min.x - 8.0, label_y + 3.0), Align2::RIGHT_CENTER, word, FontId::new(18.0, bold.clone()), TEXT);
    }

    // Stats: leader's metres left, leader's speed.
    painter.line_segment([Pos2::new(MAP.min.x + 10.0, line_y), Pos2::new(MAP.max.x, line_y)], Stroke::new(1.0f32, Color32::from_white_alpha(90)));
    let left = f.remaining.map(|m| fmt_length(m.max(0.0))).unwrap_or_else(|| "-".to_owned());
    painter.text(Pos2::new(MAP.min.x + 14.0, row_y + 2.0), Align2::LEFT_CENTER, "LEFT", FontId::new(15.0, bold.clone()), DIM);
    painter.text(Pos2::new(MAP.min.x + 58.0, row_y), Align2::LEFT_CENTER, left, FontId::new(24.0, bold.clone()), TEXT);
    // A small gauge, then the speed.
    let gauge = Pos2::new(MAP.min.x + 196.0, row_y + 4.0);
    let arc: Vec<Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
            gauge + Vec2::new(a.cos(), a.sin()) * 10.0
        })
        .collect();
    painter.add(egui::Shape::line(arc, Stroke::new(2.0f32, DIM)));
    painter.line_segment([gauge, gauge + Vec2::new(5.0, -7.0)], Stroke::new(2.0f32, DIM));
    let speed = s.speed_kmh.map(|v| format!("{v:.1}")).unwrap_or_else(|| "--.-".to_owned());
    let value = painter.text(Pos2::new(gauge.x + 18.0, row_y), Align2::LEFT_CENTER, speed, FontId::new(24.0, bold.clone()), TEXT);
    painter.text(Pos2::new(value.max.x + 6.0, row_y + 3.0), Align2::LEFT_CENTER, "km/h", FontId::new(16.0, bold), DIM);
}
