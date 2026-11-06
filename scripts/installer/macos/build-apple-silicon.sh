#!/usr/bin/env bash
# 构建 macOS Apple Silicon（arm64）安装包。
set -euo pipefail

VERSION="${1:-0.0.0}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TARGET="aarch64-apple-darwin"
BINARY_DIR="$ROOT/target/$TARGET/release"
BINARIES=(codex-plus-plus codex-plus-legacy-shim)

# 前端产物要由调用方先构建好（vite:build），统一应用会把 dist 嵌进二进制。
if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "error: 缺少 Rust target $TARGET，请先运行：" >&2
  echo "       rustup target add $TARGET" >&2
  exit 1
fi

cd "$ROOT"
echo "==> 构建 $TARGET" >&2
cargo build --release --target "$TARGET"

for bin in "${BINARIES[@]}"; do
  path="$BINARY_DIR/$bin"
  if [ ! -x "$path" ]; then
    echo "error: 缺少构建产物 $path" >&2
    exit 1
  fi

  archs="$(lipo -archs "$path")"
  if [ "$archs" != "arm64" ]; then
    echo "error: $bin 必须仅包含 arm64（实际架构：$archs）" >&2
    exit 1
  fi
  echo "    $bin -> $archs" >&2
done

BINARY_DIR="$BINARY_DIR" bash "$ROOT/scripts/installer/macos/package-dmg.sh" "$VERSION" arm64
