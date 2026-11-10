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
- `src/patches.rs`: menu toggles that neutralize game methods (`PATCHES` table: class,
  method, argc, `ReturnFalse` for bool / `Skip` for void; return type is checked before
  hooking). The stub jumps to `ret_false`/`ret_void` while the toggle is on. Installed at game ready.
  Add a patch = add a `Toggle` (if new) + a `Patch` row + a `TRAMPOLINES` slot.
- Hachimi on Windows inits plugins *after* firing game-initialized callbacks, so
  `lib.rs` also checks readiness directly at load.
- `src/gui.rs`: menu section (checkboxes + status).

## Rules
- No crate dependencies so far; OS calls are declared by hand in `src/sys.rs`.
- The hook hot path (`tracer::enter`) runs on game threads: no il2cpp calls, no panics.
- Game classes live in `umamusume.dll`, namespace `Gallop`. Hachimi's own hooks in
  `src/il2cpp/hook/umamusume/` of the Hachimi-Edge repo are a good map of useful classes.

## Commands
- `cargo test` (works on Linux or Windows x86-64)
- `cargo build --release` -> `target/release/uma_esport_spectator.dll`
