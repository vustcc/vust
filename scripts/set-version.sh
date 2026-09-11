#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 <version>" >&2
  echo "Example: $0 0.1.0-alpha.1" >&2
  exit 1
fi

VERSION="$1"
SEMVER_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
if [[ ! "$VERSION" =~ $SEMVER_PATTERN ]]; then
  echo "Invalid SemVer version: $VERSION" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
CARGO_FILE="$REPOSITORY_ROOT/Cargo.toml"
FRONTEND_PACKAGE_FILE="$REPOSITORY_ROOT/frontend/package.json"

for version_file in "$CARGO_FILE" "$FRONTEND_PACKAGE_FILE"; do
  if [ ! -f "$version_file" ]; then
    echo "Required version file not found: $version_file" >&2
    exit 1
  fi
done

CARGO_VERSION_COUNT="$(awk '
  /^\[workspace\.package\][[:space:]]*$/ { in_workspace_package = 1; next }
  /^\[/ { in_workspace_package = 0 }
  in_workspace_package && /^[[:space:]]*version[[:space:]]*=/ { count += 1 }
  END { print count + 0 }
' "$CARGO_FILE")"
if [ "$CARGO_VERSION_COUNT" -ne 1 ]; then
  echo "Expected exactly one workspace.package version in Cargo.toml, found: $CARGO_VERSION_COUNT" >&2
  exit 1
fi

FRONTEND_VERSION_COUNT="$(grep -Ec '^  "version": "[^"]+",$' "$FRONTEND_PACKAGE_FILE" || true)"
if [ "$FRONTEND_VERSION_COUNT" -ne 1 ]; then
  echo "Expected exactly one top-level version in frontend/package.json, found: $FRONTEND_VERSION_COUNT" >&2
  exit 1
fi

TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

awk -v new_version="$VERSION" '
  /^\[workspace\.package\][[:space:]]*$/ { in_workspace_package = 1 }
  in_workspace_package && /^[[:space:]]*version[[:space:]]*=/ {
    sub(/version[[:space:]]*=[[:space:]]*"[^"]+"/, "version = \"" new_version "\"")
  }
  in_workspace_package && /^\[/ && !/^\[workspace\.package\][[:space:]]*$/ {
    in_workspace_package = 0
  }
  { print }
' "$CARGO_FILE" > "$TEMP_DIR/Cargo.toml"

sed -E \
  "s/^  \"version\": \"[^\"]+\",$/  \"version\": \"$VERSION\",/" \
  "$FRONTEND_PACKAGE_FILE" > "$TEMP_DIR/frontend-package.json"

cp "$TEMP_DIR/Cargo.toml" "$CARGO_FILE"
cp "$TEMP_DIR/frontend-package.json" "$FRONTEND_PACKAGE_FILE"

echo "Updated Cargo.toml workspace version to $VERSION"
echo "Updated frontend/package.json version to $VERSION"
