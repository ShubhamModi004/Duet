#!/usr/bin/env bash
# Run Duet from source on macOS. Installs Rust if needed.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Installing Xcode Command Line Tools (re-run this script when it finishes)…"
  xcode-select --install
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi
if ! command -v node >/dev/null 2>&1; then
  echo "Node.js is needed for the Tauri CLI: brew install node" >&2
  exit 1
fi

cd app/src-tauri
case "${1:-dev}" in
  dev) npx --yes @tauri-apps/cli@2 dev ;;
  build) npx --yes @tauri-apps/cli@2 build && open ../../target/release/bundle/dmg ;;
  *) echo "usage: $0 [dev|build]" >&2; exit 2 ;;
esac
