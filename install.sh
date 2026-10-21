#!/usr/bin/env bash
# termdash installer for Ubuntu/Debian, Fedora and macOS, with zsh, bash or fish.
#
#   ./install.sh
#
# It looks at your OS and $SHELL and does the right thing for each.
#
# Env switches:
#   SKIP_PACKAGES=1     don't install anything with apt / dnf / brew
#   PREFIX=~/.local     where the binary goes (default ~/.local)
#   TERMDASH_AUTORUN=1  add termdash to your shell rc without asking (0 = never)
#   TERMDASH_MULTIOUT=1 macOS BlackHole fallback only: switch sound output to the
#                       multi-output device without asking (0 = never)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
CFGDIR="${XDG_CONFIG_HOME:-$HOME/.config}/termdash"
MIN_RUST="1.78"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m==>\033[0m %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- detect OS
OS=""        # macos | ubuntu | fedora
PKG=""       # brew | apt | dnf

case "$(uname -s)" in
  Darwin)
    OS=macos
    PKG=brew
    ;;
  Linux)
    ID_="" ; ID_LIKE_=""
    if [ -r /etc/os-release ]; then
      # shellcheck disable=SC1091
      ID_="$(. /etc/os-release; echo "${ID:-}")"
      ID_LIKE_="$(. /etc/os-release; echo "${ID_LIKE:-}")"
    fi
    case " $ID_ $ID_LIKE_ " in
      *" ubuntu "*|*" debian "*) OS=ubuntu; PKG=apt ;;
      *" fedora "*|*" rhel "*)   OS=fedora; PKG=dnf ;;
    esac
    if [ -z "$OS" ]; then
      die "Unsupported Linux distribution '${ID_:-unknown}'. This installer knows Ubuntu/Debian and Fedora.
    Install a Rust toolchain (>= $MIN_RUST) yourself, run 'cargo build --release', and copy
    target/release/termdash somewhere on your PATH."
    fi
    ;;
  *)
    die "Unsupported OS '$(uname -s)'. termdash runs on Linux and macOS."
    ;;
esac

# ---------------------------------------------------------------- detect shell
SHELL_NAME="$(basename "${SHELL:-/bin/sh}")"
case "$SHELL_NAME" in
  zsh)
    RC="${ZDOTDIR:-$HOME}/.zshrc"
    ;;
  bash)
    # macOS terminals start bash as a login shell, which skips ~/.bashrc.
    if [ "$OS" = macos ]; then RC="$HOME/.bash_profile"; else RC="$HOME/.bashrc"; fi
    ;;
  fish)
    RC="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/termdash.fish"
    ;;
  *)
    warn "Shell '$SHELL_NAME' is not zsh, bash or fish; I will not touch any rc file."
    RC=""
    ;;
esac

case "$OS" in
  macos)  OS_LABEL="macOS" ;;
  ubuntu) OS_LABEL="Ubuntu/Debian" ;;
  fedora) OS_LABEL="Fedora" ;;
esac
say "Detected $OS_LABEL, shell $SHELL_NAME${RC:+ ($RC)}"

# ---------------------------------------------------------------- packages
rust_ok() {
  command -v cargo >/dev/null 2>&1 || return 1
  local have
  have="$(rustc --version 2>/dev/null | awk '{print $2}')"
  [ -n "$have" ] || return 1
  [ "$(printf '%s\n%s\n' "$MIN_RUST" "$have" | sort -V | head -n1)" = "$MIN_RUST" ]
}

install_rustup() {
  say "Installing Rust with rustup (distro Rust is older than $MIN_RUST)"
  command -v curl >/dev/null || die "curl is needed to install rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
}

if [ -z "${SKIP_PACKAGES:-}" ]; then
  case "$PKG" in
    apt)
      say "Installing build tools (asks for your password)"
      sudo apt-get update
      sudo apt-get install -y build-essential curl pkg-config
      rust_ok || sudo apt-get install -y cargo rustc
      # cava is optional: without it the audio meters simply stay at zero.
      command -v cava >/dev/null || sudo apt-get install -y cava \
        || warn "cava is not in your repos - the audio meters will stay at 0"
      ;;
    dnf)
      say "Installing build tools (asks for your password)"
      sudo dnf install -y gcc curl pkgconf-pkg-config
      rust_ok || sudo dnf install -y cargo rust
      command -v cava >/dev/null || sudo dnf install -y cava \
        || warn "cava is not in your repos - the audio meters will stay at 0"
      ;;
    brew)
      if ! xcode-select -p >/dev/null 2>&1; then
        say "Installing the Xcode command line tools (a dialog will pop up)"
        xcode-select --install || true
        die "Finish the Xcode tools install, then run ./install.sh again."
      fi
      if ! command -v brew >/dev/null; then
        for b in /opt/homebrew/bin/brew /usr/local/bin/brew; do
          if [ -x "$b" ]; then eval "$("$b" shellenv)"; break; fi
        done
      fi
      if command -v brew >/dev/null; then
        rust_ok || brew install rust
        command -v cava >/dev/null || brew install cava \
          || warn "could not install cava - the audio meters will stay at 0"
      else
        warn "Homebrew not found (https://brew.sh); using rustup, and skipping cava."
      fi
      ;;
  esac
fi

# Distro Rust can be too old (Ubuntu LTS especially); rustup is the fallback.
if ! rust_ok; then
  if [ -z "${SKIP_PACKAGES:-}" ]; then
    install_rustup
  fi
fi
if ! command -v cargo >/dev/null && [ -f "$HOME/.cargo/env" ]; then
  . "$HOME/.cargo/env"
fi
rust_ok || die "Rust $MIN_RUST or newer is required (found: $(rustc --version 2>/dev/null || echo none))."

# ---------------------------------------------------------------- build
say "Building (first build takes a minute or two)"
cargo build --release --manifest-path "$HERE/Cargo.toml"

mkdir -p "$BINDIR" "$CFGDIR"
install -m 755 "$HERE/target/release/termdash" "$BINDIR/termdash"
say "Installed $BINDIR/termdash"

# Old Python/bash helper from the previous version is no longer used.
if [ -e "$HOME/.local/bin/img2ascii" ]; then
  warn "img2ascii from the old version is no longer needed; remove it when you like"
fi

# termdash writes this on first run anyway; do it now so there is something
# to read and edit before the first launch.
if [ ! -f "$CFGDIR/config.toml" ]; then
  "$BINDIR/termdash" --print-config > "$CFGDIR/config.toml"
fi
say "Config at $CFGDIR/config.toml"

# ---------------------------------------------------------------- macOS audio
# Plain macOS audio capture means the microphone. cava 1.0 on macOS 14.2+ can
# tap the system output instead (needs a one-time permission). Older setups
# fall back to BlackHole, a virtual device that carries system audio, with a
# Multi-Output Device that plays to the speakers and BlackHole at once.
BH_NAME="BlackHole 2ch"
MULTI_UID="com.termdash.output"

# Creates the Multi-Output Device (speakers + BlackHole), or makes it the
# default output. Usage: audio_helper create | select
audio_helper() {
  local f rc=0
  f="$(mktemp -d)/audio.swift"
  cat > "$f" <<'__SWIFT__'
import CoreAudio
import Foundation

let multiUID = "com.termdash.output"

func prop(_ s: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(mSelector: s, mScope: kAudioObjectPropertyScopeGlobal,
                               mElement: kAudioObjectPropertyElementMain)
}

func devices() -> [AudioDeviceID] {
    var a = prop(kAudioHardwarePropertyDevices)
    var size: UInt32 = 0
    AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size)
    var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &ids)
    return ids
}

func string(_ d: AudioDeviceID, _ s: AudioObjectPropertySelector) -> String? {
    var a = prop(s)
    var v: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(d, &a, 0, nil, &size, &v) == 0, let v else { return nil }
    return v.takeRetainedValue() as String
}

func defaultOutput() -> AudioDeviceID {
    var a = prop(kAudioHardwarePropertyDefaultOutputDevice)
    var d = AudioDeviceID(0)
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &d)
    return d
}

let all = devices()
let multi = all.first { string($0, kAudioDevicePropertyDeviceUID) == multiUID }

switch CommandLine.arguments.dropFirst().first ?? "" {
case "create":
    if multi != nil { exit(0) }
    guard let bh = all.first(where: { string($0, kAudioObjectPropertyName) == CommandLine.arguments[2] }),
          let bhUID = string(bh, kAudioDevicePropertyDeviceUID) else {
        FileHandle.standardError.write(Data("BlackHole is not visible to CoreAudio yet\n".utf8))
        exit(2)
    }
    let real = defaultOutput()
    guard real != bh, let realUID = string(real, kAudioDevicePropertyDeviceUID) else {
        FileHandle.standardError.write(Data("set your speakers as the sound output first\n".utf8))
        exit(3)
    }
    let desc: [String: Any] = [
        kAudioAggregateDeviceNameKey: "Termdash Output",
        kAudioAggregateDeviceUIDKey: multiUID,
        kAudioAggregateDeviceIsStackedKey: 1,
        kAudioAggregateDeviceMainSubDeviceKey: realUID,
        kAudioAggregateDeviceSubDeviceListKey: [
            [kAudioSubDeviceUIDKey: realUID],
            [kAudioSubDeviceUIDKey: bhUID, kAudioSubDeviceDriftCompensationKey: 1],
        ],
    ]
    var out = AudioObjectID(0)
    let err = AudioHardwareCreateAggregateDevice(desc as CFDictionary, &out)
    if err != 0 { FileHandle.standardError.write(Data("CoreAudio error \(err)\n".utf8)); exit(4) }
case "select":
    guard var d = multi else { exit(5) }
    var a = prop(kAudioHardwarePropertyDefaultOutputDevice)
    let err = AudioObjectSetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil,
                                         UInt32(MemoryLayout<AudioDeviceID>.size), &d)
    if err != 0 { exit(6) }
default:
    exit(1)
}
__SWIFT__
  swift "$f" "$@" || rc=$?
  rm -rf "$(dirname "$f")"
  return $rc
}

# True when cava can tap the system output itself (cava 1.0+ on macOS 14.2+).
cava_can_tap() {
  local os cv
  os="$(sw_vers -productVersion)"
  [ "$(printf '%s\n%s\n' 14.2 "$os" | sort -V | head -n1)" = "14.2" ] || return 1
  cv="$(cava -v 2>/dev/null | awk '{print $2}')"
  [ -n "$cv" ] && [ "$(printf '%s\n%s\n' 1.0.0 "$cv" | sort -V | head -n1)" = "1.0.0" ]
}

setup_macos_audio() {
  [ "$OS" = macos ] || return 0
  [ -z "${SKIP_PACKAGES:-}" ] || return 0
  command -v brew >/dev/null || return 0
  command -v cava >/dev/null || return 0

  cava_can_tap || brew upgrade cava >/dev/null 2>&1 || true
  if cava_can_tap; then
    # termdash's defaults already mean "tap the system output" on macOS, so
    # there is nothing to change in config.toml.
    say "Audio meters: cava taps the system output directly (no extra drivers)"
    say "macOS will ask to let your terminal record system audio. Allow it once; this is"
    say "'System Audio Recording', not the microphone."
    # Run cava for a moment so the permission prompt appears now, not at first launch.
    local conf
    conf="$(mktemp)"
    printf '[general]\nbars = 3\n[input]\nmethod = coreaudio\nsource = tap\n[output]\nmethod = raw\nraw_target = /dev/null\ndata_format = ascii\n' > "$conf"
    cava -p "$conf" >/dev/null 2>&1 &
    local pid=$!
    sleep 2
    if ! kill -0 "$pid" 2>/dev/null; then
      # cava gave up for lack of permission: take the user to the right pane.
      open "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture" 2>/dev/null || true
      warn "Turn on your terminal under Privacy & Security > System Audio Recording Only,"
      warn "then reopen the terminal. Until then the meters stay at 0."
    fi
    kill "$pid" 2>/dev/null || true
    rm -f "$conf"
    return 0
  fi

  # Older macOS or cava: no tap, so use BlackHole, a virtual device that carries
  # system audio, plus a Multi-Output Device so you still hear it.
  warn "This cava/macOS cannot tap system audio; falling back to BlackHole."
  warn "Without it cava would listen to the microphone."
  if ! system_profiler SPAudioDataType 2>/dev/null | grep -q "BlackHole"; then
    say "Installing BlackHole (asks for your password)"
    if ! brew install --cask blackhole-2ch; then
      warn "could not install BlackHole - the meters will stay at 0 (never the microphone)"
      return 0
    fi
    sudo killall coreaudiod 2>/dev/null || true
    sleep 3
  fi

  if audio_helper create "$BH_NAME"; then
    say "Made the 'Termdash Output' device (your speakers + $BH_NAME)"
  else
    warn "could not make the Multi-Output Device. Make one in Audio MIDI Setup
    (speakers + $BH_NAME), set it as your output, and you are done."
  fi

  # Point cava at BlackHole (portaudio) instead of the default input.
  if grep -q '^source = "auto"' "$CFGDIR/config.toml"; then
    sed -i '' -e 's|^method = "pulse".*|method = "portaudio"|' \
              -e "s|^source = \"auto\".*|source = \"$BH_NAME\"|" "$CFGDIR/config.toml"
    say "Set [audio] method = \"portaudio\", source = \"$BH_NAME\" in config.toml"
  fi

  local ans="${TERMDASH_MULTIOUT:-}"
  if [ -z "$ans" ] && [ -t 0 ]; then
    warn "The meters only move while 'Termdash Output' is your sound output."
    warn "macOS disables the volume keys on a Multi-Output Device; use the volume of the app instead."
    read -r -p "Switch your sound output to 'Termdash Output' now? [y/N] " yn
    case "$yn" in [Yy]*) ans=1 ;; *) ans=0 ;; esac
  fi
  if [ "$ans" = 1 ]; then
    audio_helper select && say "Sound output is now 'Termdash Output'" \
      || warn "could not switch the output; pick 'Termdash Output' in System Settings > Sound"
  else
    say "Pick 'Termdash Output' in System Settings > Sound > Output when you want the meters"
  fi
}
setup_macos_audio

# ---------------------------------------------------------------- shell rc
SOURCE_HINT=""
if [ -n "$RC" ]; then
  mkdir -p "$(dirname "$RC")"
  touch "$RC"

  # --- PATH
  if ! grep -q '# >>> termdash path >>>' "$RC"; then
    case "$SHELL_NAME" in
      fish)
        cat >> "$RC" <<__PATH__
# >>> termdash path >>>
fish_add_path -g "$BINDIR"
# <<< termdash path <<<
__PATH__
        ;;
      *)
        cat >> "$RC" <<__PATH__

# >>> termdash path >>>
case ":\$PATH:" in
  *":$BINDIR:"*) ;;
  *) export PATH="$BINDIR:\$PATH" ;;
esac
# <<< termdash path <<<
__PATH__
        ;;
    esac
    say "Added $BINDIR to PATH in $RC"
  fi

  # --- autorun
  if ! grep -q '# >>> termdash autorun >>>' "$RC"; then
    ans="${TERMDASH_AUTORUN:-}"
    if [ -z "$ans" ] && [ -t 0 ]; then
      read -r -p "Draw termdash in every new $SHELL_NAME terminal? [y/N] " yn
      case "$yn" in [Yy]*) ans=1 ;; *) ans=0 ;; esac
    fi
    if [ "$ans" = 1 ]; then
      case "$SHELL_NAME" in
        zsh)
          cat >> "$RC" <<'__RUN__'

# >>> termdash autorun >>>
# If you use powerlevel10k, move this block ABOVE its "instant prompt" block.
if [[ -o interactive && -t 1 && -z "$TERMDASH_SHOWN" ]] && command -v termdash >/dev/null; then
  export TERMDASH_SHOWN=1
  termdash
fi
# <<< termdash autorun <<<
__RUN__
          ;;
        bash)
          cat >> "$RC" <<'__RUN__'

# >>> termdash autorun >>>
if [[ $- == *i* && -t 1 && -z "${TERMDASH_SHOWN:-}" ]] && command -v termdash >/dev/null; then
  export TERMDASH_SHOWN=1
  termdash
fi
# <<< termdash autorun <<<
__RUN__
          ;;
        fish)
          cat >> "$RC" <<'__RUN__'
# >>> termdash autorun >>>
if status is-interactive; and isatty stdout; and not set -q TERMDASH_SHOWN; and command -q termdash
    set -gx TERMDASH_SHOWN 1
    termdash
end
# <<< termdash autorun <<<
__RUN__
          ;;
      esac
      say "termdash will draw in every new $SHELL_NAME terminal"
    fi
  fi
  RC_SHOW="$RC"
  case "$RC" in "$HOME"/*) RC_SHOW="~/${RC#"$HOME"/}" ;; esac
  SOURCE_HINT="Open a new terminal, or:  source $RC_SHOW"
else
  SOURCE_HINT="Add $BINDIR to your PATH, then open a new terminal"
fi

# ---------------------------------------------------------------- done
case "${KITTY_WINDOW_ID:+kitty}${GHOSTTY_RESOURCES_DIR:+ghostty}${WEZTERM_EXECUTABLE:+wezterm}" in
  "") IMG_NOTE="This terminal does not show real images, so the logo and album art use block characters.
(kitty, Ghostty and WezTerm draw real images.)" ;;
  *)  IMG_NOTE="This terminal supports real images, so the logo and album art are drawn as pictures." ;;
esac

case "$OS" in
  macos) PLAYER_NOTE="The player panel shows Music and Spotify (it asks them through osascript, so macOS may ask once to allow it)." ;;
  *)     PLAYER_NOTE="The player panel shows any MPRIS player (Spotify, Firefox, mpv, VLC, ...)." ;;
esac

cat <<EOF

Done.

  1. Put a picture at $CFGDIR/logo.png  (png, jpg, webp, gif, bmp)
  2. $SOURCE_HINT
  3. Try it:
       termdash           draw once
       termdash --live    live meters and clocks, q or Ctrl+C quits

Everything is configurable in $CFGDIR/config.toml
$IMG_NOTE
$PLAYER_NOTE
The player panel moves to the right when the window is at least 142 columns wide.
EOF
