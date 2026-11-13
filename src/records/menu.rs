//! The "Record cards" menu section: on/off, the race on screen and its
//! records, preview, and an editor (holder, time, date, picture, race name)
//! for every record in records.txt, plus new records for the race on screen.
//!
//! The editor needs Hachimi's text fields and buttons (`api::extra()`); older
//! Hachimi builds lack them, then the section explains how to edit the file.
//! Hachimi keys a text field by its buffer's address and keeps its own copy of
//! the text: loading another record into the same buffer would not show. Each
//! load therefore allocates new buffers (`Fields::new`).
//!
//! Runs on the render thread: no game calls (the race comes from `game::current`).

use std::ffi::{c_char, c_void};
use std::sync::Mutex;

use super::{game, Course, Key, Record};
use crate::api::{self, cstr};

const TEXT_LEN: usize = 96;

struct Fields {
    name: Box<[u8; TEXT_LEN]>,
    holder: Box<[u8; TEXT_LEN]>,
    time: Box<[u8; TEXT_LEN]>,
    date: Box<[u8; TEXT_LEN]>,
    event: Box<[u8; TEXT_LEN]>,
    picture: String,
}

fn buf(s: &str) -> Box<[u8; TEXT_LEN]> {
    let mut b = Box::new([0u8; TEXT_LEN]);
    let bytes = s.as_bytes();
    let n = bytes.len().min(TEXT_LEN - 1);
    b[..n].copy_from_slice(&bytes[..n]);
    b
}

fn text_of(b: &[u8; TEXT_LEN]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(TEXT_LEN);
    String::from_utf8_lossy(&b[..end]).trim().to_owned()
}

impl Fields {
    fn new(r: &Record) -> Fields {
        let name = match &r.key {
            Key::Race { name, .. } => name.as_str(),
            Key::Track(_) => "",
        };
        Fields {
            name: buf(name),
            holder: buf(&r.holder),
            time: buf(&r.time),
            date: buf(&r.date),
            event: buf(&r.event),
            picture: r.picture.clone(),
        }
    }
}

/// What a menu entry edits.
#[derive(Clone)]
enum Target {
    Existing(usize),
    NewTrack(Course),
    NewRace(i32),
}

struct Editor {
    /// Label of the entry being edited, and its fields.
    loaded: String,
    fields: Option<Fields>,
    message: String,
}

static EDITOR: Mutex<Editor> = Mutex::new(Editor { loaded: String::new(), fields: None, message: String::new() });

fn blank(key: Key) -> Record {
    Record {
        key,
        holder: String::new(),
        time: String::new(),
        date: String::new(),
        event: String::new(),
        picture: String::new(),
    }
}

/// The editor's entries: new records for the race on screen first (when it
/// has none yet), then every record of the file.
fn entries(records: &[Record]) -> Vec<(String, Target)> {
    let mut out = Vec::new();
    if let Some(now) = game::current() {
        let (track, race) = super::find(records, &now);
        if let (None, Some(c)) = (track, now.course) {
            out.push((format!("+ New track record: {}", c.label()), Target::NewTrack(c)));
        }
        if race.is_none() {
            out.push((format!("+ New race record for race id {}", now.race), Target::NewRace(now.race)));
        }
    }
    out.extend(records.iter().enumerate().map(|(i, r)| (r.menu_name(), Target::Existing(i))));
    out
}

fn record_for(records: &[Record], t: &Target) -> Record {
    match t {
        Target::Existing(i) => records[*i].clone(),
        Target::NewTrack(c) => blank(Key::Track(*c)),
        Target::NewRace(id) => blank(Key::Race { name: String::new(), ids: vec![*id] }),
    }
}

pub unsafe fn draw(ui: *mut c_void) {
    let api = api::get();
    let extra = api::extra();
    let text = |s: &str| cstr(s);
    let small = |s: &str| (api.gui_ui_small)(ui, text(s).as_ptr());

    (api.gui_ui_heading)(ui, text("Record cards").as_ptr());
    let mut on = super::is_enabled();
    if (api.gui_ui_checkbox)(ui, text("Show track / race records when the gates start loading (5 s)").as_ptr(), &mut on) {
        super::set_enabled(on);
    }
    let mut secs = super::seconds();
    if (api.gui_ui_slider_f32)(ui, text("Show for (seconds)").as_ptr(), &mut secs, super::MIN_SECONDS, super::MAX_SECONDS) {
        super::set_seconds(secs);
    }
    small(&format!("Hook: {}", game::status()));
    if let Some(e) = super::ugui::error() {
        small(&format!("Cards: {e}"));
    }

    let records = super::records();
    match game::current() {
        Some(now) => {
            let course = now.course.map(|c| c.label()).unwrap_or_else(|| "course unknown".to_owned());
            small(&format!("This race: {course}, race id {}, instance id {}", now.race, now.instance));
            let (track, race) = super::find(&records, &now);
            let line = |r: Option<&Record>, what: &str| match r {
                Some(r) => format!("{what}: {} {} ({})", r.holder, r.time, super::date_line(&r.date, &r.event)),
                None => format!("{what}: none"),
            };
            small(&line(track, "Track record"));
            small(&line(race, "Race record"));
        }
        None => {
            small("This race: none yet (open a race to see its records)");
        }
    }

    // Preview: a button when Hachimi has them, else a checkbox that resets.
    let preview = match extra.gui_ui_button {
        Some(button) => button(ui, text("Preview the cards").as_ptr()),
        None => {
            let mut p = false;
            (api.gui_ui_checkbox)(ui, text("Preview the cards").as_ptr(), &mut p)
        }
    };
    if preview {
        super::preview();
    }

    let (Some(button), Some(text_edit)) = (extra.gui_ui_button, extra.gui_ui_text_edit_singleline) else {
        small("Edit hachimi\\uma-esport-spectator\\records\\records.txt (reloads by itself); this Hachimi version has no text fields for the menu editor.");
        (api.gui_ui_separator)(ui);
        return;
    };

    (api.gui_ui_label)(ui, text("Edit a record").as_ptr());
    let list = entries(&records);
    let mut editor = EDITOR.lock().unwrap_or_else(|e| e.into_inner());
    let mut selected = list.iter().position(|(l, _)| *l == editor.loaded).unwrap_or(0) as i32;
    let names: Vec<_> = list.iter().map(|(l, _)| text(l)).collect();
    let items: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
    let id = text("uma_spectator_record_edit");
    (api.gui_ui_combo_menu)(ui, id.as_ptr(), &mut selected, items.as_ptr(), items.len(), std::ptr::null_mut(), 0);
    let Some((label, target)) = list.get(selected.max(0) as usize).cloned() else {
        (api.gui_ui_separator)(ui);
        return;
    };
    let current = record_for(&records, &target);
    if editor.loaded != label || editor.fields.is_none() {
        editor.loaded = label.clone();
        editor.fields = Some(Fields::new(&current));
        editor.message.clear();
    }
    let ed = &mut *editor;
    let f = ed.fields.as_mut().expect("just set");

    let field = |name: &str, b: &mut [u8; TEXT_LEN]| {
        (api.gui_ui_label)(ui, text(name).as_ptr());
        text_edit(ui, b.as_mut_ptr() as *mut c_char, TEXT_LEN);
    };
    if let Key::Race { ids, .. } = &current.key {
        field("Race name (RACE RECORD : ...)", &mut f.name);
        let ids: Vec<String> = ids.iter().map(i32::to_string).collect();
        small(&format!("Race ids: {}", ids.join(", ")));
    } else {
        small(&current.heading());
    }
    field("Holder", &mut f.holder);
    field("Time (e.g. 2:20.6)", &mut f.time);
    field("Date (e.g. 2018/11/25)", &mut f.date);
    field("Set in which race (e.g. Japan Cup)", &mut f.event);

    (api.gui_ui_label)(ui, text("Picture (PNG in the records folder)").as_ptr());
    let mut pictures = vec!["(no picture)".to_owned()];
    pictures.extend(super::pictures());
    if !f.picture.is_empty() && !pictures.contains(&f.picture) {
        pictures.push(f.picture.clone());
    }
    let mut pic = pictures.iter().position(|p| *p == f.picture).unwrap_or(0) as i32;
    let pic_names: Vec<_> = pictures.iter().map(|p| text(p)).collect();
    let pic_items: Vec<*const c_char> = pic_names.iter().map(|n| n.as_ptr()).collect();
    let pic_id = text("uma_spectator_record_picture");
    if (api.gui_ui_combo_menu)(ui, pic_id.as_ptr(), &mut pic, pic_items.as_ptr(), pic_items.len(), std::ptr::null_mut(), 0) {
        f.picture = if pic <= 0 { String::new() } else { pictures[pic as usize].clone() };
    }
    small("Pictures go in hachimi\\uma-esport-spectator\\records (transparent background works best); none: the card stretches.");

    // The record as edited.
    let mut edited = current.clone();
    edited.holder = text_of(&f.holder);
    edited.time = text_of(&f.time);
    edited.date = text_of(&f.date);
    edited.event = text_of(&f.event);
    edited.picture = f.picture.clone();
    if let Key::Race { name, .. } = &mut edited.key {
        *name = text_of(&f.name);
    }

    // This race's id added to a race record of the same race in another mode.
    if let (Some(now), Target::Existing(_), Key::Race { ids, .. }) = (game::current(), &target, &edited.key) {
        if !ids.contains(&now.race) && button(ui, text(&format!("Use this record for race id {} too", now.race)).as_ptr()) {
            if let Key::Race { ids, .. } = &mut edited.key {
                ids.push(now.race);
            }
            ed.message = save_record(&records, &target, edited.clone(), &mut ed.loaded);
        }
    }

    if button(ui, text("Save").as_ptr()) {
        ed.message = match &edited.key {
            Key::Race { name, .. } if name.is_empty() => "Give the race a name first.".to_owned(),
            _ => save_record(&records, &target, edited.clone(), &mut ed.loaded),
        };
    }
    if button(ui, text("Preview this card").as_ptr()) {
        super::preview_record(&edited);
    }
    if let Target::Existing(i) = target {
        if button(ui, text("Delete this record").as_ptr()) {
            let mut all = records.clone();
            all.remove(i);
            super::save(all);
            ed.loaded.clear();
            ed.fields = None;
            ed.message = "Deleted.".to_owned();
        }
    }
    if !ed.message.is_empty() {
        small(&ed.message);
    }
    (api.gui_ui_separator)(ui);
}

/// Saves `edited` in place of `target`; returns the message to show and
/// points `loaded` at the saved record's entry.
fn save_record(records: &[Record], target: &Target, edited: Record, loaded: &mut String) -> String {
    let mut all = records.to_vec();
    match target {
        Target::Existing(i) => all[*i] = edited.clone(),
        Target::NewTrack(_) | Target::NewRace(_) => all.push(edited.clone()),
    }
    super::save(all);
    *loaded = edited.menu_name();
    "Saved.".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_round_trip_and_truncate() {
        assert_eq!(text_of(&buf("Almond Eye")), "Almond Eye");
        let long = "x".repeat(200);
        assert_eq!(text_of(&buf(&long)).len(), TEXT_LEN - 1);
        assert_eq!(text_of(&buf("  2:20.6 ")), "2:20.6");
    }
}
