# uma-esport-spectator

Hachimi Edge plugin for Umamusume: Pretty Derby (Steam, Windows) for watching and studying races.

- **Leaderboard overlay**: a live, right-aligned row of runner cards during races (character
  icon, gate number in its bracket colour, owner's trainer name), with animated position swaps.
- **Hide race UI while running**: hide the game's race HUD (buttons, icons, marker,
  commentary) from the fanfare to the winning circle, for a clean broadcast view.
- **Race clock**: broadcast-style running time, 1000 m split, and the winner's time.
- **Disable skill cut-ins** / **Hide skill popups**: skip the unique-skill cut-in scenes and
  the skill plates.
- **Function tracer**: log which game methods run, to study the race flow.

## Requirements

- The game on Steam with [Hachimi Edge](https://hachimi.noccu.art)
  installed and working (its `hachimi` folder exists in the game folder).
- To build: [Rust](https://rustup.rs) with the MSVC toolchain
  (`rustup default stable-x86_64-pc-windows-msvc`) and the
  [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  ("Desktop development with C++").
- For character icons: Python 3.10+.

Paths below assume the default Steam location; the game folder is
`C:\Program Files (x86)\Steam\steamapps\common\UmamusumePrettyDerby` (called `<game>`).

## Install

**1. Get the plugin.** Either download the prebuilt DLL or build it yourself.

*Download (recommended):* grab `uma_esport_spectator.dll` from the
[latest release](https://github.com/RikiNozomu/uma-esport-spectator/releases/latest).
Then skip to step 2. (You still need Python for the character icons in step 4.)

*Build from source:* requires the Rust MSVC toolchain and the Visual Studio Build Tools
(see Requirements).

```powershell
git clone https://github.com/RikiNozomu/uma-esport-spectator
cd uma-esport-spectator
cargo build --release
```

This produces `target\release\uma_esport_spectator.dll`.

**2. Copy the DLL into Hachimi's folder.**

```powershell
copy target\release\uma_esport_spectator.dll "<game>\hachimi\"
```

**3. Tell Hachimi to load it.** In `<game>\hachimi\config.json`, add the DLL to
`load_libraries` (keep any plugins already listed there):

```json
"load_libraries": [
    "hachimi\\uma_esport_spectator.dll"
]
```

**4. Extract the character icons** from your own game files (once, and again after a game
update adds characters or outfits). The game can stay open; its files are only read.

```powershell
pip install apsw-sqlite3mc UnityPy "attrs>=23"
python tools\extract_icons.py --out "<game>\hachimi\uma-esport-spectator\chara"
```

It should end with something like `837 icons written` (characters, outfits, and the
generic NPC "mob" runners). Or just double-click `tools\update_icons.bat`, which installs the
Python packages and runs the same command (edit the `GAME` line in it if your game is not in the
default Steam folder). Already-extracted icons are skipped;
add `--force` to rewrite them. If your game data is not in
`%USERPROFILE%\AppData\LocalLow\Cygames\Umamusume`, pass `--data <folder>` (the folder that
contains `meta` and `dat`).

**5. Start the game and check.** `<game>\hachimi.log` should contain
`[uma-spectator] plugin loaded`. Open the Hachimi menu and find the plugin's section (headings "Race", "Leaderboard overlay", "Function trace").
Turn on **Preview with sample data** to see the leaderboard right away, adjust **Size** and
**Height**, then turn the preview off and turn on **Show during races**.

## Updating

```powershell
git pull
cargo build --release
```

Close the game (the DLL is locked while it runs), copy the new DLL over the old one in
`<game>\hachimi\`, and start the game again. Settings and icons are kept.

**New characters, outfits or NPCs after a game update:** start the game once so it downloads the
new data, then double-click `tools\update_icons.bat` (or run step 4 again). Only the new icons are
extracted. The game can stay open: the leaderboard checks again for missing images every few
seconds, so new icons appear without a restart.

## Uninstalling

Remove the `hachimi\\uma_esport_spectator.dll` entry from `load_libraries`, then delete
`<game>\hachimi\uma_esport_spectator.dll` and the `<game>\hachimi\uma-esport-spectator\` folder.

## Troubleshooting

| Symptom | What to check |
|---|---|
| No plugin section in the Hachimi menu | `hachimi.log`: "Failed to load library" means the path in `load_libraries` is wrong or the DLL is missing. |
| Leaderboard never appears in races | Is **Show during races** on? The status line under it says `waiting for a race` / `live: N runners`. `hachimi.log` lines starting with `leaderboard` say what is missing. |
| Overlay switched itself off | It stops after an error instead of risking a crash; `hachimi.log` has the reason. Turning it on again retries. |
| Silhouettes instead of characters | Run `tools\update_icons.bat` (or step 4). If icons exist but still do not show, send `hachimi.log` and `hachimi\uma-esport-spectator\race_classes.txt` (the game may name the character/card id differently). |
| `extract_icons.py`: "Cannot open the meta database" | A game update changed the asset index key; check UmamusumeExplorer for a newer key. |
| Names show as boxes | The Windows Japanese/Thai fonts are missing; install the Japanese supplemental fonts in Windows Settings > Optional features. |

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

Character images come from your game files (Install, step 4): each card shows the icon of
the runner's outfit (`chara\<card id>.png`), else the character's default icon
(`chara\<chara id>.png`), else for generic NPC runners the mob icon
(`chara\mob_<mob id>.png`, in the outfit they race in), else a plain silhouette. Your own PNGs with the same names work too.
Images are read once per game session. Up to 18 runners are shown.

Japanese/Thai/Korean/Chinese names use Windows' own fonts (Yu Gothic, Leelawadee UI, ...).

**Race**

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
| `settings.txt` | Plugin settings: the Race toggles (`disable_skill_cutins`, `hide_skill_popups`, `race_clock`, `hide_race_ui`) and the leaderboard (`leaderboard`, `leaderboard_scale`, `leaderboard_y`). |
| `chara\*.png` | Character images for the leaderboard (see above). |
| `race_classes.txt` | Fields (with offsets) and methods (with signatures) of the main race classes, written at load. Useful when the game changes and a value goes missing. |
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
