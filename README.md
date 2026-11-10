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
  horse, the player's included. Makes `Gallop.RaceEventPlayer::IsPlayableCutIn` return false.
- **Hide skill popups (right side)**: the skill plates shown when a skill activates are
  not shown. Skips `Gallop.RaceMainViewController::PlaySkillInfo` and
  `Gallop.RaceManagerReplayBase::ShowMessageUISkill`.

  Both toggles apply on the next skill and are saved in `settings.txt`. Their hooks
  are installed once at load (see `src/patches.rs`), so the tracer shows these
  methods as `FAILED`. The line under each checkbox says whether each hook is `ok`.
- **Race clock** (top right; `TOP_RIGHT` in clock.rs), on by default: a broadcast-style clock drawn by the game's
  own UI (works in exclusive fullscreen and OBS game capture). White running seconds;
  when the leader passes 1000 m (half the course for races up to 1400 m) the split
  shows in yellow with tenths and the distance for 5 s; when the leader finishes,
  the winner's official time (`1.56.1`) is held until the winning-circle scene.
  See `src/clock.rs` (timing) and `src/label.rs` (the Unity label).
- **Hide race UI while running**: from the fanfare (gate-in) through the scoreboard until the winning circle, the race
  HUD is hidden: skip / menu / fast-forward / photo buttons, the rank / character icon,
  the progress bar, the 3D marker over the player's horse, commentary, message plates,
  skill info. Two layers: the game's own `SetVisible*(bool)` switches
  (`RaceMainViewController`, `RaceViewReplay::VisibleMarkers`), and a CanvasGroup that
  fades the whole race HUD view for parts that are shown by default. Everything is
  restored at the winning circle or when the toggle is turned off (taps work again from
  the scoreboard on, in case it waits for one). The menu button
  is hidden too; Esc still opens the pause menu.
- **Record function calls**: on = start a new log file, off = finish it and write the summary.
  The first time it is turned on, hooks are installed; the game freezes for a few seconds.
- **Log every call (heavy)**: off (default) logs only the first call of each method,
  which gives the order things happen in. On logs every call; use it for short windows only.

Files, all under `hachimi\uma-esport-spectator\`:

| File | What |
|---|---|
| `trace_targets.txt` | Which classes/methods to hook. Created with defaults on first run. Edit, then restart the game. |
| `hooked_methods.txt` | Every method the plugin tried to hook, `ok` or `FAILED`, with its signature (return type, parameter types and names). |
| `settings.txt` | Plugin settings (`disable_skill_cutins`, `hide_skill_popups`, `race_clock`, `hide_race_ui` = `true`/`false`). |
| `race_classes.txt` | Fields (with offsets) and methods (with signatures) of the main race classes, written at load. |
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
