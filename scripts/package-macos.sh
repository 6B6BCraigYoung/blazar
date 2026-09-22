#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../apps/desktop"
export PATH="$HOME/.cargo/bin:$PATH"

if [[ "${1:-}" == "--icons" ]]; then
  rsvg-convert -w 1024 -h 1024 icons/src/blazar-brand.svg -o icons/icon.png
  cargo tauri icon icons/icon.png -o icons >/dev/null
  rm -rf icons/android icons/ios icons/Square*Logo.png icons/StoreLogo.png
fi

if [[ ! -x engine/easytier-core ]]; then
  ../../scripts/fetch-easytier.sh
fi

cargo build --bins --release --features tauri/custom-protocol
cargo tauri bundle --bundles app,dmg
echo
ls -d ../../target/release/bundle/macos/*.app ../../target/release/bundle/dmg/*.dmg
