//! Fonts for the overlay. egui's built-in font has no Japanese or Thai
//! glyphs, so trainer and character names would show as boxes; Windows'
//! own fonts are added as fallbacks. Missing files are skipped.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Font family for numbers and the LEADER tag.
pub const BOLD: &str = "bold";

/// On other platforms (offline previews only) set `UMA_FONT_DIR` and name
/// the files with `UMA_FONTS` / `UMA_BOLD_FONT` (`file[#index]`, `:`-separated).
fn font_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        r"C:\Windows\Fonts".into()
    } else {
        std::env::var_os("UMA_FONT_DIR").unwrap_or_default().into()
    }
}

fn env_list(var: &str) -> Vec<(String, u32)> {
    std::env::var(var)
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| match s.split_once('#') {
            Some((f, i)) => (f.to_owned(), i.parse().unwrap_or(0)),
            None => (s.to_owned(), 0),
        })
        .collect()
}

fn list(var: &str, windows: &[(&str, u32)]) -> Vec<(String, u32)> {
    if cfg!(windows) {
        windows.iter().map(|(f, i)| ((*f).to_owned(), *i)).collect()
    } else {
        env_list(var)
    }
}

/// (file, face index in a .ttc) tried in order; every one found is added.
const FALLBACKS: &[(&str, u32)] = &[
    ("YuGothM.ttc", 0),  // Japanese
    ("meiryo.ttc", 0),   // Japanese (older Windows)
    ("msgothic.ttc", 0), // Japanese (last resort)
    ("LeelawUI.ttf", 0), // Thai
    ("tahoma.ttf", 0),   // Thai (older Windows)
    ("malgun.ttf", 0),   // Korean
    ("msyh.ttc", 0),     // Chinese
];

/// Condensed bold faces for the gate numbers, first found wins.
const BOLD_FACES: &[(&str, u32)] = &[("bahnschrift.ttf", 0), ("segoeuib.ttf", 0), ("arialbd.ttf", 0)];

fn load(file: &str, index: u32) -> Option<FontData> {
    let bytes = std::fs::read(font_dir().join(file)).ok()?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    Some(data)
}

/// Installs the fonts. Returns whether a bold face was found.
pub fn install(ctx: &egui::Context) -> bool {
    let mut defs = FontDefinitions::default();

    for (file, index) in list("UMA_FONTS", FALLBACKS) {
        if let Some(data) = load(&file, index) {
            defs.font_data.insert(file.clone(), Arc::new(data));
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families.entry(family).or_default().push(file.clone());
            }
        }
    }

    let bold = list("UMA_BOLD_FONT", BOLD_FACES).into_iter().find_map(|(file, index)| load(&file, index).map(|d| (file, d)));
    let has_bold = bold.is_some();
    let mut bold_family: Vec<String> = Vec::new();
    if let Some((file, data)) = bold {
        let name = format!("bold:{file}");
        defs.font_data.insert(name.clone(), Arc::new(data));
        bold_family.push(name);
    }
    // Fall back to the normal fonts for any glyph the bold face lacks.
    bold_family.extend(defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    defs.families.insert(FontFamily::Name(BOLD.into()), bold_family);

    ctx.set_fonts(defs);
    has_bold
}
