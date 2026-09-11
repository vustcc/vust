#!/usr/bin/env bash

set -euo pipefail

fail() {
  echo "Build failed: $*" >&2
  exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
RUST_TARGET="${VUST_RUST_TARGET:-x86_64-unknown-linux-musl}"
TARGET_TRIPLE="${VUST_TARGET_TRIPLE:-linux-x86_64}"

PROFILE="release"
while [[ $# -gt 0 ]]; do
  case "$1" in
    -d|--dev)
      PROFILE="fast-release"
      shift
      ;;
    *)
      fail "Unknown argument: $1"
      ;;
  esac
done

SOURCE_DATE_EPOCH="$(git log -1 --format=%ct 2>/dev/null || echo 0)"
export SOURCE_DATE_EPOCH

cd "$ROOT_DIR"

command -v pnpm >/dev/null 2>&1 || fail "pnpm not found"
command -v cargo >/dev/null 2>&1 || fail "cargo not found"

if [[ "$PROFILE" == "release" ]]; then
  echo "==> 1/5 Frontend type-check"
  pnpm -C frontend type-check

  echo "==> 2/5 Build frontend"
  pnpm -C frontend build

  echo "==> 3/5 Rust code check (clippy, warnings as errors)"
  cargo clippy --all-targets --all-features -- -D warnings
else
  echo "==> Skipping Frontend type-check and Rust clippy in dev mode"
  echo "==> 1/3 Build frontend"
  pnpm -C frontend build
fi

if [[ "$PROFILE" == "release" ]]; then
  echo "==> 4/5 Build backend (release, rustTarget=$RUST_TARGET, targetTriple=$TARGET_TRIPLE)"
  cargo build --release --target "$RUST_TARGET"
else
  echo "==> 2/3 Build backend ($PROFILE, rustTarget=$RUST_TARGET, targetTriple=$TARGET_TRIPLE)"
  cargo build --profile "$PROFILE" --target "$RUST_TARGET"
fi

if [[ "$PROFILE" == "release" ]]; then
  echo "==> 5/5 Package release"
else
  echo "==> 3/3 Package release"
fi
VUST_BUILD_PROFILE="$PROFILE" VUST_RUST_TARGET="$RUST_TARGET" VUST_TARGET_TRIPLE="$TARGET_TRIPLE" "$SCRIPT_DIR/package.sh"

echo "Pipeline completed"
