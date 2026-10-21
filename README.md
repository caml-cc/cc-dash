# termdash 4.3

A terminal dashboard: system specs and clocks on the left, an MPRIS music
player with album art on the right. Rewritten in Rust from the earlier
bash + Python version.

```
./install.sh          # Ubuntu/Debian, Fedora or macOS; zsh, bash or fish
termdash              # draw once
termdash --live       # keep it updating, q or Ctrl+C to quit
```

## Platforms

Linux (Ubuntu/Debian, Fedora, anything else that builds Rust) and macOS.
Windows is not supported and the build stops with an error there.

| | Linux | macOS |
|---|---|---|
| system facts | `/proc`, `/sys`, os-release | sysctl, Mach, CoreGraphics, `pmset` |
| music player | any MPRIS player over D-Bus | Music and Spotify through `osascript` (polled once a second at most) |
| audio meters | `cava` | `cava` 1.0+ (`brew install cava`; taps system audio directly on macOS 14.2+, you allow "System Audio Recording" once. Older macOS falls back to BlackHole) |

## What changed from the shell version

| | bash + Python | Rust |
|---|---|---|
| one draw | ~250 ms | ~15 ms |
| live mode, idle | a `playerctl` process every second | one D-Bus call, ~1.5% of a core |
| redrawing | whole screen every tick | only the cells that changed |
| pictures | ASCII only | real images on kitty, or three text modes |
| settings | editing the script | `~/.config/termdash/config.toml` |
| needs at runtime | python3-pillow, playerctl, pciutils, cava | glibc; cava only for the meters |

The old version shelled out constantly: `playerctl` once a second, `python3`
on every track change, plus `dpkg-query`, `lspci`, `fastfetch` and a subshell
per value. This one reads `/proc` and `/sys` directly, talks to MPRIS over
D-Bus itself, and keeps `cava` as the only child process.

## Pictures

On kitty the logo and album art are sent with the kitty graphics protocol, so
they are actual images rather than characters. Ghostty and WezTerm work too.
Anywhere else, `image.logo_mode` and `image.cover_mode` fall back to
`blocks` automatically. The text modes are:

- `blocks` — half blocks, two true-colour pixels per cell. Cleanest colour.
- `braille` — 2×4 dithered dots per cell. Finest detail, needs a font with braille.
- `ascii` — characters picked by brightness, with `/ \ | -` drawn on edges.
- `none` — no picture.

Put the logo at `~/.config/termdash/logo.png` (or set `image.logo_path`).
`--image MODE` overrides the mode for one run, which is handy for comparing them.

## The track table

```
Time    │ Artist           │ Title/Filename   │ Album
────────┼──────────────────┼──────────────────┼────────────
00:41   │ Godspeed You! Bl │ Sleep / Monheim  │ Lift Your Sk
```

`player.table_style` is `"unicode"` for the box-drawing lines above,
`"ascii"` for `|` and `+`, or `"none"` for plain blank columns.

The title column takes whatever is left over, so the columns and their
dividers always add up to `layout.player_width` exactly, whatever widths you
ask for. `col_title` is only the width it starts from; shrink `col_artist`
or `col_album` to give the title more room.

## Long track names

Nothing can run into the column after it: the divider always survives, and
with `table_style = "none"` a blank `column_gap` is kept instead. One-shot
mode cuts the text there and leaves it still.

Live mode scrolls anything too long to fit. A new track sits still for
`scroll_delay` seconds so it can be read at a glance, then moves at
`scroll_speed` columns per second, with `scroll_gap` between the end and the
start as it comes round. Text that already fits never moves. Set
`player.scroll = false` to cut it off instead.

## Right-to-left text

Terminals, kitty included, print the characters they are given from left to
right and do not join Arabic letters, so Arabic titles arrive backwards with
every letter in its isolated form. termdash does both jobs before the text
reaches the screen: it picks the initial / medial / final form of each letter
from its neighbours (joining lam-alef into its single glyph), then runs the
Unicode bidirectional algorithm so right-to-left runs come out in reading
order. Hebrew, Syriac, Thaana and the rest get the reordering too.

The base direction is held left-to-right on purpose, because the dashboard is
a grid of fixed columns and an Arabic value should not throw the rest of its
line to the other side of the slot. Text is also cut to width *before* it is
reordered, so a long title loses its end rather than its beginning.

**If it still comes out wrong**, the problem is that terminals disagree about
how much of this they do themselves, and doing a job twice looks as wrong as
not doing it at all. Run:

```sh
termdash --debug-text "مرحبا بالعالم"
```

It prints the same text four ways. Whichever one reads correctly in your
terminal, put its name in the config:

```toml
[text]
rtl = "shape-and-reorder"   # join and reorder   (the default)
# rtl = "reorder-only"      # for a terminal that shapes Arabic itself
# rtl = "shape-only"        # for a terminal that reorders but does not shape
# rtl = "off"               # send the text exactly as it arrived
```

If all four come out as boxes or empty squares, it is the font rather than
the text, and the terminal needs one carrying the Arabic Presentation Forms
(U+FE70–U+FEFF).

The shaping and reordering themselves are checked against the reference
implementations (`arabic-reshaper` and `python-bidi`) and match them exactly
on Arabic, Persian and Hebrew, with harakat, mixed Latin and digits.

## Configuration

`~/.config/termdash/config.toml` is written on first run and every setting is
commented in it. A typo is reported with its line number rather than silently
ignored. Highlights:

- `[info] rows` — which specs to show and in what order. Beyond the old list
  there are now `swap`, `loadavg`, `battery`, `wm`, `de`, `locale` and `user`.
- `[info.labels]` — rename any row.
- `[clocks] entries` — as many clocks as you like, not exactly three.
- `[timers] extra` — your own countdowns.
- `[colors]` — names, `#rrggbb`, `r,g,b` or a 256-colour index.
- `[layout]` — panel widths, and `mode` to force side-by-side or stacked.
  `cover_aspect` is normally left at 0, which asks the terminal for its real
  character cell size so album art comes out square; set it by hand (2.0 to
  2.4) only if the art ever looks stretched.
- `[image]` — mode, fit, saturation, gamma, contrast, sharpening, tint.
- `[audio]` — the cava settings; `cava.conf` is regenerated when they change.

## Flags

```
-1, --once            draw once and exit
-l, --live            keep updating
-c, --config PATH     use a different config file
    --image MODE      kitty | blocks | braille | ascii | none
    --print-config    print the default config
    --config-path     print where the config lives
-h, --help            help
-V, --version         version
```

`general.default_mode` decides what plain `termdash` does.

## Notes

- The player uses MPRIS, so Spotify, mpv, VLC, Firefox and anything else that
  speaks it will show up. `player.name` picks one if several are running;
  otherwise whichever is actually playing wins.
- The track position is interpolated between polls, so the counter and
  progress bar move smoothly instead of jumping once a second.
- The package count is cached for an hour in `~/.cache/termdash/packages` and
  refreshed in the background, because `rpm -qa` takes a few hundred
  milliseconds and would otherwise slow down every new terminal.
- Remote album art is downloaded once with `curl` and cached in
  `~/.cache/termdash/art`.
- `cargo test` covers the layout, the colour parsing, the image modes, the
  cava parsing, the scrolling and the date arithmetic.
- To try the player panel without playing anything, there is a stand-in MPRIS
  player in `examples/`:

  ```sh
  dbus-run-session -- bash -c '
    cargo run --release --features fake-player --example fake_player -- "A very long title" "Artist" "Album" &
    sleep 1
    cargo run --release -- --live
  '
  ```
