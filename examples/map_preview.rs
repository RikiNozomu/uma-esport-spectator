//! Dumps a tessellated track-map frame for an offline look:
//! `cargo run --example map_preview -- <out dir>`, then
//! `python3 tools/rasterize.py <out dir>`. Same file format as `render_preview`.
//! A made-up 2000 m stadium-shaped course; not part of the plugin.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use uma_esport_spectator::track_map::draw::MapUi;
use uma_esport_spectator::track_map::{Dot, Frame, Ground, MapState};

/// A point `d` metres along a stadium loop (two straights, two half circles),
/// tilted by `angle` in the world.
fn stadium(d: f32, straight: f32, radius: f32, angle: f32) -> (f32, f32) {
    let half = std::f32::consts::PI * radius;
    let lap = 2.0 * straight + 2.0 * half;
    let d = d.rem_euclid(lap);
    let (u, v) = if d < straight {
        (-straight / 2.0 + d, -radius)
    } else if d < straight + half {
        let a = -std::f32::consts::FRAC_PI_2 + (d - straight) / radius;
        (straight / 2.0 + radius * a.cos(), radius * a.sin())
    } else if d < 2.0 * straight + half {
        (straight / 2.0 - (d - straight - half), radius)
    } else {
        let a = std::f32::consts::FRAC_PI_2 + (d - 2.0 * straight - half) / radius;
        (-straight / 2.0 + radius * a.cos(), radius * a.sin())
    };
    (u * angle.cos() - v * angle.sin() + 300.0, u * angle.sin() + v * angle.cos() - 120.0)
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    let (length, straight, radius, angle) = (2000.0f32, 560.0, 140.0, 0.9);
    // Start 1950 m before the goal, which is on the straight at lap distance 280.
    let start = 280.0 - length;
    let course: Vec<(f32, f32)> = (0..=500).map(|i| stadium(start + length * i as f32 / 500.0, straight, radius, angle)).collect();
    let dots: Vec<Dot> = (0..16)
        .map(|i| {
            let d = 1388.0 - i as f32 * 9.5;
            let (x, z) = stadium(start + d, straight, radius - (i % 3) as f32 * 2.5, angle);
            Dot { index: i, x, z, distance: d }
        })
        .collect();
    let state = MapState { course, length, ground: Some(Ground::Turf), dots, speed_kmh: Some(62.4) };
    let frame = Frame { state, gates: (0..16).map(|i| (i, i + 1)).collect(), field: 16, remaining: Some(612.0) };
    let mut ui = MapUi::default();
    let output = ui.frame(1920, 1080, &frame);
    dump(&ui.ctx, output, &out.join("frame000.bin"));
    println!("frame000: track map");
}

fn dump(ctx: &egui::Context, output: egui::FullOutput, path: &std::path::Path) {
    let ppp = output.pixels_per_point;
    let prims = ctx.tessellate(output.shapes, ppp);
    let mut f = BufWriter::new(File::create(path).unwrap());
    let tex: Vec<_> = output.textures_delta.set.iter().collect();
    f.write_all(&(tex.len() as u32).to_le_bytes()).unwrap();
    for (id, delta) in tex {
        let egui::ImageData::Color(img) = &delta.image;
        let key = match id { egui::TextureId::Managed(n) => *n, egui::TextureId::User(n) => *n + (1 << 40) };
        f.write_all(&key.to_le_bytes()).unwrap();
        let pos = delta.pos.map(|[x, y]| [x as i32, y as i32]).unwrap_or([-1, -1]);
        for v in pos { f.write_all(&v.to_le_bytes()).unwrap(); }
        f.write_all(&(img.size[0] as u32).to_le_bytes()).unwrap();
        f.write_all(&(img.size[1] as u32).to_le_bytes()).unwrap();
        for p in &img.pixels { f.write_all(&p.to_array()).unwrap(); }
    }
    let meshes: Vec<_> = prims.iter().filter_map(|p| match &p.primitive {
        egui::epaint::Primitive::Mesh(m) => Some((p.clip_rect, m)),
        _ => None,
    }).collect();
    f.write_all(&(meshes.len() as u32).to_le_bytes()).unwrap();
    f.write_all(&ppp.to_le_bytes()).unwrap();
    for (clip, m) in meshes {
        let key = match m.texture_id { egui::TextureId::Managed(n) => n, egui::TextureId::User(n) => n + (1 << 40) };
        f.write_all(&key.to_le_bytes()).unwrap();
        for v in [clip.min.x, clip.min.y, clip.max.x, clip.max.y] { f.write_all(&v.to_le_bytes()).unwrap(); }
        f.write_all(&(m.vertices.len() as u32).to_le_bytes()).unwrap();
        for v in &m.vertices {
            for x in [v.pos.x, v.pos.y, v.uv.x, v.uv.y] { f.write_all(&x.to_le_bytes()).unwrap(); }
            f.write_all(&v.color.to_array()).unwrap();
        }
        f.write_all(&(m.indices.len() as u32).to_le_bytes()).unwrap();
        for ix in &m.indices { f.write_all(&ix.to_le_bytes()).unwrap(); }
    }
}
