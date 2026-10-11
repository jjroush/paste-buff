#!/bin/sh
# Install the paste-buff binary from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/jjroush/paste-buff/main/scripts/install.sh | sh
#
# Environment:
#   PASTE_BUFF_VERSION  tag to install (default: latest), e.g. v0.1.0
#   PASTE_BUFF_BIN_DIR  where to put the binary (default: ~/.local/bin)
#   GITHUB_TOKEN        needed only while the repo is private
set -eu

REPO="${PASTE_BUFF_REPO:-jjroush/paste-buff}"
VERSION="${PASTE_BUFF_VERSION:-latest}"
BIN_DIR="${PASTE_BUFF_BIN_DIR:-$HOME/.local/bin}"
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

[ "$(uname -s)" = "Darwin" ] || { echo "paste-buff only runs on macOS" >&2; exit 1; }
case "$(uname -m)" in
  arm64 | aarch64) TARGET=aarch64-apple-darwin ;;
  x86_64) TARGET=x86_64-apple-darwin ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

ASSET="paste-buff-$TARGET.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fetch() {
  # $1 = URL, $2 = output path, $3 = Accept header
  if [ -n "$TOKEN" ]; then
    curl -fsSL -H "Authorization: Bearer $TOKEN" -H "Accept: $3" -o "$2" "$1"
  else
    curl -fsSL -H "Accept: $3" -o "$2" "$1"
  fi
}

if [ -n "$TOKEN" ]; then
  # Private repo: resolve asset ids through the API and download as octet-stream.
  if [ "$VERSION" = "latest" ]; then
    api="https://api.github.com/repos/$REPO/releases/latest"
  else
    api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
  fi
  fetch "$api" "$TMP/release.json" "application/vnd.github+json"
  asset_url() {
    tr -d '\n' < "$TMP/release.json" |
      grep -o "\"url\": *\"[^\"]*/assets/[0-9]*\"[^}]*\"name\": *\"$1\"" |
      head -1 | grep -o 'https://[^"]*/assets/[0-9]*'
  }
  tar_url="$(asset_url "$ASSET")"
  sums_url="$(asset_url SHA256SUMS)"
  [ -n "$tar_url" ] || { echo "no asset $ASSET in release $VERSION" >&2; exit 1; }
  fetch "$tar_url" "$TMP/$ASSET" "application/octet-stream"
  fetch "$sums_url" "$TMP/SHA256SUMS" "application/octet-stream"
else
  if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/$VERSION"
  fi
  fetch "$base/$ASSET" "$TMP/$ASSET" "application/octet-stream"
  fetch "$base/SHA256SUMS" "$TMP/SHA256SUMS" "application/octet-stream"
fi

( cd "$TMP" && grep " $ASSET\$" SHA256SUMS | shasum -a 256 -c - >/dev/null ) ||
  { echo "checksum mismatch for $ASSET" >&2; exit 1; }

tar -xzf "$TMP/$ASSET" -C "$TMP"
mkdir -p "$BIN_DIR"
install -m 755 "$TMP/paste-buff-$TARGET/paste-buff" "$BIN_DIR/paste-buff"

echo "installed $BIN_DIR/paste-buff ($("$BIN_DIR/paste-buff" --version 2>/dev/null || echo "$VERSION"))"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: $BIN_DIR is not on your PATH" ;;
esac
echo "run it with: paste-buff &   (then press ⌘⇧V)"
