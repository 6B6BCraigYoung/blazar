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

(cd ../../crates/web && trunk build --release)
cargo build --bins --release --features tauri/custom-protocol
cargo tauri bundle --bundles "${BLAZAR_BUNDLES:-app,dmg}"
../../scripts/local-signing.sh sign ../../target/release/bundle/macos/Blazar.app
OUT=../../target/bundle.noindex
mkdir -p "$OUT"
rm -rf "$OUT/Blazar.app"
mv ../../target/release/bundle/macos/Blazar.app "$OUT/Blazar.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
  -u ../../target/release/bundle/macos/Blazar.app 2>/dev/null || true
echo
ls -d "$OUT"/*.app ../../target/release/bundle/dmg/*.dmg 2>/dev/null || true
