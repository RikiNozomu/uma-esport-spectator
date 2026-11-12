//! Skill popups: which skill-name plates show during races, and whether they
//! carry the trainer's name.
//!
//! Modes (`skill_popups` in settings.txt):
//! - Default: every skill of player-owned horses (a trainer name), only gold
//!   and unique skills (rarity >= 2) of NPC runners.
//! - All: every skill of every horse.
//! - Off: no skill plates and no skill info (the old "Hide skill popups").
//! Toggle `skill_popup_trainer_name`: player-owned horses' plates show the
//! trainer name.
//!
//! The game's path in replays (function trace): RaceEventPlayer asks
//! `IsShowSkillEnemy(teamId, horseIndex, isMob)` for every horse that is not
//! the viewer's, then calls `RaceManagerReplayBase::ShowMessageUISkill(horse,
//! skillId, isEnemy)` (one per skill plate) and `RaceMainViewController::
//! PlaySkillInfo`. `IsShowSkillEnemy` is forced true (Default, All) and each
//! plate is decided in `ShowMessageUISkill` (trainer, skill master rarity).
//! Other runners' white skills never reach ShowMessageUISkill: activations
//! seen in `SkillManager::AddUsedSkillId` that the game did not show are
//! shown by us (see "skipped plates"). The plate methods under
//! ShowMessageUISkill (`RequestShowMessagePlate`, `GetMessagePlateType`,
//! `MessagePlateCtrl::PlayIn`) are inlined: hooks there never run.
//!
//! Trainer name: the game puts it on *enemy* plates (portrait on the left,
//! name bar under the skill), so the toggle shows trainers' horses' plates as
//! enemy plates. The enemy plate of a white skill is a pastel rainbow, so the
//! virtual `MessagePlate*::InitTextureSkill` (never inlined) picks the normal
//! white texture for those. `PlayDesc` is a class (reference), not a struct.
//!
//! Every hook goes through `il2cpp::hook_unique` (code shared with another
//! method is never hooked) and runs on the main thread.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::il2cpp::{self, Method, Obj, GAME};
use crate::{api, settings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Default = 0,
    All = 1,
    Off = 2,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Default, Mode::All, Mode::Off];

    pub fn key(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::All => "all",
            Mode::Off => "off",
        }
    }

    /// Dropdown entry (Hachimi's dropdown is narrow).
    pub fn name(self) -> &'static str {
        match self {
            Mode::Default => "Default",
            Mode::All => "All",
            Mode::Off => "Off",
        }
    }

    /// Shown under the dropdown for the selected mode.
    pub fn description(self) -> &'static str {
        match self {
            Mode::Default => "Every skill of trainers' horses, gold and unique skills of NPC runners.",
            Mode::All => "Every skill of every horse.",
            Mode::Off => "No skill popups and no skill info.",
        }
    }

    /// From settings.txt; `hide_skill_popups = true` (before 1.2.1) means Off.
    pub fn from_settings(value: Option<&str>, old_hide: bool) -> Mode {
        match value {
            Some(v) => Mode::ALL.into_iter().find(|m| m.key() == v).unwrap_or(Mode::Default),
            None if old_hide => Mode::Off,
            None => Mode::Default,
        }
    }

    fn from_u8(v: u8) -> Mode {
        Mode::ALL.get(v as usize).copied().unwrap_or(Mode::Default)
    }
}

/// Opponent plates come in three types, but the game's white-skill one is a
/// pastel rainbow (it never shows opponents' white skills itself). True =
/// use the normal plate type instead, so white skills look white; gold and
/// unique keep their opponent plates. `rarity`: SkillRarity (1 = white).
pub fn white_plate_for_opponent(rarity: i32, is_enemy: bool) -> bool {
    is_enemy && rarity == 1
}

/// Whether a skill plate shows. `owned` = the horse has a trainer;
/// `rarity` = skill master rarity (1 white, 2 gold, 3+ unique), 0 if unknown.
pub fn show_plate(mode: Mode, owned: bool, rarity: i32) -> bool {
    match mode {
        Mode::Off => false,
        Mode::All => true,
        // Unknown rarity: show rather than hide a possibly important skill.
        Mode::Default => owned || rarity >= 2 || rarity <= 0,
    }
}

const KEY_MODE: &str = "skill_popups";
const KEY_TRAINER: &str = "skill_popup_trainer_name";
const KEY_OLD_HIDE: &str = "hide_skill_popups";

static MODE: AtomicU8 = AtomicU8::new(Mode::Default as u8);
static TRAINER: AtomicBool = AtomicBool::new(false);

pub fn mode() -> Mode {
    Mode::from_u8(MODE.load(Ordering::Relaxed))
}

pub fn set_mode(m: Mode) {
    MODE.store(m as u8, Ordering::Relaxed);
    settings::set_value(KEY_MODE, m.key());
}

pub fn trainer_name() -> bool {
    TRAINER.load(Ordering::Relaxed)
}

pub fn set_trainer_name(on: bool) {
    TRAINER.store(on, Ordering::Relaxed);
    settings::set_value(KEY_TRAINER, on);
}

/// Reads the settings (after settings::load).
pub fn load() {
    let m = Mode::from_settings(settings::get_string(KEY_MODE).as_deref(), settings::get_bool(KEY_OLD_HIDE, false));
    MODE.store(m as u8, Ordering::Relaxed);
    TRAINER.store(settings::get_bool(KEY_TRAINER, false), Ordering::Relaxed);
}

// ---------------------------------------------------------------- game side

struct Game {
    horse_info_class: usize,
    horse_data: Method,
    trainer_name: Method,
    /// MasterDataManager (singleton), its skill table getter, the table's Get(id).
    master_manager_class: usize,
    skill_table: Method,
    skill_get: Method,
    rarity_off: usize,
    plate_type_off: usize,
    /// RaceManagerReplayBase::IsCutInPlayingOrReserved
    cut_in: Method,
}

unsafe impl Send for Game {}
unsafe impl Sync for Game {}

/// Plate colour: `MessagePlate._playDesc` (a PlayDesc object) and its fields,
/// and static `RaceUIMessagePlate.GetMessagePlateType(rarity, plateType, isEnemy)`.
struct TexFix {
    desc_off: usize,
    desc_class: usize,
    desc_type: usize,
    desc_master: usize,
    desc_enemy: usize,
    get_type: Method,
}

unsafe impl Send for TexFix {}
unsafe impl Sync for TexFix {}

static GAME_DATA: OnceLock<Game> = OnceLock::new();
static TEX_FIX: OnceLock<TexFix> = OnceLock::new();
/// `SkillManager`'s owner field (IHorseRaceInfo): for plates the game skips.
static OWNER_OFF: OnceLock<usize> = OnceLock::new();
static STATUS: OnceLock<String> = OnceLock::new();

static ORIG_ENEMY: AtomicUsize = AtomicUsize::new(0);
static ORIG_SKILL: AtomicUsize = AtomicUsize::new(0);
/// ShowMessageUISkill's MethodInfo, to call it for skipped skills.
static SKILL_INFO: AtomicUsize = AtomicUsize::new(0);
static ORIG_INFO: AtomicUsize = AtomicUsize::new(0);
static ORIG_USED: AtomicUsize = AtomicUsize::new(0);
static ORIG_TEXTURE: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

/// The first events of each kind are logged, to check them in hachimi.log.
const LOG_FIRST: u8 = 12;
static LOGGED: AtomicU8 = AtomicU8::new(0);
static LOGGED_TEX: AtomicU8 = AtomicU8::new(0);
static LOGGED_ADDED: AtomicU8 = AtomicU8::new(0);

pub fn status() -> String {
    STATUS.get().cloned().unwrap_or_else(|| "waiting for the game".to_owned())
}

unsafe fn call0<T>(m: Method, this: Obj) -> T {
    let f: extern "C" fn(Obj, usize) -> T = std::mem::transmute(m.code);
    f(this, m.info)
}

impl Game {
    unsafe fn resolve() -> Result<Game, String> {
        use il2cpp::{class, expect, field_offset, method, nested};
        let info = class(GAME, "Gallop", "HorseRaceInfo")?;
        let data = class(GAME, "Gallop", "HorseData")?;
        let manager = class(GAME, "Gallop", "MasterDataManager")?;
        let table = class(GAME, "Gallop", "MasterSkillData")?;
        let skill_data = nested(table, "SkillData")?;
        let rm = class(GAME, "Gallop", "RaceManagerReplayBase")?;

        let horse_data = method(info, "get_HorseData", 0)?;
        expect(horse_data, false, &["Gallop.HorseData"], 0)?;
        let trainer_name = method(data, "get_TrainerName", 0)?;
        expect(trainer_name, false, &["System.String"], 0)?;
        let skill_table = method(manager, "get_masterSkillData", 0)?;
        expect(skill_table, false, &["Gallop.MasterSkillData"], 0)?;
        let skill_get = il2cpp::method_params(table, "Get", &["System.Int32"])?;
        expect(skill_get, false, &["Gallop.MasterSkillData.SkillData"], 1)?;
        let cut_in = method(rm, "IsCutInPlayingOrReserved", 0)?;
        expect(cut_in, false, &["System.Boolean"], 0)?;
        Ok(Game {
            horse_info_class: info as usize,
            horse_data,
            trainer_name,
            master_manager_class: manager as usize,
            skill_table,
            skill_get,
            rarity_off: field_offset(skill_data, "Rarity")?,
            plate_type_off: field_offset(skill_data, "PlateType")?,
            cut_in,
        })
    }

    /// The horse's trainer name (managed string), null for NPC runners.
    unsafe fn trainer(&self, info: Obj) -> Obj {
        if !il2cpp::is_instance(info, self.horse_info_class as *mut c_void) {
            return std::ptr::null_mut();
        }
        let data: Obj = call0(self.horse_data, info);
        if data.is_null() {
            return std::ptr::null_mut();
        }
        let name: Obj = call0(self.trainer_name, data);
        if name.is_null() || (api::get().il2cpp_string_length)(name) <= 0 {
            return std::ptr::null_mut();
        }
        name
    }

    /// Skill master rarity (1 white, 2 gold, 3+ unique); 0 if unknown.
    unsafe fn rarity(&self, skill_id: i32) -> i32 {
        let manager = (api::get().il2cpp_get_singleton_like_instance)(self.master_manager_class as *mut c_void);
        if manager.is_null() {
            return 0;
        }
        let table: Obj = call0(self.skill_table, manager);
        if table.is_null() {
            return 0;
        }
        let get: extern "C" fn(Obj, i32, usize) -> Obj = std::mem::transmute(self.skill_get.code);
        let record = get(table, skill_id, self.skill_get.info);
        if record.is_null() { 0 } else { il2cpp::read::<i32>(record, self.rarity_off) }
    }
}

impl TexFix {
    unsafe fn resolve() -> Result<TexFix, String> {
        use il2cpp::{class, expect, field_offset, method_params, nested};
        let plate_ui = class(GAME, "Gallop", "RaceUIMessagePlate")?;
        let desc = nested(plate_ui, "PlayDesc")?;
        let plate = class(GAME, "Gallop", "MessagePlate")?;
        let get_type = method_params(
            plate_ui,
            "GetMessagePlateType",
            &["Gallop.SkillDefine.SkillRarity", "Gallop.SkillDefine.SkillPlateType", "System.Boolean"],
        )?;
        expect(get_type, true, &["Gallop.RaceUIMessagePlate.MessagePlateType"], 3)?;
        Ok(TexFix {
            desc_off: field_offset(plate, "_playDesc")?,
            desc_class: desc as usize,
            desc_type: field_offset(desc, "Type")?,
            desc_master: field_offset(desc, "MasterSkill")?,
            desc_enemy: field_offset(desc, "IsEnemy")?,
            get_type,
        })
    }

    /// Before a plate picks its texture: a white skill on an opponent plate
    /// gets the normal type. Returns (desc, old type) to put back afterwards.
    unsafe fn before(&self, g: &Game, plate: Obj) -> Option<(Obj, i32)> {
        let desc: Obj = il2cpp::read(plate, self.desc_off);
        if !il2cpp::is_instance(desc, self.desc_class as *mut c_void) {
            return None;
        }
        let master: Obj = il2cpp::read(desc, self.desc_master);
        if master.is_null() {
            return None;
        }
        let enemy = il2cpp::read::<u8>(desc, self.desc_enemy) != 0;
        let rarity: i32 = il2cpp::read(master, g.rarity_off);
        if !white_plate_for_opponent(rarity, enemy) {
            return None;
        }
        let plate_type: i32 = il2cpp::read(master, g.plate_type_off);
        let f: extern "C" fn(i32, i32, u8, usize) -> i32 = std::mem::transmute(self.get_type.code);
        let normal = f(rarity, plate_type, 0, self.get_type.info);
        let old: i32 = il2cpp::read(desc, self.desc_type);
        write(desc, self.desc_type, normal);
        if LOGGED_TEX.fetch_add(1, Ordering::Relaxed) < LOG_FIRST {
            api::get().info(&format!("skill popups: white skill on an opponent plate: texture of type {normal} instead of {old}"));
        }
        Some((desc, old))
    }
}

unsafe fn write<T: Copy>(obj: Obj, offset: usize, v: T) {
    std::ptr::write_unaligned((obj as *mut u8).add(offset) as *mut T, v)
}

/// What to do with one skill plate: None = hide, Some(is_enemy) = show with
/// that enemy flag (true puts the trainer name on the plate).
unsafe fn decide(g: &Game, info: Obj, skill_id: i32, is_enemy: bool) -> Option<bool> {
    let mode = mode();
    if mode == Mode::Off {
        return None;
    }
    let owned = !g.trainer(info).is_null();
    let rarity = if mode == Mode::Default && !owned { g.rarity(skill_id) } else { 0 };
    let show = show_plate(mode, owned, rarity);
    let enemy = is_enemy || (trainer_name() && owned);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < LOG_FIRST {
        api::get().info(&format!(
            "skill popups: skill {skill_id} trainer={owned} rarity={rarity} enemy={is_enemy}->{enemy} {mode:?} -> {}",
            if show { "shown" } else { "hidden" }
        ));
    }
    show.then_some(enemy)
}

// ---------------------------------------------------------------- skipped plates
//
// For other runners the game only shows some skills (white ones never reach
// ShowMessageUISkill). Every activation of every horse goes through
// `SkillManager::AddUsedSkillId`; an activation that has not reached
// ShowMessageUISkill `WAIT_FRAMES` later is shown by us, through the same
// filter. Uniques are left to the game (their plate comes after the cut-in).

/// (horse info, skill id, frame it was used)
static PENDING: Mutex<Vec<(usize, i32, u64)>> = Mutex::new(Vec::new());
static FRAME: AtomicU64 = AtomicU64::new(0);
static RACE: AtomicUsize = AtomicUsize::new(0);
static GEN: AtomicU32 = AtomicU32::new(0);
const WAIT_FRAMES: u64 = 2;
const MAX_PENDING: usize = 64;
/// Dropped if still waiting after this many frames (a long cut-in, a pause).
const GIVE_UP_FRAMES: u64 = 600;

fn pending() -> std::sync::MutexGuard<'static, Vec<(usize, i32, u64)>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// The game is showing (or deliberately hiding) this plate: nothing to add.
fn mark_handled(info: Obj, skill_id: i32) {
    pending().retain(|&(i, s, _)| !(i == info as usize && s == skill_id));
}

/// Once per race frame (main thread, from clock's AlterUpdate hook).
pub fn tick(rm: Obj) {
    if rm.is_null() || ORIG_USED.load(Ordering::Acquire) == 0 {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe { tick_inner(rm) }));
}

unsafe fn tick_inner(rm: Obj) {
    let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
    let gen = crate::race_ui::race_gen();
    if RACE.swap(rm as usize, Ordering::Relaxed) != rm as usize || GEN.swap(gen, Ordering::Relaxed) != gen {
        pending().clear();
        return;
    }
    let Some(g) = GAME_DATA.get() else { return };
    let due: Vec<(usize, i32)> = {
        let mut list = pending();
        if list.is_empty() {
            return;
        }
        if mode() == Mode::Off {
            list.clear();
            return;
        }
        list.retain(|&(_, _, f)| frame.saturating_sub(f) < GIVE_UP_FRAMES);
        // Like the game: no skill plate while a cut-in plays or waits.
        if call0::<u8>(g.cut_in, rm) != 0 {
            return;
        }
        let (due, wait): (Vec<_>, Vec<_>) = list.drain(..).partition(|&(_, _, f)| frame.saturating_sub(f) >= WAIT_FRAMES);
        *list = wait;
        due.into_iter().map(|(i, s, _)| (i, s)).collect()
    };
    let mi = SKILL_INFO.load(Ordering::Relaxed) as *const c_void;
    for (info, skill_id) in due {
        if g.rarity(skill_id) >= 3 {
            continue;
        }
        if LOGGED_ADDED.fetch_add(1, Ordering::Relaxed) < LOG_FIRST {
            api::get().info(&format!("skill popups: skill {skill_id} not shown by the game, adding its plate"));
        }
        // Through our own hook: the mode filter and the trainer name apply.
        show_message_ui_skill(rm, info as Obj, skill_id, 1, mi);
    }
}

// ---------------------------------------------------------------- hooks

/// `bool RaceEventPlayer::IsShowSkillEnemy(int teamId, int horseIndex, bool isMob)`
extern "C" fn is_show_skill_enemy(this: Obj, team: i32, idx: i32, is_mob: u8, mi: *const c_void) -> u8 {
    if matches!(mode(), Mode::Default | Mode::All) {
        return 1;
    }
    let f: extern "C" fn(Obj, i32, i32, u8, *const c_void) -> u8 = unsafe { std::mem::transmute(ORIG_ENEMY.load(Ordering::Acquire)) };
    f(this, team, idx, is_mob, mi)
}

/// `void RaceManagerReplayBase::ShowMessageUISkill(IHorseRaceInfo horse, int skillId, bool isEnemy)`:
/// one call per skill plate (status plates use other methods).
extern "C" fn show_message_ui_skill(this: Obj, info: Obj, skill_id: i32, is_enemy: u8, mi: *const c_void) {
    mark_handled(info, skill_id);
    let decision = match GAME_DATA.get() {
        Some(g) => catch_unwind(AssertUnwindSafe(|| unsafe { decide(g, info, skill_id, is_enemy != 0) })).unwrap_or(Some(is_enemy != 0)),
        None => Some(is_enemy != 0),
    };
    let Some(enemy) = decision else { return };
    let f: extern "C" fn(Obj, Obj, i32, u8, *const c_void) = unsafe { std::mem::transmute(ORIG_SKILL.load(Ordering::Acquire)) };
    f(this, info, skill_id, enemy as u8, mi)
}

/// `void SkillManager::AddUsedSkillId(int skillId)`: every skill activation of every horse.
extern "C" fn add_used_skill_id(this: Obj, skill_id: i32, mi: *const c_void) {
    let f: extern "C" fn(Obj, i32, *const c_void) = unsafe { std::mem::transmute(ORIG_USED.load(Ordering::Acquire)) };
    f(this, skill_id, mi);
    if mode() == Mode::Off || this.is_null() {
        return;
    }
    if let Some(&off) = OWNER_OFF.get() {
        let owner: Obj = unsafe { il2cpp::read(this, off) };
        let mut list = pending();
        if !owner.is_null() && list.len() < MAX_PENDING {
            list.push((owner as usize, skill_id, FRAME.load(Ordering::Relaxed)));
        }
    }
}

/// `void MessagePlate*::InitTextureSkill()` (virtual): the plate picks its texture.
fn init_texture_skill(slot: usize, this: Obj, mi: *const c_void) {
    let restore = match (GAME_DATA.get(), TEX_FIX.get()) {
        (Some(g), Some(t)) if !this.is_null() => catch_unwind(AssertUnwindSafe(|| unsafe { t.before(g, this) })).ok().flatten(),
        _ => None,
    };
    let f: extern "C" fn(Obj, *const c_void) = unsafe { std::mem::transmute(ORIG_TEXTURE[slot].load(Ordering::Acquire)) };
    f(this, mi);
    if let (Some((desc, old)), Some(t)) = (restore, TEX_FIX.get()) {
        unsafe { write(desc, t.desc_type, old) };
    }
}

extern "C" fn init_texture_skill_0(this: Obj, mi: *const c_void) {
    init_texture_skill(0, this, mi)
}

extern "C" fn init_texture_skill_1(this: Obj, mi: *const c_void) {
    init_texture_skill(1, this, mi)
}

/// `void RaceMainViewController::PlaySkillInfo(int horseIndex, bool isDebuff, SkillCategory category)`
extern "C" fn play_skill_info(this: Obj, idx: i32, debuff: u8, category: i32, mi: *const c_void) {
    if mode() == Mode::Off {
        return;
    }
    let f: extern "C" fn(Obj, i32, u8, i32, *const c_void) = unsafe { std::mem::transmute(ORIG_INFO.load(Ordering::Acquire)) };
    f(this, idx, debuff, category, mi)
}

// ---------------------------------------------------------------- install

/// Hooks `class::name(params)` returning `ret`; the line for the status text.
unsafe fn install_hook(class: &str, name: &str, params: &[&str], ret: &str, detour: usize, slot: &AtomicUsize) -> Result<Method, String> {
    let m = il2cpp::class(GAME, "Gallop", class)
        .and_then(|k| il2cpp::method_params(k, name, params))
        .and_then(|m| il2cpp::expect(m, false, &[ret], params.len() as u32).map(|_| m))?;
    let t = il2cpp::hook_unique(m, detour)?;
    slot.store(t, Ordering::Release);
    Ok(m)
}

/// At game ready (main thread), after settings::load.
pub fn install() {
    load();
    let mut status = String::new();
    let mut note = |what: &str, r: Result<(), String>| {
        let line = match r {
            Ok(()) => format!("{what}: ok"),
            Err(e) => {
                api::get().error(&format!("skill popups: {what}: {e}"));
                format!("{what}: FAILED ({e})")
            }
        };
        let _ = writeln!(status, "{line}");
    };
    const I32: &str = "System.Int32";
    const BOOL: &str = "System.Boolean";
    const VOID: &str = "System.Void";

    let game = unsafe { Game::resolve() };
    let filter_ok = game.is_ok();
    note("filter", game.map(|g| drop(GAME_DATA.set(g))));

    unsafe {
        let r = install_hook(
            "RaceManagerReplayBase",
            "ShowMessageUISkill",
            &["Gallop.IHorseRaceInfo", I32, BOOL],
            VOID,
            show_message_ui_skill as *const () as usize,
            &ORIG_SKILL,
        );
        note("ShowMessageUISkill", r.map(|m| SKILL_INFO.store(m.info, Ordering::Relaxed)));
        note(
            "PlaySkillInfo",
            install_hook(
                "RaceMainViewController",
                "PlaySkillInfo",
                &[I32, BOOL, "Gallop.SkillDefine.SkillCategory"],
                VOID,
                play_skill_info as *const () as usize,
                &ORIG_INFO,
            )
            .map(drop),
        );
        // Without the filter, forcing these would show every NPC skill in Default.
        if filter_ok && ORIG_SKILL.load(Ordering::Acquire) != 0 {
            note(
                "IsShowSkillEnemy",
                install_hook("RaceEventPlayer", "IsShowSkillEnemy", &[I32, I32, BOOL], BOOL, is_show_skill_enemy as *const () as usize, &ORIG_ENEMY)
                    .map(drop),
            );
            let owner = il2cpp::class(GAME, "Gallop", "SkillManager").and_then(|k| il2cpp::field_of_type(k, "Gallop.IHorseRaceInfo"));
            let r = owner.and_then(|off| {
                let _ = OWNER_OFF.set(off);
                install_hook("SkillManager", "AddUsedSkillId", &[I32], VOID, add_used_skill_id as *const () as usize, &ORIG_USED).map(drop)
            });
            note("skipped plates (AddUsedSkillId)", r);
        }
        // Plate colour: each plate class's InitTextureSkill (virtual, so never inlined).
        if filter_ok {
            let r = TexFix::resolve().and_then(|t| {
                let _ = TEX_FIX.set(t);
                let mut hooked: Vec<usize> = Vec::new();
                let detours = [init_texture_skill_0 as *const () as usize, init_texture_skill_1 as *const () as usize];
                for (i, class) in ["MessagePlateNormalRace", "MessagePlateTeamStadium"].into_iter().enumerate() {
                    let m = il2cpp::class(GAME, "Gallop", class).and_then(|k| il2cpp::method_params(k, "InitTextureSkill", &[]))?;
                    il2cpp::expect(m, false, &[VOID], 0)?;
                    if hooked.contains(&m.code) {
                        continue;
                    }
                    let t = il2cpp::hook_unique(m, detours[i])?;
                    ORIG_TEXTURE[i].store(t, Ordering::Release);
                    hooked.push(m.code);
                }
                Ok(())
            });
            note("white plate colour (InitTextureSkill)", r);
        }
    }
    let _ = STATUS.set(status.trim_end().to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_decide_plates() {
        // Default: trainers' horses everything, NPCs gold (2) and unique (3+).
        assert!(show_plate(Mode::Default, true, 1));
        assert!(!show_plate(Mode::Default, false, 1));
        assert!(show_plate(Mode::Default, false, 2));
        assert!(show_plate(Mode::Default, false, 5));
        assert!(show_plate(Mode::Default, false, 0), "unknown rarity shows");
        for (owned, rarity) in [(true, 1), (false, 1), (false, 3)] {
            assert!(show_plate(Mode::All, owned, rarity));
            assert!(!show_plate(Mode::Off, owned, rarity));
        }
    }

    #[test]
    fn opponent_white_plates() {
        assert!(white_plate_for_opponent(1, true));
        assert!(!white_plate_for_opponent(2, true), "gold keeps the opponent plate");
        assert!(!white_plate_for_opponent(3, true), "unique keeps the opponent plate");
        assert!(!white_plate_for_opponent(1, false), "own plates untouched");
    }

    #[test]
    fn mode_from_settings() {
        assert_eq!(Mode::from_settings(None, false), Mode::Default);
        assert_eq!(Mode::from_settings(None, true), Mode::Off, "old hide_skill_popups = true");
        assert_eq!(Mode::from_settings(Some("all"), true), Mode::All, "new key wins");
        assert_eq!(Mode::from_settings(Some("off"), false), Mode::Off);
        assert_eq!(Mode::from_settings(Some("bogus"), false), Mode::Default);
        for m in Mode::ALL {
            assert_eq!(Mode::from_settings(Some(m.key()), false), m);
            assert_eq!(Mode::from_u8(m as u8), m);
        }
    }
}
