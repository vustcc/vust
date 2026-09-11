#!/usr/bin/env bash

set -euo pipefail

fail() {
  echo "Package failed: $*" >&2
  exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || echo 0)}"
BUILD_DATE="@$EPOCH"
PUBLISHED_AT="$(TZ=UTC git log -1 --date=format-local:'%Y-%m-%dT%H:%M:%SZ' --format=%cd 2>/dev/null || echo "1970-01-01T00:00:00Z")"
TARGET="x86_64-unknown-linux-musl"
RUST_TARGET="${VUST_RUST_TARGET:-$TARGET}"
TARGET_TRIPLE="${VUST_TARGET_TRIPLE:-linux-x86_64}"
BUILD_PROFILE="${VUST_BUILD_PROFILE:-release}"
BIN_DIR="$ROOT_DIR/target/$RUST_TARGET/$BUILD_PROFILE"
OUT_FILE="$ROOT_DIR/target/vust-bundle.tar.gz"

[[ -x "$BIN_DIR/vust" ]] || fail "missing binary: $BIN_DIR/vust"
[[ -x "$BIN_DIR/vust-agent" ]] || fail "missing binary: $BIN_DIR/vust-agent"

VUST_VERSION=""
VUST_TOML="$ROOT_DIR/Cargo.toml"
if [[ -f "$VUST_TOML" ]]; then
  VUST_VERSION="$(grep -E '^version[[:space:]]*=' "$VUST_TOML" | head -n 1 | awk -F '\"' '{print $2}')"
fi
if [[ -z "$VUST_VERSION" ]]; then
  fail "failed to read vust version"
fi

BUNDLE_DIR_NAME="vust-$VUST_VERSION"
[[ -x "$ROOT_DIR/deploy/install.sh" ]] || fail "missing deploy/install.sh"

STAGING_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$STAGING_DIR"
}
trap cleanup EXIT

mkdir -p "$STAGING_DIR/$BUNDLE_DIR_NAME/templates"

cp "$ROOT_DIR/deploy/install.sh" "$STAGING_DIR/$BUNDLE_DIR_NAME/"

if [[ -x "$BIN_DIR/vustctl" ]]; then
  cp "$BIN_DIR/vustctl" "$STAGING_DIR/$BUNDLE_DIR_NAME/vustctl"
elif [[ -x "$ROOT_DIR/deploy/vustctl" ]]; then
  cp "$ROOT_DIR/deploy/vustctl" "$STAGING_DIR/$BUNDLE_DIR_NAME/"
fi

TEMPLATE_SRC="$ROOT_DIR/deploy/templates"
[[ -d "$TEMPLATE_SRC" ]] || fail "missing templates dir: $TEMPLATE_SRC"
cp -R "$TEMPLATE_SRC/." "$STAGING_DIR/$BUNDLE_DIR_NAME/templates/"

mkdir -p "$(dirname "$OUT_FILE")"

sign_file() {
  local file="$1"
  local sig="$file.sig"

  local key_source="${VUST_SIGNING_PRIVATE_KEY:-}"
  if [[ -z "$key_source" && "$BUILD_PROFILE" == "fast-release" ]]; then
    echo "Skipping signature for $file in fast-release mode (no key provided)"
    return 0
  fi

  command -v minisign &>/dev/null || fail "minisign is required for signing, but it is not installed."
  [[ -n "$key_source" ]] || fail "VUST_SIGNING_PRIVATE_KEY is required"

  # Expand leading tilde ~ to $HOME
  local resolved_source="${key_source/#\~/$HOME}"
  local key_file=""
  local temp_key=""

  if [[ -f "$resolved_source" ]]; then
    key_file="$resolved_source"
  elif [[ "$key_source" =~ ^untrusted[[:space:]]+comment:[[:space:]]+minisign[[:space:]]+(encrypted[[:space:]]+)?secret[[:space:]]+key ]]; then
    temp_key="$(mktemp)"
    printf '%s\n' "$key_source" > "$temp_key"
    key_file="$temp_key"
  else
    fail "VUST_SIGNING_PRIVATE_KEY is neither a valid file path nor a valid private key content."
  fi

  local err_msg
  if ! err_msg=$(printf '%s\n' "${VUST_SIGNING_PRIVATE_KEY_PASSWORD:-}" | \
    minisign -S -s "$key_file" -m "$file" -x "$sig" -q 2>&1); then
    fail "minisign signing failed: $err_msg"
  fi

  if [[ -n "$temp_key" ]]; then
    rm -f "$temp_key"
  fi
}

sha_file() {
  local file="$1"
  local name
  name="$(basename "$file")"
  sha256sum "$file" | awk -v n="$name" '{print $1 "  " n}' > "$file.sha256"
}

COMPONENT_DIR="$STAGING_DIR/$BUNDLE_DIR_NAME/components"
mkdir -p "$COMPONENT_DIR/controller" "$COMPONENT_DIR/agent"
cp "$BIN_DIR/vust" "$COMPONENT_DIR/controller/vust"
cp "$BIN_DIR/vust-agent" "$COMPONENT_DIR/agent/vust-agent"

CONTROLLER_PACKAGE="$STAGING_DIR/$BUNDLE_DIR_NAME/vust-${TARGET_TRIPLE}.tar.gz"
AGENT_PACKAGE="$STAGING_DIR/$BUNDLE_DIR_NAME/vust-agent-${TARGET_TRIPLE}.tar.gz"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="$BUILD_DATE" \
  -C "$COMPONENT_DIR/controller" -czf "$CONTROLLER_PACKAGE" vust
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="$BUILD_DATE" \
  -C "$COMPONENT_DIR/agent" -czf "$AGENT_PACKAGE" vust-agent
sha_file "$CONTROLLER_PACKAGE"
sha_file "$AGENT_PACKAGE"
sign_file "$CONTROLLER_PACKAGE"
sign_file "$AGENT_PACKAGE"
rm -rf "$COMPONENT_DIR"

# Auto derive release channel
CHANNEL="stable"
if [[ "$VUST_VERSION" == *-* ]]; then
  CHANNEL="prerelease"
fi
RELEASE_CHANNEL="${VUST_RELEASE_CHANNEL:-$CHANNEL}"
COMPATIBILITY_FILE="$ROOT_DIR/release-compatibility.json"
SUPPORTED_SUITE_CONTRACT_VERSIONS="$(sed -n 's/.*"supportedSuiteContractVersions"[[:space:]]*:[[:space:]]*\(\[[^]]*\]\).*/\1/p' "$COMPATIBILITY_FILE")"
if [[ -z "$SUPPORTED_SUITE_CONTRACT_VERSIONS" ]]; then
  echo "invalid release compatibility config: $COMPATIBILITY_FILE" >&2
  exit 1
fi

cat <<EOF > "$STAGING_DIR/$BUNDLE_DIR_NAME/release.json"
{
  "version": "${VUST_VERSION}",
  "channel": "${RELEASE_CHANNEL}",
  "targetTriple": "${TARGET_TRIPLE}",
  "publishedAt": "${PUBLISHED_AT}",
  "compatibility": {
    "supportedSuiteContractVersions": ${SUPPORTED_SUITE_CONTRACT_VERSIONS}
  }
}
EOF
sign_file "$STAGING_DIR/$BUNDLE_DIR_NAME/release.json"

OUT_FILE="$(dirname "$OUT_FILE")/vust-${VUST_VERSION}-${TARGET_TRIPLE}.tar.gz"
tar \
  --sort=name \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mtime="$BUILD_DATE" \
  -C "$STAGING_DIR" \
  -czf "$OUT_FILE" \
  "$BUNDLE_DIR_NAME"

echo "package done: $OUT_FILE"
