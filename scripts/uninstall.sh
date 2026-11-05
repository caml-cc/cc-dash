#!/usr/bin/env bash
# cc-dash uninstaller. Undoes what scripts/install.sh did (for cc-dash and for
# the older termdash installs):
#
#   curl -fsSLO https://raw.githubusercontent.com/caml-cc/cc-dash/main/scripts/uninstall.sh
#   bash uninstall.sh
#
# It removes the binary, the shell rc blocks, the caches and, on macOS, the
# "cc-dash Output" Multi-Output Device. Your config (and logo) is only removed
# if you say so. Packages the installer added (cava, Rust, BlackHole) are left
# alone, since other programs may use them.
#
# Env switches:
#   PREFIX=~/.local     where the binary was installed (default ~/.local)
#   CC_DASH_PURGE=1     also delete the config directory without asking (0 = keep it)
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
CFG_BASE="${XDG_CONFIG_HOME:-$HOME/.config}"
CACHE_BASE="${XDG_CACHE_HOME:-$HOME/.cache}"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }

# Remove a "# >>> tag >>>" ... "# <<< tag <<<" block (and the blank line before
# it) from a file. Returns 1 when the block was not there.
strip_block() {
  local f="$1" tag="$2" tmp
  [ -f "$f" ] && grep -qF "# >>> $tag >>>" "$f" || return 1
  tmp="$(mktemp)"
  awk -v s="# >>> $tag >>>" -v e="# <<< $tag <<<" '
    $0 == s { skip = 1; blank = 0; next }
    skip    { if ($0 == e) skip = 0; next }
    blank   { print ""; blank = 0 }
    $0 == "" { blank = 1; next }
    { print }
    END { if (blank) print "" }
  ' "$f" > "$tmp" && cat "$tmp" > "$f"
  rm -f "$tmp"
}

# ---------------------------------------------------------------- binary
for name in cc-dash termdash; do
  if [ -e "$BINDIR/$name" ]; then
    rm -f "$BINDIR/$name"
    say "Removed $BINDIR/$name"
  fi
done

# ---------------------------------------------------------------- shell rc
# Look at every rc file the installer could have written, not only the one for
# the current $SHELL.
for rc in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile"; do
  for name in cc-dash termdash; do
    if strip_block "$rc" "$name autorun"; then say "Removed $name autorun from $rc"; fi
  done
done

# The PATH block only goes when nothing else lives in $BINDIR, so other tools
# installed there stay on your PATH.
if [ ! -d "$BINDIR" ] || [ -z "$(ls -A "$BINDIR" 2>/dev/null)" ]; then
  for rc in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile"; do
    for name in cc-dash termdash; do
      if strip_block "$rc" "$name path"; then say "Removed $BINDIR from PATH in $rc"; fi
    done
  done
  KEEP_PATH=""
else
  KEEP_PATH=1
fi

# fish: the installer wrote a conf.d file of its own. Keep only its PATH block
# when $BINDIR is still in use.
for name in cc-dash termdash; do
  f="$CFG_BASE/fish/conf.d/$name.fish"
  [ -f "$f" ] || continue
  if [ -n "$KEEP_PATH" ]; then
    strip_block "$f" "$name autorun" && say "Removed $name autorun from $f"
  else
    rm -f "$f"
    say "Removed $f"
  fi
done

# ---------------------------------------------------------------- caches
for name in cc-dash termdash; do
  if [ -d "$CACHE_BASE/$name" ]; then
    rm -rf "${CACHE_BASE:?}/$name"
    say "Removed $CACHE_BASE/$name"
  fi
done

# ---------------------------------------------------------------- macOS audio
# Remove the Multi-Output Device the BlackHole fallback made, if there is one.
remove_macos_device() {
  [ "$(uname -s)" = Darwin ] || return 0
  command -v swift >/dev/null || return 0
  local f rc=0
  f="$(mktemp -d)/audio.swift"
  cat > "$f" <<'__SWIFT__'
import CoreAudio
import Foundation

let uids = ["com.cc-dash.output", "com.termdash.output"]

func prop(_ s: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(mSelector: s, mScope: kAudioObjectPropertyScopeGlobal,
                               mElement: kAudioObjectPropertyElementMain)
}

func uid(_ d: AudioDeviceID) -> String? {
    var a = prop(kAudioDevicePropertyDeviceUID)
    var v: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(d, &a, 0, nil, &size, &v) == 0, let v else { return nil }
    return v.takeRetainedValue() as String
}

var a = prop(kAudioHardwarePropertyDevices)
var size: UInt32 = 0
AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size)
var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &ids)

var removed = 0
for d in ids where uids.contains(uid(d) ?? "") {
    if AudioHardwareDestroyAggregateDevice(d) == 0 { removed += 1 }
}
exit(removed > 0 ? 0 : 10)
__SWIFT__
  swift "$f" >/dev/null 2>&1 || rc=$?
  rm -rf "$(dirname "$f")"
  case "$rc" in
    0)  say "Removed the cc-dash Multi-Output Device" ;;
    10) ;;  # there was none
    *)  warn "could not remove the cc-dash Multi-Output Device; delete it in Audio MIDI Setup" ;;
  esac
}
remove_macos_device

# ---------------------------------------------------------------- config
for name in cc-dash termdash; do
  dir="$CFG_BASE/$name"
  [ -d "$dir" ] || continue
  ans="${CC_DASH_PURGE:-}"
  if [ -z "$ans" ] && [ -t 0 ]; then
    read -r -p "Delete your config and logo in $dir? [y/N] " yn
    case "$yn" in [Yy]*) ans=1 ;; *) ans=0 ;; esac
  fi
  if [ "$ans" = 1 ]; then
    rm -rf "$dir"
    say "Removed $dir"
  else
    say "Kept $dir"
  fi
done

cat <<EOF

cc-dash is uninstalled. Open a new terminal for the rc changes to take effect.
cava, Rust and BlackHole (if the installer added them) are still installed;
remove them with your package manager if you no longer want them.
EOF
