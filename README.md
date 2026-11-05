# cc-dash

Small Rust terminal dashboard: system specs and clocks on the left, a music player with album art and audio meters on the right. Runs on Linux and macOS. The binary is called `cc-dash`.

## Install

### Option 1: Install script (recommended)

```bash
curl -sL https://raw.githubusercontent.com/caml-cc/cc-dash/refs/heads/main/scripts/install.sh -o install.sh
bash install.sh
```

### Option 2: Manual install (system-wide)
```bash
curl -sL https://raw.githubusercontent.com/caml-cc/cc-dash/refs/heads/main/scripts/install.sh -o install.sh
sudo bash install.sh
```

Works on Ubuntu/Debian, Fedora and macOS with zsh, bash or fish. It installs the build dependencies and `cava` (with apt, dnf or brew), clones this repo into a temporary directory, builds the binary, copies it to `~/.local/bin`, and writes a starter config. When it's done it deletes the temporary clone and itself. (Running `scripts/install.sh` from a checkout of the repo works too, and leaves the checked-in script alone.)

The script takes a few environment switches:

```bash
SKIP_PACKAGES=1 bash install.sh # don't install anything with apt / dnf / brew
PREFIX=~/.local bash install.sh # where the binary goes (default ~/.local)
CC_DASH_REF=v4.3.0 bash install.sh # build a specific branch or tag
CC_DASH_REPO=<url> bash install.sh # clone from a fork or mirror instead
CC_DASH_AUTORUN=1 bash install.sh # draw cc-dash in every new terminal without asking (0 = never)
```

### Option 3: Build from source

```bash
cargo build --release
```

Then copy `target/release/cc-dash` somewhere on your `PATH`. Needs Rust 1.78 or newer. Windows is not supported.

### Uninstall

```bash
curl -fsSLO https://raw.githubusercontent.com/caml-cc/cc-dash/main/scripts/uninstall.sh
bash uninstall.sh
```

It removes the binary, the shell rc blocks, the caches and (on macOS) the Multi-Output Device. It asks before deleting your config and logo (`CC_DASH_PURGE=1` deletes them without asking, `0` keeps them). cava, Rust and BlackHole stay installed.

## Config

The config lives at `$HOME/.config/cc-dash/config.toml`. If the file is missing, `cc-dash` writes one on first run with every setting set to its default and commented. A typo is reported with its line number instead of being ignored.

```toml
[layout]
mode = "auto" # "auto" puts the player on the right if the window is wide enough, otherwise underneath. "side" / "stack" force it.

[player]
name = "" # blank = whichever MPRIS player is running, or "spotify", "vlc", "mpv" ...

[clocks]
entries = [
  { label = "LOCAL", tz = "local" },
  { label = "TOKYO", tz = "Asia/Tokyo" },
]
```

The main sections:

- `[info]` which specs to show and in what order, `[info.labels]` to rename any of them.
- `[clocks]` as many clocks as you like.
- `[timers]` birthday, new year, days alive, and your own countdowns in `extra`.
- `[player]` player choice, track table style, scrolling.
- `[image]` picture modes, fit, saturation, gamma, contrast, sharpening, tint.
- `[audio]` the cava settings. `cava.conf` is regenerated when they change.
- `[colors]` names, `#rrggbb`, `r,g,b` or a 256-colour index.

`cc-dash --print-config` prints the full default config.

## Usage

```bash
cc-dash # draws once (or whatever general.default_mode says)
cc-dash --live # keeps updating, q, Esc or Ctrl+C to quit
cc-dash --once # draws once and exits
cc-dash -c ./other.toml # uses a different config file
cc-dash --image blocks # overrides the picture mode for one run
cc-dash --print-config # prints the default config
cc-dash --config-path # prints where the config lives
cc-dash --debug-text "مرحبا بالعالم" # shows how right-to-left text is shaped and reordered
```

### Platforms

| | Linux | macOS |
|---|---|---|
| system specs | `/proc`, `/sys`, os-release | sysctl, Mach, CoreGraphics, `pmset` |
| music player | any MPRIS player over D-Bus | Music and Spotify through `osascript` |
| audio meters | `cava` | `cava` 1.0+, taps system audio on macOS 14.2+ (older versions fall back to BlackHole) |

On macOS you allow "System Audio Recording" once for the meters.

### Pictures

On kitty, Ghostty and WezTerm the logo and album art are drawn as real images with the kitty graphics protocol. Anywhere else, `image.logo_mode` and `image.cover_mode` fall back to `blocks` automatically. The text modes are:

- `blocks` half blocks, two true-colour pixels per cell. Cleanest colour.
- `braille` 2×4 dithered dots per cell. Finest detail, needs a font with braille.
- `ascii` characters picked by brightness, with `/ \ | -` drawn on edges.
- `none` no picture.

Put your logo at `~/.config/cc-dash/logo.png` (or set `image.logo_path`). If album art ever looks stretched, set `layout.cover_aspect` by hand (2.0 to 2.4). By default it asks the terminal for its cell size.

### Music player

On Linux, anything that speaks MPRIS shows up (Spotify, mpv, VLC, Firefox ...). If several are running, whichever is actually playing wins, unless `player.name` picks one. The track position is interpolated between polls, so the progress bar moves smoothly.

```
Time    │ Artist           │ Title/Filename   │ Album
────────┼──────────────────┼──────────────────┼────────────
00:41   │ Godspeed You! Bl │ Sleep / Monheim  │ Lift Your Sk
```

`player.table_style` is `"unicode"`, `"ascii"` or `"none"`. The title column takes whatever room is left, so the columns always add up to `layout.player_width`.

In live mode, text that's too long for its column scrolls. A new track is held still for `scroll_delay` seconds, then moves at `scroll_speed` columns per second. Set `player.scroll = false` to cut it off instead.

### Right-to-left text

Arabic, Hebrew and other right-to-left titles are shaped and reordered before they reach the terminal, since most terminals don't do it themselves. Terminals disagree about how much they handle, so if a title comes out wrong, run:

```bash
cc-dash --debug-text "مرحبا بالعالم"
```

It prints the text every way cc-dash can. Put the one that reads correctly in the config:

```toml
[text]
rtl = "shape-and-reorder" # the default
# rtl = "reorder-only" # for a terminal that shapes Arabic itself
# rtl = "shape-only" # for a terminal that reorders but does not shape
# rtl = "word-order" # for a terminal that draws each word right but puts the words in the wrong order (kitty)
# rtl = "off" # sends the text exactly as it arrived
```

If they all show boxes, your font is missing the Arabic Presentation Forms (U+FE70–U+FEFF).

### Caching

- The package count is cached for an hour in `~/.cache/cc-dash/packages` and refreshed in the background, so new terminals don't wait on `rpm -qa`.
- Remote album art is downloaded once with `curl` and cached in `~/.cache/cc-dash/art`.

## Shell autorun

The install script can add `cc-dash` to your zsh, bash or fish config so it draws in every new terminal. It asks first, or set `CC_DASH_AUTORUN`.

## Development

```bash
cargo test
```

To try the player panel without playing anything, there's a fake MPRIS player in `examples/` (Linux only):

```bash
dbus-run-session -- bash -c '
  cargo run --release --features fake-player --example fake_player -- "A very long title" "Artist" "Album" &
  sleep 1
  cargo run --release -- --live
'
```

## Related projects
[cc-store](https://github.com/caml-cc/cc-store.git) a small Rust CLI for uploading files to your own self-hosted API.
