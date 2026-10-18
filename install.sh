#!/usr/bin/env bash
# Build Netwatch from this checkout and install it into /Applications.
#
#   ./install.sh
#
# Environment:
#   NETWATCH_INSTALL_DIR  where to put Netwatch.app (default /Applications,
#                         or ~/Applications if that isn't writable)
#   NETWATCH_NO_LAUNCH=1  don't open the app afterwards
set -euo pipefail

APP="Netwatch.app"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[[ "$(uname -s)" == Darwin ]] || die "Netwatch only runs on macOS."
command -v cargo >/dev/null || die "cargo not found. Install Rust from https://rustup.rs and re-run."

# Build the checkout this script lives in, whatever the current directory.
cd "$(dirname "${BASH_SOURCE[0]}")"
[[ -f crates/netwatch-app/Cargo.toml ]] || die "run this from the Netwatch source tree"

if ! command -v cargo-packager >/dev/null; then
  say "Installing cargo-packager"
  cargo install cargo-packager --locked
fi

say "Building $APP (release; the first build takes a few minutes)"
cargo packager --release -p netwatch-app --formats app
built="target/packages/$APP"
[[ -d "$built" ]] || die "build finished but $built is missing"

dest="${NETWATCH_INSTALL_DIR:-/Applications}"
if [[ -z "${NETWATCH_INSTALL_DIR:-}" && ! -w "$dest" ]]; then
  dest="$HOME/Applications"
fi
mkdir -p "$dest"

# Replace a running copy.
if pgrep -xq netwatch; then
  say "Quitting the running Netwatch"
  osascript -e 'quit app "Netwatch"' >/dev/null 2>&1 || pkill -x netwatch || true
  sleep 1
fi

say "Installing to $dest/$APP"
rm -rf "${dest:?}/$APP"
cp -R "$built" "$dest/"
# Clear quarantine in case the source tree was downloaded rather than cloned.
xattr -dr com.apple.quarantine "$dest/$APP" 2>/dev/null || true

if [[ "${NETWATCH_NO_LAUNCH:-}" != 1 ]]; then
  open "$dest/$APP"
  say "Netwatch is running: look for it in the menu bar."
else
  say "Installed. Open $dest/$APP to start it."
fi
