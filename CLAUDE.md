# uma-esport-spectator

Hachimi Edge plugin (Rust cdylib, Windows x86-64, plugin API v3) for Umamusume.
Goal: study the race flow, later build esport-spectator features on top.

## Layout
- `src/lib.rs`: `hachimi_init_v3` entry, registers game-init callback and menu section.
- `src/api.rs`: hand-written bindings to the Hachimi API, resolved by name via `get_api`.
  Source of truth for signatures: `src/core/plugin_api.rs` in https://github.com/kairusds/Hachimi-Edge
  (`hachimi_get_api` has the name table). Add new functions to the `api_struct!` list;
  init fails if any listed name is missing in the host. Optional newer functions go in
  `Extra` (`api::extra()`), resolved one by one, None when the host lacks them.
- `src/stub.rs`: generic x86-64 entry stubs (Win64 ABI). Changing the asm: keep `cargo test` green.
- `src/tracer.rs`: method enumeration, hook install (on the main thread via
  `il2cpp_schedule_on_thread`), recording sessions, log files.
- `src/targets.rs`: `trace_targets.txt` parsing and glob matching.
- `src/sig.rs`: method signatures from IL2CPP metadata (main thread / init only).
- `src/settings.rs`: one `settings.txt` for everything: menu toggles (`TOGGLES`, `is_on`/`set`)
  and free key/values (`get_bool`/`get_f32`/`set_value`, used by the leaderboard). Every save
  writes all keys, so features never drop each other's settings.
- `src/patches.rs`: toggles that neutralize game methods (`PATCHES` table: class,
  method, argc, `ReturnFalse` for bool / `Skip` for void; return type is checked before
  hooking). The stub jumps to `ret_false`/`ret_void` while the toggle is on. Installed at game ready.
  Add a patch = add a `Toggle` (if new) + a `Patch` row + a `TRAMPOLINES` slot.
- `src/skill_popups.rs`: skill-plate mode (`skill_popups` = default/all/off) + trainer-name
  toggle. Replay path (trace): `RaceEventPlayer::IsShowSkillEnemy(teamId, horseIndex, isMob)` gates
  non-viewer horses (forced true in default/all) -> `RaceManagerReplayBase::ShowMessageUISkill(horse,
  skillId, isEnemy)` (decided per plate: trainer from HorseData, rarity from
  `MasterDataManager.masterSkillData.Get(id).Rarity`; trainer-name toggle = pass isEnemy=true, the
  game's enemy plate carries the trainer name) and `RaceMainViewController::PlaySkillInfo` (skipped
  when off). Other runners' white skills never reach ShowMessageUISkill: every activation goes
  through `SkillManager::AddUsedSkillId` (owner = its IHorseRaceInfo field); those not shown by the
  game within `WAIT_FRAMES` are shown by `tick` (clock's per-frame hook) through the same hook,
  except uniques (cut-in path) and while `IsCutInPlayingOrReserved`. Plate colour: enemy + white
  (rarity 1) = pastel rainbow; the virtual `MessagePlateNormalRace/TeamStadium::InitTextureSkill`
  temporarily sets `_playDesc.Type` to the normal type (`TexFix`). `PlayDesc` is a CLASS (object
  offsets, `ref PlayDesc` = pointer to a reference). Inlined, hooks never run: `GetMessagePlateType`,
  `RaceUIMessagePlate::RequestShowMessagePlate`, `RaceMainViewController::ShowMessagePlate`,
  `MessagePlateCtrl::PlayIn`. `race_classes.txt` dumps the plate classes. "Owned" = the
  horse's `HorseData.get_TrainerName` is non-empty. `RaceManagerReplayBase::AddSkillEvent` is NOT
  the replay path: it is folded code (an empty body shared with other methods); hooking it crashed
  the game at the title screen. New hooks use `il2cpp::hook_unique`, which refuses shared code.
- `src/il2cpp.rs`: lookup/hook/field helpers. `method()` walks parent classes quietly;
  `expect()` checks static/return/param count before calling with a hand-written signature.
  `describe_class()` feeds `race_classes.txt` (real field/method names: check it before guessing).
- `src/label.rs`: in-game Unity text (clones `UI/Parts/Notification` like Hachimi's captions,
  reparents its `Gallop.TextCommon` to the notice canvas). Main thread only.
- `src/clock.rs`: race clock (ends at the result board: `StateOverRunResult_Enter` calls
  `clock::result_board_shown`). Owns the per-frame hook on `RaceManagerReplayBase::AlterUpdate`
  (`this` = race manager) and the race-end hooks; these also drive `race_ui`. Pure
  timing/format logic is unit-tested in the same file.
- `src/race_ui.rs`: "hide race UI". Layer 1: hooks `void X(bool)` switches (`TARGETS`:
  `RaceMainViewController::SetVisible*`, `RaceViewReplay::VisibleMarkers`), records what
  the game wants per object, passes `false` from StateGateIn_Enter (fanfare) to
  StateWinningCircleInit_Enter; taps allowed from StateOverRunResult_Enter. Only ever calls switches the game turned on.
  Layer 2: CanvasGroup alpha 0 on `RaceMainView` (re-applied each frame while hiding).
  Race boundaries: `StateGateIn_Enter` = race start (`race_gen()` bumps; clock resets on it),
  race end hooks invalidate everything. Never call into a game object recorded in an
  earlier race: the race manager can be reused across races, its views are not
  (calling a destroyed view crashed the game inside IL2CPP; `STAMP`/`RACE_SEQ` guard this).
- Clock layout: two labels (time, distance) in the `TOP_RIGHT` corner, aligned to it; at the top right the running seconds sit `TENTHS_GAP` inwards (left alignment
  keeps the seconds in place as tenths appear). Tune `MARGIN_X`, `DISTANCE_LAYOUT.margin_y`. Rich-text
  transparent placeholders don't work: TextCommon's outline still draws them.
- Hachimi on Windows inits plugins *after* firing game-initialized callbacks, so
  `lib.rs` also checks readiness directly at load.
- `src/seek/`: seek bar (toggle `SEEK_BAR`), our replacement for Hachimi's `race_playback_slider`,
  which never shows on Global (Hachimi's scene id comes from a `SceneManager::AlterUpdate` hook the
  Global client doesn't run in races). `game.rs` (main thread, from clock's per-frame hook, after the
  clock tick) publishes the replay time and applies commands: pause on drag start, seek (Hachimi's
  `seek_sync` sequence, no SEH guard here: only checked, non-null objects), resume on release;
  seeks wait out cut-ins, a released drag never waits. `input.rs` (Windows) subclasses the game
  window (Hachimi chains it behind its own WndProc) and swallows only messages on the bar / during
  a drag. Drawn with its own egui context + renderer in `leaderboard/render_d3d11.rs`. Shows while
  the mouse moves (`SHOW_FOR`), hovered, dragging or paused; from the gates opening to the end of
  the timeline (result board), finish included. Playback speed (`SPEEDS` 0.25-4x, « label »; label
  click = 1x): hook on `RaceManagerReplayBase::SetDeltaTimeScale` multiplies the game's own scale by
  ours; back to 1x on a new race and at the result board. Music is not resynced after a seek nor
  pitched with speed (Hachimi's `resync_race_music` is the reference). `examples/seek_preview.rs`
  renders its states offline.
- `src/track_map/`: course map top-left (toggle `track_map`, default on), broadcast style: course
  sampled once per race with `RaceManager::GetLaneTransform(distance, out pos, out rot)` (0..course
  length, only while `clock::race_live()`: the call needs the course set up), runners from
  `HorseRaceInfo._position`/`_distance` every frame (`game.rs`, clock's per-frame hook). `mod.rs`
  projection (long axis horizontal, goal at the front, tilt `TILT`, skew `SKEW`) is pure and tested;
  `draw.rs` paints a tilted band (turf/dirt from `RaceInfo.get_GroundType`), goal tick, bracket-colour
  dots (gate by `race::Runner::index`), leader as the red marker, "TURF 2,000m", leader's metres left
  and speed (distance over race time). Own egui context + renderer in render_d3d11.rs. Size/position
  (`View`, menu sliders, `track_map_scale`/`_x`/`_y`): laid out at the top-left in 1080p points, size =
  pixels per point x scale, position = layer transform from `placement` (0..1 of the free space).
  `examples/map_preview.rs` renders it offline. Sprints only draw their part of the oval.
- `src/records/`: record cards (toggle `record_cards`): TRACK RECORD (course + ground + distance) and
  RACE RECORD (a race's ids in every mode: Career, Room Match, ...) cards for `seconds()` (slider
  `record_cards_seconds`, 2-15, default 5; `motion()`: fade in rising `RISE`, fade out sinking `SINK`) when
  gate loading begins = hook on `RaceViewReplay::SetupGateIn` (trace: StateGateIn_Enter 1.5 s ->
  SetupGateIn 24.2 s -> EndGateInDemo 30.2 s -> StateWaitStartDash 51.0 s -> StateRace 51.07 s).
  Race on screen: `RaceManager.get_RaceInfo` ids + `get_RaceCourseSet` (master row; fields found by
  name candidates, `RaceTrackId`/`Distance`/`Ground`, members logged if missing), published from
  clock's per-frame hook (`game::current`) for the menu. `records/records.txt` (`[track id turf 2400]`
  / `[race Name]` + `ids`, holder/time/date/event/picture; created with `defaults()` = Triple Crown +
  Arima), pictures in the same folder. Drawn as game UI by `ugui.rs` (main thread): our own overlay
  `Canvas` (sortingOrder 900, `CanvasScaler` 1920x1080 matched on height = 1080p points,
  `CanvasGroup` for the fade, never blocks clicks, DontDestroyOnLoad) with a bottom gradient
  (RawImage, PNG made with the image crate) and two card slots (Image strips, RawImage picture,
  Text in the game font from the `UI/Parts/Notification` prefab's TextCommon, best fit). `tick` is
  scheduled on the main thread from the present callback (`request_tick`) while cards show, so it
  runs in and out of races (menu preview); lays out on a new show / screen size, then only fades and
  moves. `layout.rs` (pure, tested): bottom-left origin, yellow title strip, white holder + time,
  yellow date strip (date + event), picture overlapping the card's right edge, no picture = the card
  stretches over the group; groups centred in their screen slice, `fit_scale` for 4:3. Cards hold
  the picture's file name; `folder()` falls back to `UMA_RECORDS_DIR` / `./records` without the
  Hachimi API (tests). `menu.rs` editor uses `api::extra()` (button, text field: optional, newer
  Hachimi only); Hachimi keys a text field by its buffer's address and keeps its own copy, so each
  load allocates new buffers.
- `src/gui.rs`: menu section (checkboxes, sliders, status).
- `src/race.rs`: leaderboard race snapshot on the main thread (positions, gate, chara/card/mob
  id, trainer name), max 18 runners. Return types are checked before calling getters.
  `race_classes.txt` (written by `clock`) also lists `HorseData`/`RaceHorseManagerBase` for it.
- `src/leaderboard/`: overlay, shown from `SHOW_AFTER` (10 s of `clock::race_time()`) until
  the leader is within `leaderboard_hide_before_goal` m of the goal (`clock::remaining_distance()`,
  default 200, 0 = never) or `clock::race_live()` ends (result board / race-end hooks); each appearance plays an
  entrance (fade + rise, `ENTRANCE_*`; a frame gap > `REAPPEAR_GAP` restarts it); `next_frame`
  keeps the last order for `EXIT_TIME` after it should hide (fade + sink). One LEADER tag owned
  by `Board` glides to the leader (`TAG_RATE`); cards have no tag of their own. `race_live` is published by clock's per-frame tick even with the clock
  off). Order comes from `race::rank`: finished runners (IsFinished or distance >= course) by
  official `HorseRaceInfoReplay.get_FinishOrder`, frozen; the rest by live `CurOrder`.
  The finish order is precomputed for the whole race: never use it before the line. `layout.rs` (placement, JRA bracket colours) and `anim.rs`
  (per-runner card animation) are pure and unit-tested; `mod.rs` paints with egui;
  `render_d3d11.rs` (Windows) draws from Hachimi's present callback with egui-directx11.
  Never keep a back-buffer reference across frames; save/restore D3D11 state.
- `tools/extract_icons.py`: exports chr_icon textures from the game's own files (meta =
  SQLite3MultipleCiphers ChaCha20; bundles XOR'd after 256 bytes; see UmamusumeExplorer)
  into `chara/<card id>.png`, `chara/<chara id>.png` and `chara/mob_<mob id>.png` (NPCs,
  outfit from master `mob_data.dress_id`).
- `examples/render_preview.rs` + `tools/rasterize.py`: render overlay frames to PNG/GIF
  offline to check the design (`UMA_FONT_DIR`/`UMA_FONTS`/`UMA_BOLD_FONT` for fonts off Windows).

## Rules
- Dependencies: egui, image (png), and on Windows egui-directx11 + windows, at the
  versions Hachimi Edge uses. Small OS calls are declared by hand in `src/sys.rs`.
- Host callbacks that run plugin logic are wrapped in `catch_unwind` (release uses
  `panic = "unwind"`); a panic must never reach Hachimi.
- The hook hot path (`tracer::enter`) runs on game threads: no il2cpp calls, no panics.
- Game classes live in `umamusume.dll`, namespace `Gallop`. Hachimi's own hooks in
  `src/il2cpp/hook/umamusume/` of the Hachimi-Edge repo are a good map of useful classes.

## Commands
- `cargo test` (works on Linux or Windows x86-64)
- Type-check the Windows build from Linux (needs Rust std source in the toolchain):
  `RUSTC_BOOTSTRAP=1 cargo check --release -Zbuild-std=std,panic_unwind --target x86_64-pc-windows-msvc`
- `cargo build --release` -> `target/release/uma_esport_spectator.dll`
