//! Dumps tessellated seek-bar frames (one per state) for an offline look:
//! `cargo run --example seek_preview -- <out dir>`, then
//! `python3 tools/rasterize.py <out dir>`. Same file format as `render_preview`.
//! Not part of the plugin.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use uma_esport_spectator::seek::draw::SeekUi;
use uma_esport_spectator::seek::{Frame, Part, Replay};

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    let (w, h) = (1920u32, 1080u32);
    let replay = Replay { cur: 31.4, total: 95.2, paused: false, interactable: true, speed: 1.0 };
    let base = Frame { replay, shown_time: 31.4, dragging: false, hover: Part::None, alpha: 1.0 };
    let states = [
        ("playing", base.clone()),
        ("hover track", Frame { hover: Part::Track, ..base.clone() }),
        ("dragging", Frame { dragging: true, hover: Part::Track, shown_time: 62.0, replay: Replay { paused: true, ..replay }, ..base.clone() }),
        ("paused", Frame { replay: Replay { paused: true, ..replay }, hover: Part::PlayPause, ..base.clone() }),
        ("2.0x, hover faster", Frame { replay: Replay { speed: 2.0, ..replay }, hover: Part::Faster, ..base.clone() }),
        ("4.0x (top), hover label", Frame { replay: Replay { speed: 4.0, ..replay }, hover: Part::Speed, ..base.clone() }),
        ("0.25x (bottom), hover slower", Frame { replay: Replay { speed: 0.25, ..replay }, hover: Part::Slower, ..base.clone() }),
        ("cut-in (disabled)", Frame { replay: Replay { interactable: false, ..replay }, ..base.clone() }),
    ];
    let mut ui = SeekUi::default();
    for (i, (name, f)) in states.iter().enumerate() {
        let (output, _) = ui.frame(w, h, f);
        dump(&ui.ctx, output, &out.join(format!("frame{i:03}.bin")));
        println!("frame{i:03}: {name}");
    }
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
