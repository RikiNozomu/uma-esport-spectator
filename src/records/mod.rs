//! Record cards: when the horses start loading into the gates (right after
//! the fanfare), broadcast-style cards show the course's TRACK RECORD and the
//! race's RACE RECORD for `seconds()` (menu slider): holder, time, date, and an
//! optional picture of the holder. Both side by side when the race has both,
//! one centred otherwise, none when it has neither.
//!
//! - `game` (main thread): hook on `RaceViewReplay::SetupGateIn` (the moment
//!   gate loading begins, ~23 s after `StateGateIn_Enter`); publishes the race
//!   on screen from clock's per-frame hook for the menu's editor.
//! - `ugui` (main thread): the cards as the game's own UI (a canvas of ours),
//!   updated every frame while showing; `layout` = their geometry (pure).
//! - here: the records file (`records/records.txt`, editable in the menu or by
//!   hand, reloaded when it changes), lookup, timing. Pure parts are tested.

pub mod game;
pub mod layout;
pub mod menu;
pub mod ugui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::{api, settings, tracer};

const KEY: &str = "record_cards";
const SECONDS_KEY: &str = "record_cards_seconds";
static ENABLED: AtomicBool = AtomicBool::new(true);
/// How long the cards stay, fades included (f32 bits).
static SECONDS: AtomicU32 = AtomicU32::new(0x40a0_0000); // 5.0

pub const DEFAULT_SECONDS: f32 = 5.0;
pub const MIN_SECONDS: f32 = 2.0;
pub const MAX_SECONDS: f32 = 15.0;
/// Fade in while rising `RISE` points into place; fade out while sinking `SINK`.
pub const FADE_IN: f32 = 0.35;
pub const FADE_OUT: f32 = 0.45;
pub const RISE: f32 = 24.0;
pub const SINK: f32 = 24.0;

pub fn seconds() -> f32 {
    f32::from_bits(SECONDS.load(Ordering::Relaxed))
}

pub fn set_seconds(s: f32) {
    let s = s.clamp(MIN_SECONDS, MAX_SECONDS);
    SECONDS.store(s.to_bits(), Ordering::Relaxed);
    settings::set_value(SECONDS_KEY, format!("{s:.1}"));
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    settings::set_value(KEY, on);
    if !on {
        hide();
    }
}

pub fn load_settings() {
    ENABLED.store(settings::get_bool(KEY, true), Ordering::Relaxed);
    let s = settings::get_f32(SECONDS_KEY, DEFAULT_SECONDS).clamp(MIN_SECONDS, MAX_SECONDS);
    SECONDS.store(s.to_bits(), Ordering::Relaxed);
}

/// `<plugin>\records`: records.txt and the holders' pictures. Outside the
/// game (tests, offline previews): `UMA_RECORDS_DIR`, else `./records`.
pub fn folder() -> PathBuf {
    if api::try_get().is_none() {
        return std::env::var_os("UMA_RECORDS_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("records"));
    }
    tracer::plugin_dir().join("records")
}

fn file() -> PathBuf {
    folder().join("records.txt")
}

// ---------------------------------------------------------------- data

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Turf,
    Dirt,
}

impl Ground {
    /// Master `race_course_set.ground`.
    pub fn from_master(v: i32) -> Option<Ground> {
        match v {
            1 => Some(Ground::Turf),
            2 => Some(Ground::Dirt),
            _ => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Ground::Turf => "turf",
            Ground::Dirt => "dirt",
        }
    }
}

/// A course + surface + distance: what a track record belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Course {
    pub id: i32,
    pub ground: Ground,
    pub distance: i32,
}

impl Course {
    /// "TOKYO TURF 2400"
    pub fn label(&self) -> String {
        let name = course_name(self.id).map(str::to_owned).unwrap_or_else(|| format!("COURSE {}", self.id));
        format!("{} {} {}", name.to_uppercase(), self.ground.word().to_uppercase(), self.distance)
    }
}

/// Racecourse names by master `race_track_id`.
pub const COURSES: [(i32, &str); 15] = [
    (10001, "Sapporo"),
    (10002, "Hakodate"),
    (10003, "Niigata"),
    (10004, "Fukushima"),
    (10005, "Nakayama"),
    (10006, "Tokyo"),
    (10007, "Chukyo"),
    (10008, "Kyoto"),
    (10009, "Hanshin"),
    (10010, "Kokura"),
    (10101, "Oi"),
    (10103, "Kawasaki"),
    (10104, "Funabashi"),
    (10105, "Morioka"),
    (10201, "Longchamp"),
];

pub fn course_name(id: i32) -> Option<&'static str> {
    COURSES.iter().find(|(c, _)| *c == id).map(|(_, n)| *n)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Track(Course),
    /// A race under every id it has (Career, Room Match, Champions Meeting...
    /// each have their own race id).
    Race { name: String, ids: Vec<i32> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub key: Key,
    pub holder: String,
    /// Shown as written, e.g. "2:20.6".
    pub time: String,
    /// When it was set, shown as written, e.g. "2018/11/25".
    pub date: String,
    /// The race it was set in, e.g. "Japan Cup" (shown after the date).
    pub event: String,
    /// A PNG in the records folder, or empty.
    pub picture: String,
}

impl Record {
    /// "TRACK RECORD : TOKYO TURF 2400" / "RACE RECORD : JAPANESE DERBY"
    pub fn heading(&self) -> String {
        match &self.key {
            Key::Track(c) => format!("TRACK RECORD : {}", c.label()),
            Key::Race { name, .. } => format!("RACE RECORD : {}", name.to_uppercase()),
        }
    }

    /// Short name for the menu.
    pub fn menu_name(&self) -> String {
        match &self.key {
            Key::Track(c) => format!("Track: {}", c.label()),
            Key::Race { name, .. } => format!("Race: {name}"),
        }
    }
}

/// The race on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaceNow {
    pub instance: i32,
    pub race: i32,
    pub course: Option<Course>,
}

/// (track record, race record) for a race.
pub fn find<'a>(records: &'a [Record], now: &RaceNow) -> (Option<&'a Record>, Option<&'a Record>) {
    let track = records.iter().find(|r| matches!(&r.key, Key::Track(c) if Some(*c) == now.course));
    let race = records.iter().find(|r| matches!(&r.key, Key::Race { ids, .. } if ids.contains(&now.race)));
    (track, race)
}

// ---------------------------------------------------------------- file

const HEADER: &str = "\
# Record cards (uma-esport-spectator): shown for 5 s when the horses start loading into the
# gates. Edit here or in the Hachimi menu (Record cards); this file reloads when it changes.
#
# [track <course id> <turf|dirt> <distance>]  ->  TRACK RECORD : TOKYO TURF 2400
# [race <name>] with ids = <race ids>          ->  RACE RECORD : <NAME>
#   A race has one id per mode (Career, Room Match, Champions Meeting, ...): list them all.
#   The menu shows the id of the race on screen; docs/races.csv in
#   github.com/RikiNozomu/uma-banner-gate-and-fence lists every race id.
# holder, time, date, event (the race it was set in): shown as written.
# picture: a PNG in this folder (optional; without one the card stretches).
#
# Course ids: 10001 Sapporo, 10002 Hakodate, 10003 Niigata, 10004 Fukushima, 10005 Nakayama,
# 10006 Tokyo, 10007 Chukyo, 10008 Kyoto, 10009 Hanshin, 10010 Kokura, 10101 Oi,
# 10103 Kawasaki, 10104 Funabashi, 10105 Morioka, 10201 Longchamp.
";

fn parse_section(head: &str) -> Option<Key> {
    let mut words = head.split_whitespace();
    match words.next()? {
        "track" => {
            let id = words.next()?.parse().ok()?;
            let ground = match words.next()? {
                "turf" => Ground::Turf,
                "dirt" => Ground::Dirt,
                _ => return None,
            };
            let distance = words.next()?.parse().ok()?;
            Some(Key::Track(Course { id, ground, distance }))
        }
        "race" => {
            let name = head.trim().strip_prefix("race")?.trim().to_owned();
            (!name.is_empty()).then_some(Key::Race { name, ids: Vec::new() })
        }
        _ => None,
    }
}

pub fn parse(text: &str) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    let mut current: Option<Record> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.extend(current.take());
            current = parse_section(head).map(|key| Record {
                key,
                holder: String::new(),
                time: String::new(),
                date: String::new(),
                event: String::new(),
                picture: String::new(),
            });
            continue;
        }
        let (Some(r), Some((k, v))) = (current.as_mut(), line.split_once('=')) else { continue };
        let v = v.trim().to_owned();
        match k.trim() {
            "holder" => r.holder = v,
            "time" => r.time = v,
            "date" => r.date = v,
            "event" => r.event = v,
            "picture" => r.picture = v,
            "ids" => {
                if let Key::Race { ids, .. } = &mut r.key {
                    *ids = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                }
            }
            _ => {}
        }
    }
    out.extend(current);
    out
}

pub fn serialize(records: &[Record]) -> String {
    let mut out = String::from(HEADER);
    for r in records {
        out.push('\n');
        match &r.key {
            Key::Track(c) => out.push_str(&format!("[track {} {} {}]\n", c.id, c.ground.word(), c.distance)),
            Key::Race { name, ids } => {
                out.push_str(&format!("[race {name}]\n"));
                let ids: Vec<String> = ids.iter().map(i32::to_string).collect();
                out.push_str(&format!("ids = {}\n", ids.join(", ")));
            }
        }
        out.push_str(&format!(
            "holder = {}\ntime = {}\ndate = {}\nevent = {}\npicture = {}\n",
            r.holder, r.time, r.date, r.event, r.picture
        ));
    }
    out
}

/// The records the plugin starts with: the Triple Crown and the Arima Kinen
/// (JRA records as of November 2026), their race ids from the game's master data.
/// Pictures named here are optional: without the file the card stretches.
pub fn defaults() -> Vec<Record> {
    let rec = |key: Key, holder: &str, time: &str, date: &str, event: &str, picture: &str| Record {
        key,
        holder: holder.to_owned(),
        time: time.to_owned(),
        date: date.to_owned(),
        event: event.to_owned(),
        picture: picture.to_owned(),
    };
    let track = |id, distance| Key::Track(Course { id, ground: Ground::Turf, distance });
    let race = |name: &str, ids: &[i32]| Key::Race { name: name.to_owned(), ids: ids.to_vec() };
    vec![
        // Satsuki Sho, Nakayama turf 2000: Robchen, 2026 Satsuki Sho (course record too).
        rec(track(10005, 2000), "Robchen", "1:56.5", "2026/04/19", "Satsuki Sho", ""),
        rec(race("Satsuki Sho", &[1005, 4904, 5112, 5975, 10164]), "Robchen", "1:56.5", "2026/04/19", "Satsuki Sho", ""),
        // Japanese Derby, Tokyo turf 2400: course record from the 2018 Japan Cup.
        rec(track(10006, 2400), "Almond Eye", "2:20.6", "2018/11/25", "Japan Cup", "almond_eye.png"),
        rec(
            race("Japanese Derby", &[1010, 4909, 5117, 5935, 5936, 5937, 7013, 10169]),
            "Do Deuce",
            "2:21.9",
            "2022/05/29",
            "Japanese Derby",
            "",
        ),
        // Kikuka Sho, Kyoto turf 3000.
        rec(track(10008, 3000), "Toho Jackal", "3:01.0", "2014/10/26", "Kikuka Sho", ""),
        rec(race("Kikuka Sho", &[1015, 1026, 4914, 5122, 5950, 7010, 10174]), "Toho Jackal", "3:01.0", "2014/10/26", "Kikuka Sho", ""),
        // Arima Kinen, Nakayama turf 2500.
        rec(track(10005, 2500), "Zenno Rob Roy", "2:29.5", "2004/12/26", "Arima Kinen", "zenno_rob_roy.png"),
        rec(
            race("Arima Kinen", &[1023, 4922, 5130, 5955, 7019, 7023, 10182, 12004]),
            "Zenno Rob Roy",
            "2:29.5",
            "2004/12/26",
            "Arima Kinen",
            "zenno_rob_roy.png",
        ),
    ]
}

// ---------------------------------------------------------------- store

struct Store {
    records: Vec<Record>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
}

static STORE: Mutex<Store> = Mutex::new(Store { records: Vec::new(), modified: None, checked: None });
const RECHECK: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(file()).and_then(|m| m.modified()).ok()
}

/// The records, reloaded when records.txt changed (checked at most once a
/// second). The file is created with the defaults the first time.
pub fn records() -> Vec<Record> {
    let mut s = lock(&STORE);
    if s.checked.is_some_and(|t| t.elapsed() < RECHECK) {
        return s.records.clone();
    }
    s.checked = Some(Instant::now());
    let m = modified();
    if m.is_none() {
        let defaults = defaults();
        write_file(&defaults);
        s.records = defaults;
        s.modified = modified();
    } else if m != s.modified {
        s.records = parse(&std::fs::read_to_string(file()).unwrap_or_default());
        s.modified = m;
    }
    s.records.clone()
}

/// Replaces all records and saves the file.
pub fn save(records: Vec<Record>) {
    write_file(&records);
    let mut s = lock(&STORE);
    s.records = records;
    s.modified = modified();
    s.checked = Some(Instant::now());
}

fn write_file(records: &[Record]) {
    let path = file();
    let result = std::fs::create_dir_all(folder()).and_then(|_| std::fs::write(&path, serialize(records)));
    if let Err(e) = result {
        api::get().warn(&format!("record cards: cannot write {}: {e}", path.display()));
    }
}

/// PNG files in the records folder, sorted.
pub fn pictures() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(folder()) else { return Vec::new() };
    let mut v: Vec<String> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_lowercase().ends_with(".png"))
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------- showing

/// One card as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub heading: String,
    pub holder: String,
    pub time: String,
    /// Date and event: "2018/11/25  JAPAN CUP".
    pub date: String,
    /// File name in the records folder (or a full path).
    pub picture: Option<String>,
}

/// The bottom strip: date, then the race it was set in.
pub fn date_line(date: &str, event: &str) -> String {
    let (date, event) = (date.trim(), event.trim().to_uppercase());
    match (date.is_empty(), event.is_empty()) {
        (false, false) => format!("{date}  {event}"),
        (false, true) => date.to_owned(),
        (true, _) => event,
    }
}

impl Card {
    pub fn from_record(r: &Record) -> Card {
        let picture = (!r.picture.trim().is_empty()).then(|| r.picture.trim().to_owned());
        Card {
            heading: r.heading(),
            holder: r.holder.clone(),
            time: r.time.clone(),
            date: date_line(&r.date, &r.event),
            picture,
        }
    }
}

/// The cards for a race: track record first, race record second.
pub fn cards_for(records: &[Record], now: &RaceNow) -> Vec<Card> {
    let (track, race) = find(records, now);
    track.into_iter().chain(race).map(Card::from_record).collect()
}

struct Showing {
    cards: Vec<Card>,
    started: Instant,
    /// `seconds()` when it started.
    duration: f32,
    serial: u32,
}

static SHOWING: Mutex<Option<Showing>> = Mutex::new(None);
static SERIAL: AtomicU32 = AtomicU32::new(0);

/// Starts showing `cards` for `seconds()` (nothing when empty).
pub fn show(cards: Vec<Card>) {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    *lock(&SHOWING) =
        (!cards.is_empty()).then(|| Showing { cards, started: Instant::now(), duration: seconds(), serial });
}

/// Whether cards are (or should be) on screen. Any thread.
pub fn is_showing() -> bool {
    lock(&SHOWING).is_some()
}

pub fn hide() {
    *lock(&SHOWING) = None;
}

/// The menu's "Preview": the cards of the race on screen (or the last one),
/// else the Japanese Derby's. Any thread (no game calls).
pub fn preview() {
    let records = records();
    let mut cards = game::current().map(|now| cards_for(&records, &now)).unwrap_or_default();
    if cards.is_empty() {
        let derby = RaceNow {
            instance: 0,
            race: 10169,
            course: Some(Course { id: 10006, ground: Ground::Turf, distance: 2400 }),
        };
        cards = cards_for(&records, &derby);
    }
    show(cards);
}

/// Shows one record's card (the editor's preview).
pub fn preview_record(r: &Record) {
    show(vec![Card::from_record(r)]);
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(3)
}

/// (opacity, vertical offset in points, down = positive) at `t` seconds into
/// a show of `duration` seconds; None once it is over. Fades in rising
/// `RISE` points into place, holds, fades out sinking `SINK` points.
pub fn motion(t: f32, duration: f32) -> Option<(f32, f32)> {
    if !(0.0..duration).contains(&t) {
        return None;
    }
    let fade_in = (t / FADE_IN).min(1.0);
    let fade_out = ((duration - t) / FADE_OUT).min(1.0);
    let alpha = fade_in.min(fade_out).clamp(0.0, 1.0);
    let offset = if fade_in < 1.0 {
        RISE * (1.0 - ease_out(fade_in))
    } else {
        SINK * ease_in(1.0 - fade_out)
    };
    Some((alpha, offset))
}

pub struct Frame {
    pub cards: Vec<Card>,
    pub alpha: f32,
    /// Vertical offset (points, down = positive): the rise in, the sink out.
    pub offset: f32,
    /// Changes with every new show (the cards to lay out again).
    pub serial: u32,
}

/// The cards' state now (`ugui`, every frame while showing).
pub fn next_frame() -> Option<Frame> {
    let mut s = lock(&SHOWING);
    let showing = s.as_ref()?;
    match motion(showing.started.elapsed().as_secs_f32(), showing.duration) {
        Some((alpha, offset)) => Some(Frame { cards: showing.cards.clone(), alpha, offset, serial: showing.serial }),
        None => {
            *s = None;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokyo() -> RaceNow {
        RaceNow { instance: 800019, race: 10169, course: Some(Course { id: 10006, ground: Ground::Turf, distance: 2400 }) }
    }

    #[test]
    fn file_round_trip() {
        let records = defaults();
        let text = serialize(&records);
        assert_eq!(parse(&text), records);
        // Hand-written: comments, spacing, unknown keys and broken sections.
        let r = parse("# x\n[race  Japan Cup ]\nids = 1, 2 ,x\nholder=A\nfoo = 1\n[track 1 grass 2]\nholder = B\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, Key::Race { name: "Japan Cup".into(), ids: vec![1, 2] });
        assert_eq!(r[0].holder, "A");
    }

    #[test]
    fn finds_both_records_of_the_derby() {
        let records = defaults();
        let (track, race) = find(&records, &tokyo());
        assert_eq!(track.unwrap().holder, "Almond Eye");
        assert_eq!(race.unwrap().holder, "Do Deuce");
        let cards = cards_for(&records, &tokyo());
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].heading, "TRACK RECORD : TOKYO TURF 2400");
        assert_eq!(cards[1].heading, "RACE RECORD : JAPANESE DERBY");
        assert_eq!(cards[1].date, "2022/05/29  JAPANESE DERBY");
        assert_eq!(cards[0].date, "2018/11/25  JAPAN CUP");
    }

    #[test]
    fn one_or_no_card() {
        let records = defaults();
        // Another Tokyo 2400 race: the track record only.
        let jc = RaceNow { race: 10178, ..tokyo() };
        assert_eq!(cards_for(&records, &jc).len(), 1);
        // Tokyo 1600: nothing.
        let mile = RaceNow { course: Some(Course { id: 10006, ground: Ground::Turf, distance: 1600 }), ..jc.clone() };
        assert!(cards_for(&records, &mile).is_empty());
        // Unknown course (not read): the race record still shows.
        let derby = RaceNow { course: None, ..tokyo() };
        assert_eq!(cards_for(&records, &derby).len(), 1);
    }

    #[test]
    fn every_default_race_is_on_its_course() {
        // Each race record has a track record for the same course in the defaults.
        let records = defaults();
        let tracks = records.iter().filter(|r| matches!(r.key, Key::Track(_))).count();
        let races = records.iter().filter(|r| matches!(r.key, Key::Race { .. })).count();
        assert_eq!((tracks, races), (4, 4));
    }

    #[test]
    fn fades_in_rising_and_out_sinking() {
        let d = 5.0;
        assert_eq!(motion(-0.1, d), None);
        assert_eq!(motion(d, d), None);
        // Start: invisible, below its place.
        assert_eq!(motion(0.0, d), Some((0.0, RISE)));
        // Rising: offset shrinks while it fades in.
        let (a1, o1) = motion(0.1, d).unwrap();
        let (a2, o2) = motion(0.2, d).unwrap();
        assert!(a1 < a2 && o1 > o2 && o2 > 0.0);
        // Held in place.
        assert_eq!(motion(2.5, d), Some((1.0, 0.0)));
        // Leaving: fading out while sinking.
        let (a3, o3) = motion(d - 0.3, d).unwrap();
        let (a4, o4) = motion(d - 0.05, d).unwrap();
        assert!(a3 > a4 && o4 > o3 && o3 >= 0.0 && o4 <= SINK);
        // The duration is the show's own.
        assert_eq!(motion(9.0, 10.0), Some((1.0, 0.0)));
        assert_eq!(motion(9.0, 5.0), None);
    }

    #[test]
    fn date_lines() {
        assert_eq!(date_line("2018/11/25", "Japan Cup"), "2018/11/25  JAPAN CUP");
        assert_eq!(date_line("2018/11/25", " "), "2018/11/25");
        assert_eq!(date_line("", "Japan Cup"), "JAPAN CUP");
        assert_eq!(date_line("", ""), "");
    }

    #[test]
    fn labels() {
        let c = Course { id: 10201, ground: Ground::Turf, distance: 2400 };
        assert_eq!(c.label(), "LONGCHAMP TURF 2400");
        let c = Course { id: 99999, ground: Ground::Dirt, distance: 1200 };
        assert_eq!(c.label(), "COURSE 99999 DIRT 1200");
    }
}
