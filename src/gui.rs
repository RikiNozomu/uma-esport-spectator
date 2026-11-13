//! The plugin's section in the Hachimi menu.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use crate::api::{self, cstr};
use crate::settings::{self, CLOCK, HIDE_UI, SEEK_BAR};
use crate::{clock, gate_banner, leaderboard, patches, race_ui, seek, skill_popups, track_map, tracer};

pub extern "C" fn menu_section(ui: *mut c_void, _userdata: *mut c_void) {
    let api = api::get();
    let text = |s: &str| cstr(s);
    unsafe {
        (api.gui_ui_heading)(ui, text("Race").as_ptr());
        for (i, toggle) in settings::TOGGLES.iter().enumerate() {
            let mut on = settings::is_on(i);
            if (api.gui_ui_checkbox)(ui, text(toggle.label).as_ptr(), &mut on) {
                settings::set(i, on);
            }
            let status = match i {
                CLOCK => clock::status(),
                HIDE_UI => race_ui::status(),
                SEEK_BAR => seek::status(),
                _ => patches::status(i),
            };
            (api.gui_ui_small)(ui, text(&status).as_ptr());
        }
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Skill popups").as_ptr());
        let modes = skill_popups::Mode::ALL;
        let names: Vec<_> = modes.iter().map(|m| text(m.name())).collect();
        let items: Vec<*const std::ffi::c_char> = names.iter().map(|n| n.as_ptr()).collect();
        let current = skill_popups::mode();
        let mut selected = modes.iter().position(|&m| m == current).unwrap_or(0) as i32;
        // No search buffer: three entries need no search (Hachimi still draws its box).
        let id = text("uma_spectator_skill_popup_mode");
        let changed =
            (api.gui_ui_combo_menu)(ui, id.as_ptr(), &mut selected, items.as_ptr(), items.len(), std::ptr::null_mut(), 0);
        if changed {
            if let Some(&m) = modes.get(selected as usize) {
                skill_popups::set_mode(m);
            }
        }
        (api.gui_ui_small)(ui, text(skill_popups::mode().description()).as_ptr());
        let mut trainer = skill_popups::trainer_name();
        if (api.gui_ui_checkbox)(ui, text("Show the trainer name on trainers' horses' popups").as_ptr(), &mut trainer) {
            skill_popups::set_trainer_name(trainer);
        }
        (api.gui_ui_small)(ui, text(&skill_popups::status()).as_ptr());
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Leaderboard overlay").as_ptr());
        let mut on = leaderboard::is_enabled();
        if (api.gui_ui_checkbox)(ui, text("Show during races").as_ptr(), &mut on) {
            leaderboard::set_enabled(on);
        }
        let mut preview = leaderboard::is_preview();
        if (api.gui_ui_checkbox)(ui, text("Preview with sample data").as_ptr(), &mut preview) {
            leaderboard::set_preview(preview);
        }
        let mut view = leaderboard::view();
        let mut changed = (api.gui_ui_slider_f32)(ui, text("Size").as_ptr(), &mut view.scale, 0.4, 2.5);
        changed |= (api.gui_ui_slider_f32)(ui, text("Height").as_ptr(), &mut view.y, 0.1, 1.0);
        if changed {
            leaderboard::set_view(view);
        }
        let mut hide = leaderboard::hide_before_goal();
        let label = text("Hide before goal (m, 0 = never)");
        if (api.gui_ui_slider_f32)(ui, label.as_ptr(), &mut hide, 0.0, leaderboard::MAX_HIDE_BEFORE_GOAL) {
            leaderboard::set_hide_before_goal(hide);
        }
        (api.gui_ui_small)(ui, text(&leaderboard::status()).as_ptr());
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Track map").as_ptr());
        let mut on = track_map::is_enabled();
        if (api.gui_ui_checkbox)(ui, text("Show the course map (top left) during races").as_ptr(), &mut on) {
            track_map::set_enabled(on);
        }
        let status = track_map::game::error().map(|e| format!("not available: {e}")).unwrap_or_else(|| {
            if track_map::is_enabled() { "shows while the race runs".to_owned() } else { "off".to_owned() }
        });
        (api.gui_ui_small)(ui, text(&status).as_ptr());
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Gate banner").as_ptr());
        let mut on = gate_banner::is_enabled();
        if (api.gui_ui_checkbox)(ui, text("Show a custom banner on the starting gate").as_ptr(), &mut on) {
            gate_banner::set_enabled(on);
        }
        let (status, ids) = gate_banner::game::status();
        (api.gui_ui_small)(ui, text(&status).as_ptr());
        let race = match ids {
            Some((inst, id)) => format!("This race: race id {id}, instance id {inst}"),
            None => "This race: none".to_owned(),
        };
        (api.gui_ui_small)(ui, text(&race).as_ptr());
        // Relative and short: the full path does not fit, has no spaces to wrap
        // on, and the menu stretched it letter by letter.
        (api.gui_ui_small)(ui, text("Pictures: hachimi\\uma-esport-spectator\\gate_banner").as_ptr());

        (api.gui_ui_label)(ui, text("Look").as_ptr());
        let looks = gate_banner::Look::ALL;
        let names: Vec<_> = looks.iter().map(|l| text(l.name())).collect();
        let items: Vec<*const std::ffi::c_char> = names.iter().map(|n| n.as_ptr()).collect();
        let current = gate_banner::look();
        let mut selected = looks.iter().position(|&l| l == current).unwrap_or(0) as i32;
        let id = text("uma_spectator_gate_banner_look");
        if (api.gui_ui_combo_menu)(ui, id.as_ptr(), &mut selected, items.as_ptr(), items.len(), std::ptr::null_mut(), 0) {
            if let Some(&l) = looks.get(selected as usize) {
                gate_banner::set_look(l);
            }
        }
        (api.gui_ui_small)(ui, text(gate_banner::look().description()).as_ptr());

        (api.gui_ui_label)(ui, text("Placement").as_ptr());
        let mut t = gate_banner::tuning();
        let mut changed = (api.gui_ui_slider_f32)(ui, text("Along").as_ptr(), &mut t.along, -2.0, 2.0);
        changed |= (api.gui_ui_slider_f32)(ui, text("Up").as_ptr(), &mut t.up, -0.5, 0.5);
        changed |= (api.gui_ui_slider_f32)(ui, text("Width x").as_ptr(), &mut t.width, 0.5, 1.5);
        changed |= (api.gui_ui_slider_f32)(ui, text("Height x").as_ptr(), &mut t.height, 0.5, 3.0);
        changed |= (api.gui_ui_slider_f32)(ui, text("Brightness").as_ptr(), &mut t.brightness, 0.2, 2.0);
        (api.gui_ui_small)(ui, text("Width and height are times the game's plate; height grows upwards.").as_ptr());
        changed |= (api.gui_ui_checkbox)(ui, text("Hide the game's name plate").as_ptr(), &mut t.hide_header);
        if changed {
            gate_banner::set_tuning(t);
        }
        let mut dump = gate_banner::dump_enabled();
        if (api.gui_ui_checkbox)(ui, text("Write gate_dump.txt (troubleshooting)").as_ptr(), &mut dump) {
            gate_banner::set_dump_enabled(dump);
        }
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Function trace").as_ptr());

        if !tracer::GAME_READY.load(Ordering::Acquire) {
            (api.gui_ui_label)(ui, text("Waiting for the game to finish loading...").as_ptr());
            return;
        }

        let mut record = tracer::wants_recording();
        if (api.gui_ui_checkbox)(ui, text("Record function calls").as_ptr(), &mut record) {
            tracer::set_recording(record);
        }

        let mut every = tracer::EVERY_CALL.load(Ordering::Relaxed);
        if (api.gui_ui_checkbox)(ui, text("Log every call (heavy)").as_ptr(), &mut every) {
            tracer::EVERY_CALL.store(every, Ordering::Relaxed);
        }

        (api.gui_ui_small)(ui, text(&tracer::status()).as_ptr());
        (api.gui_ui_separator)(ui);
    }
}
