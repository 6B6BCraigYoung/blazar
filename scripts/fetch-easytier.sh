#!/usr/bin/env bash
set -euo pipefail

VERSION=2.6.4

sha256_of() {
  case "$1" in
    macos-aarch64) echo 4be1882d1aa36d31c1d6ba0596f2cf8a097e371f8da124212324b2e0f8df7e4b ;;
    macos-x86_64) echo 89fc28a6e6995259d76ce3f11775220e8a21c760e94df91a6a9db30a69b6982e ;;
    linux-x86_64) echo 61b659eaedba658fa66fe47d17e1426cdd77e5d02fa15fed447bb4357c09dfd6 ;;
    linux-aarch64) echo f533ec25a7ea714e09f645615012200278058525795cc3bb690ff011aec1a70f ;;
    windows-x86_64) echo 27af91e270e554709b048bd32327fefd2dfce5062ae1e8701af7550c6f525f84 ;;
    windows-arm64) echo 37023f8a3451c9234b17ee2089a03dc344ce90d803b5b359cb6c46682b0549b4 ;;
  esac
}

target="${1:-}"
if [[ -z "$target" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) target=macos-aarch64 ;;
    Darwin-x86_64) target=macos-x86_64 ;;
    Linux-x86_64) target=linux-x86_64 ;;
    Linux-aarch64) target=linux-aarch64 ;;
    *) echo "无法识别本机平台，请显式指定目标" >&2; exit 1 ;;
  esac
fi
want="$(sha256_of "$target")"
[[ -n "$want" ]] || { echo "未知目标 $target" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/apps/desktop/engine"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

zip="easytier-$target-v$VERSION.zip"
echo "下载 $zip ..."
curl -fsSL --retry 3 -o "$work/$zip" \
  "https://github.com/EasyTier/EasyTier/releases/download/v$VERSION/$zip"

got="$(shasum -a 256 "$work/$zip" | cut -d' ' -f1)"
if [[ "$got" != "$want" ]]; then
  echo "SHA-256 不符！期望 $want，实际 $got —— 拒绝使用" >&2
  exit 1
fi

unzip -q "$work/$zip" -d "$work/x"
src="$(dirname "$(find "$work/x" -type f \( -name easytier-core -o -name easytier-core.exe \) | head -1)")"
rm -rf "$dest" && mkdir -p "$dest"
for f in "$src"/*; do
  case "$(basename "$f")" in
    easytier-core|easytier-cli|easytier-core.exe|easytier-cli.exe|*.dll) cp "$f" "$dest/" ;;
  esac
done
chmod 755 "$dest"/easytier-* 2>/dev/null || true
cp "$root/scripts/EASYTIER-LICENSE.txt" "$dest/LICENSE.txt" 2>/dev/null || true
echo "$VERSION $target $want" > "$dest/VERSION"
echo "引擎就位：$dest"
ls -l "$dest"
