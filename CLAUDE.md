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
- `src/cutin.rs`: "disable skill cut-ins" hook on `Gallop.RaceEventPlayer::IsPlayableCutIn`
  (stub jumps to a return-false fn while enabled). Installed at game ready.
- Hachimi on Windows inits plugins *after* firing game-initialized callbacks, so
  `lib.rs` also checks readiness directly at load.
- `src/gui.rs`: menu section (checkboxes, sliders, status).
- `src/settings.rs`: shared `settings.txt` (key = value).
- `src/race.rs`: race snapshot on the main thread (positions, gate, chara/card id, trainer
  name); writes `race_classes.txt`. Return types are checked before calling getters.
- `src/leaderboard/`: overlay. `layout.rs` (placement, JRA bracket colours) and `anim.rs`
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
