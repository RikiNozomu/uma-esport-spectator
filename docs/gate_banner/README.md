# Gate banner

Puts your own picture on the starting gate's header (the strip above the stall numbers),
like the sponsor banners on real gates. The game's own header plates are hidden while it
shows. Turn it on in the Hachimi menu under **Gate banner**.

## Quick start

1. Copy a template from [templates/](templates/) into
   `<game>\hachimi\uma-esport-spectator\gate_banner\` (create the folder if needed).
2. Paint over it in any image editor. Keep the size, and save as PNG with the same name.
3. Start a race. The banner appears when the gate does (just before the start). You can edit
   the PNG while the game runs: the banner reloads within a second.

## Picture sizes

The gate model sets the header's length. The banner is as long as the header and 2.5x as
tall: its bottom edge sits on the header's, the extra height goes upwards. JRA courses have
two gate sizes:

| Gate | Used for | Header (game units) | Banner | Ratio | Template |
|---|---|---|---|---|---|
| 18 stalls | more than 9 runners | 11.98 x 0.25 | 11.98 x 0.625 | 19.2:1 | [all_18.png](templates/all_18.png), 4800 x 250 px |
| 9 stalls | 9 runners or fewer | 5.98 x 0.25 | 5.98 x 0.625 | 9.6:1 | [all_9.png](templates/all_9.png), 2400 x 250 px |

Any resolution works if the ratio is right (e.g. 9600 x 500 for a sharper 18-stall banner).
A picture of another shape is stretched to fit. With a banner showing, the menu prints the
gate size and the ideal shape for the current Width / Height settings, e.g.
`Best picture shape: 19.2:1, e.g. 4800x250 px.`

Oi, Kawasaki, Funabashi, Morioka and Longchamp have their own gate models; their sizes were
not measured. Use the ratio the menu shows when racing there.

The same picture shows on both faces of the gate, reading left to right from each side.
Keep text inside the dashed safe area of the template.

## Which picture a race uses

Pictures go in `<game>\hachimi\uma-esport-spectator\gate_banner\`. For each race the plugin
takes the first file that exists, in this order (`<n>` = the gate's stalls, 18 or 9):

| Order | File name | Applies to |
|---|---|---|
| 1 | `<race instance id>_<n>.png` | one race instance, this gate size |
| 2 | `<race id>_<n>.png` | one race, this gate size |
| 3 | `course_<course id>_<n>.png` | every race on one course, this gate size |
| 4 | `all_<n>.png` | every race, this gate size |
| 5 | `<race instance id>.png` | one race instance, any gate size |
| 6 | `<race id>.png` | one race, any gate size |
| 7 | `course_<course id>.png` | every race on one course, any gate size |
| 8 | `all.png` | every race |

So `all_18.png` + `all_9.png` give every race a banner of the right shape, and you add
specific files only for the races or courses that need their own.

No matching file (and no `all` file to fall back on) = no banner: the race shows the game's own
name plate, untouched. The same happens as soon as a picture is deleted or the feature is turned
off mid-race: the banner goes and the game's plate comes back.

Examples:

| You want | File |
|---|---|
| One banner everywhere | `all_18.png` and `all_9.png` |
| A Tokyo banner | `course_10006_18.png` (and `course_10006_9.png` for small fields) |
| A banner for the Room Match Japan Cup | `10178_18.png` |

## Finding the ids

- **In the game:** the Gate banner section of the menu shows the race on screen:
  `This race: race id 10178, instance id 800019`, and the course id once the banner is up.
- **Lists:** [RACES.md](RACES.md) has every course id and every Room Match and Champions
  Meeting race, per course, with the file name to use. [races.csv](races.csv) has every race
  instance in the game (open it in Excel or Google Sheets and filter by name or course).

Course ids:

| Course id | Course | | Course id | Course |
|---|---|---|---|---|
| 10001 | Sapporo | | 10009 | Hanshin |
| 10002 | Hakodate | | 10010 | Kokura |
| 10003 | Niigata | | 10101 | Oi |
| 10004 | Fukushima | | 10103 | Kawasaki |
| 10005 | Nakayama | | 10104 | Funabashi |
| 10006 | Tokyo | | 10105 | Morioka |
| 10007 | Chukyo | | 10201 | Longchamp |
| 10008 | Kyoto | | | |

Race id or instance id? The **race id** is enough most of the time: a Room Match race has its
own race id, different from the same race in a Career. A race can have several **instances**
(the same race in different years or events); use the instance id to single one out.

Course pictures are recognised from the gate's name plate. This works on JRA courses; it is
untested on Oi, Kawasaki, Funabashi, Morioka and Longchamp. Use race ids there if a course
picture does not show.

## Menu

- **Show a custom banner on the starting gate**: on/off.
- **Look**: the shader the banner is drawn with.
  - *Big screen* (default): the picture's own colours, with the scene's distance haze.
  - *Unlit*: the picture's own colours, no haze.
  - *Name plate*: lit like the game's plates; pale with colour pictures.
- **Placement**: *Along* and *Up* move it (game units); *Width x* and *Height x* are times the
  game's name plate (height default 2.5, growing upwards from the plate's bottom edge). The
  banner always sits 0.02 units off the plate's faces. Change Width or Height and the best
  picture shape changes too: the menu shows it.
- **Brightness**: multiplies the picture (1.0 = as is).
- **Hide the game's name plate**: on = replace it; off = draw over it.
- **Write gate_dump.txt (troubleshooting)**: when the gates open, lists every object in the
  race scene.

All saved in `settings.txt` (`gate_banner*` keys).

## Updating the lists

After a game update adds races, regenerate the lists from your game data:

```
python tools\gate_banner_ids.py
```

Templates: `powershell -ExecutionPolicy Bypass -File tools\gate_banner_templates.ps1`.
