# uma-esport-spectator

Hachimi Edge plugin (Windows, API v3) for studying how the game runs a race.

**Stage 1: function tracer.** Hooks every method of the classes listed in
`trace_targets.txt` and logs which ones run while recording is on.

## Build

Needs Rust with the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`).

```powershell
cargo build --release
# -> target\release\uma_esport_spectator.dll
```

## Install

1. Copy `uma_esport_spectator.dll` into the game's `hachimi` folder.
2. Add it to `hachimi\config.json`:
   ```json
   "load_libraries": ["hachimi\\uma_esport_spectator.dll"]
   ```
3. Restart the game. `hachimi.log` should show `[uma-spectator] plugin loaded`.

## Use

Open the Hachimi menu. The plugin's section has:

- **Disable skill cut-ins (all horses)**: skips the unique-skill cut-in scene for every
  horse, the player's included. Takes effect on the next skill; saved in `settings.txt`.
  Works by making `Gallop.RaceEventPlayer::IsPlayableCutIn` return false
  (installed once at load). The tracer can't hook that method while this is installed.
- **Record function calls**: on = start a new log file, off = finish it and write the summary.
  The first time it is turned on, hooks are installed; the game freezes for a few seconds.
- **Log every call (heavy)**: off (default) logs only the first call of each method,
  which gives the order things happen in. On logs every call; use it for short windows only.

Files, all under `hachimi\uma-esport-spectator\`:

| File | What |
|---|---|
| `trace_targets.txt` | Which classes/methods to hook. Created with defaults on first run. Edit, then restart the game. |
| `hooked_methods.txt` | Every method the plugin tried to hook, `ok` or `FAILED`, with its signature (return type, parameter types and names). |
| `settings.txt` | Plugin settings (`disable_skill_cutins = true/false`). |
| `logs\trace_YYYYMMDD_HHMMSS.log` | One file per recording: `ms  thread  Class::Method(argc)`, then a call-count summary. |

## How it works

Methods are hooked without knowing their signatures. Each hook is a small
generated x86-64 stub (`src/stub.rs`) that saves the argument registers, calls
the logger, restores the registers, and jumps to the original code. The stub
is off the stack before the original runs, so arguments, return values and
exceptions are untouched. `cargo test` checks this on Linux x86-64.

Hooks stay installed once installed; turning recording off only stops logging.

## Limits

- `FAILED` in `hooked_methods.txt` usually means the method body is too short
  to patch, or Hachimi already hooks it.
- Methods that share code after linker folding (empty bodies, trivial getters)
  are skipped, since a hook could not tell them apart.
- Generic classes are skipped.
- If the game exits while recording, only first-call lines are guaranteed on
  disk. Turn recording off before closing.
