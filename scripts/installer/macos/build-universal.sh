#!/usr/bin/env bash
# 默认构建 Intel 与 Apple Silicon 通用安装包；传入 arm64 只构建 Apple Silicon。
# 用法：build-universal.sh [版本] [universal|arm64]
set -euo pipefail

VERSION="${1:-0.0.0}"
MODE="${2-universal}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
UNIVERSAL_DIR="$ROOT/target/universal/release"
ARM64_TARGET=aarch64-apple-darwin
ARM64_DIR="$ROOT/target/$ARM64_TARGET/release"
BINARIES=(codex-plus-plus codex-plus-legacy-shim)

case "$MODE" in
  universal) targets=(x86_64-apple-darwin "$ARM64_TARGET") ;;
  arm64) targets=("$ARM64_TARGET") ;;
  *)
    echo "error: 未知打包架构 $MODE；可选 universal 或 arm64。" >&2
    exit 1
    ;;
esac

cd "$ROOT"

# UNIVERSAL_SKIP_BUILD=1：所选架构已由上游编好并放进 target/<target>/release/，
# 这里直接打包；universal 模式先合并双架构。
if [ "${UNIVERSAL_SKIP_BUILD:-}" != "1" ]; then
  # 前端产物要由调用方先构建好（vite:build），统一应用会把 dist 嵌进二进制。
  for target in "${targets[@]}"; do
    if ! rustup target list --installed | grep -qx "$target"; then
      echo "error: 缺少 Rust target $target，请先运行：" >&2
      echo "       rustup target add $target" >&2
      exit 1
    fi
  done

  for target in "${targets[@]}"; do
    echo "==> 构建 $target" >&2
    cargo build --release --target "$target"
  done
fi

if [ "$MODE" = arm64 ]; then
  for bin in "${BINARIES[@]}"; do
    path="$ARM64_DIR/$bin"
    if [ ! -x "$path" ]; then
      echo "error: 缺少构建产物 $path" >&2
      exit 1
    fi
    archs="$(lipo -archs "$path")"
    if [ "$archs" != arm64 ]; then
      echo "error: $bin 不是 arm64 二进制（实际架构：$archs）" >&2
      exit 1
    fi
  done
  BINARY_DIR="$ARM64_DIR" bash "$ROOT/scripts/installer/macos/package-dmg.sh" "$VERSION" arm64
  exit 0
fi

rm -rf "$UNIVERSAL_DIR"
mkdir -p "$UNIVERSAL_DIR"

for bin in "${BINARIES[@]}"; do
  x64="$ROOT/target/x86_64-apple-darwin/release/$bin"
  arm64="$ARM64_DIR/$bin"
  for path in "$x64" "$arm64"; do
    if [ ! -x "$path" ]; then
      echo "error: 缺少构建产物 $path" >&2
      exit 1
    fi
  done

  echo "==> 合并 $bin" >&2
  lipo -create "$x64" "$arm64" -output "$UNIVERSAL_DIR/$bin"
  chmod +x "$UNIVERSAL_DIR/$bin"

  # 断言确实合并成双架构，避免 lipo 静默产出单架构。
  archs="$(lipo -archs "$UNIVERSAL_DIR/$bin")"
  case "$archs" in
    *x86_64*arm64*|*arm64*x86_64*) ;;
    *)
      echo "error: $bin 不是通用二进制（实际架构：$archs）" >&2
      exit 1
      ;;
  esac
  echo "    $bin -> $archs" >&2
done

BINARY_DIR="$UNIVERSAL_DIR" bash "$ROOT/scripts/installer/macos/package-dmg.sh" "$VERSION" universal
