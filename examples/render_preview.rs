//! Dumps tessellated leaderboard frames for an offline look at the design:
//! `cargo run --example render_preview -- <out dir> [image dir]`, then
//! `python3 tools/rasterize.py <out dir>`. Not part of the plugin.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use uma_esport_spectator::leaderboard::{Board, View, DEFAULT_Y};
use uma_esport_spectator::race::Runner;

const NAMES: [(&str, &str); 18] = [
    ("Special Week", "ポン"), ("Silence Suzuka", "RikiNozomu"), ("Tokai Teio", "โอตะม้าซิ่ง"),
    ("Maruzensky", "Trainer04"), ("Oguri Cap", "たづな"), ("Gold Ship", "GoldFan"),
    ("Vodka", "Trainer07"), ("Daiwa Scarlet", "สมชาย"), ("Taiki Shuttle", "Trainer09"),
    ("Grass Wonder", "Trainer10"), ("El Condor Pasa", "CondorP"), ("Symboli Rudolf", "Emperor"),
    ("Rice Shower", "Trainer13"), ("Mejiro McQueen", "マックイーン推し"), ("Agnes Tachyon", "Lab"),
    ("Kitasan Black", "Trainer16"), ("Satono Diamond", "Dia"), ("Haru Urara", "Urara-fan"),
];

fn field(order: &[usize]) -> Vec<Runner> {
    order
        .iter()
        .enumerate()
        .map(|(pos, &i)| Runner {
            gate: i as i32 + 1,
            order: pos as i32,
            chara_id: 1001 + i as i32,
            card_id: (1001 + i as i32) * 100 + 1,
            name: NAMES[i].0.to_owned(),
            trainer: NAMES[i].1.to_owned(),
            finished: false,
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("out dir"));
    let images = PathBuf::from(args.next().unwrap_or_else(|| "/nonexistent".into()));
    std::fs::create_dir_all(&out).unwrap();

    let before: Vec<usize> = vec![5, 15, 3, 0, 9, 12, 1, 7, 2, 14, 4, 10, 6, 13, 8, 11, 16];
    // Positions 2<->3 and 5<->6 swap at t = 1.0.
    let mut after = before.clone();
    after.swap(2, 3);
    after.swap(5, 6);

    let mut board = Board::new(images);
    let view = View { scale: 1.0, y: DEFAULT_Y };
    let (w, h) = (1920u32, 1080u32);
    let fps = 30.0;
    let mut frame_no = 0;
    for i in 0..=(2.2 * fps) as usize {
        let t = i as f64 / fps;
        let runners = field(if t < 1.0 { &before } else { &after });
        let output = board.frame_at(w, h, &runners, view, t);
        let ppp = output.pixels_per_point;
        let prims = board.ctx.tessellate(output.shapes, ppp);
        let mut f = BufWriter::new(File::create(out.join(format!("frame{frame_no:03}.bin"))).unwrap());
        // Texture uploads: full images and partial (font atlas) updates.
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
        frame_no += 1;
    }
    println!("{frame_no} frames ({w}x{h}) in {}", out.display());
}
