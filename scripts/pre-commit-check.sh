#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

echo "[pre-commit] backend checks..."

cargo clippy --all-targets --all-features -- -D warnings > /dev/null
cargo fmt > /dev/null

echo "[pre-commit] frontend checks..."

pnpm -C frontend lint > /dev/null
pnpm -C frontend type-check > /dev/null
pnpm -C frontend format > /dev/null

echo "[pre-commit] all checks passed."
