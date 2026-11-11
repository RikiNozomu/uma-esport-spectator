# uma-esport-spectator

Hachimi Edge plugin (Rust cdylib, Windows x86-64, plugin API v3) for Umamusume.
Goal: study the race flow, later build esport-spectator features on top.

## Layout
- `src/lib.rs`: `hachimi_init_v3` entry, registers game-init callback and menu section.
- `src/api.rs`: hand-written bindings to the Hachimi API, resolved by name via `get_api`.
  Source of truth for signatures: `src/core/plugin_api.rs` in https://github.com/kairusds/Hachimi-Edge
  (`hachimi_get_api` has the name table). Add new functions to the `api_struct!` list;
  init fails if any listed name is missing in the host.
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
- `src/gui.rs`: menu section (checkboxes, sliders, status).
- `src/race.rs`: leaderboard race snapshot on the main thread (positions, gate, chara/card/mob
  id, trainer name), max 18 runners. Return types are checked before calling getters.
  `race_classes.txt` (written by `clock`) also lists `HorseData`/`RaceHorseManagerBase` for it.
- `src/leaderboard/`: overlay, shown from `SHOW_AFTER` (10 s of `clock::race_time()`) until
  `clock::race_live()` ends (result board / race-end hooks); each appearance plays an
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
