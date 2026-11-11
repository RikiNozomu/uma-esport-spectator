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

**Leaderboard overlay**

- **Show during races**: a live leaderboard along the bottom of the screen, right-aligned,
  leader on the right. Each card shows the character image, the gate number in its
  bracket (waku) colour, and the owner's trainer name. The top 3 cards are bigger.
  Cards follow their runner: when positions swap, the cards slide past each other and
  resize; a runner that gains a place gets a short scale-up "pop".
- **Preview with sample data**: shows 18 sample runners that swap places every second,
  to check size and position outside a race.
- **Size** / **Height**: card size, and where the bottom of the row sits (fraction of the
  screen height). Saved in `settings.txt`.

Character images are PNG files you provide, in `hachimi\uma-esport-spectator\chara\`:
`<card id>.png` (one outfit, e.g. `100101.png`) is used first, then `<chara id>.png`
(any outfit, e.g. `1001.png`). Square images look best. Without an image the card shows a
plain silhouette. Images are read once per game session.

Japanese/Thai/Korean/Chinese names use Windows' own fonts (Yu Gothic, Leelawadee UI, ...).

**Race**

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
| `settings.txt` | Plugin settings (cut-ins, leaderboard on/off, size, height). |
| `chara\*.png` | Character images for the leaderboard (see above). |
| `race_classes.txt` | Methods and fields of the race classes, written at the first leaderboard update. Useful when the game changes and a value goes missing. |
| `logs\trace_YYYYMMDD_HHMMSS.log` | One file per recording: `ms  thread  Class::Method(argc)`, then a call-count summary. |

## How it works

Methods are hooked without knowing their signatures. Each hook is a small
generated x86-64 stub (`src/stub.rs`) that saves the argument registers, calls
the logger, restores the registers, and jumps to the original code. The stub
is off the stack before the original runs, so arguments, return values and
exceptions are untouched. `cargo test` checks this on Linux x86-64.

Hooks stay installed once installed; turning recording off only stops logging.

## Leaderboard internals

- Data: a few times per second the plugin asks the game's main thread (via
  `il2cpp_schedule_on_thread`) for each runner's current position, gate, character id,
  card id, name and trainer name (`src/race.rs`). Getter return types are checked
  against the game's metadata before calling them; anything missing is logged once.
- Drawing: the plugin has its own egui context and draws with `egui-directx11` from
  Hachimi's present callback (`src/leaderboard/render_d3d11.rs`), so it does not block
  game input and stays under Hachimi's own menu. Pipeline state is saved and restored,
  and no back-buffer reference is kept between frames (window resizes stay safe).
- On any error the overlay turns itself off and logs why; toggling it on retries.

Design preview without the game (Linux or Windows):

```sh
cargo run --example render_preview -- out [image dir]
python3 tools/rasterize.py out     # out/frame*.png and out/swap.gif
```

## Limits

- `FAILED` in `hooked_methods.txt` usually means the method body is too short
  to patch, or Hachimi already hooks it.
- Methods that share code after linker folding (empty bodies, trivial getters)
  are skipped, since a hook could not tell them apart.
- Generic classes are skipped.
- If the game exits while recording, only first-call lines are guaranteed on
  disk. Turn recording off before closing.
