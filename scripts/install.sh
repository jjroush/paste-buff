#!/bin/sh
# Install the paste-buff binary from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/jjroush/paste-buff/main/scripts/install.sh | sh
#
# Environment:
#   PASTE_BUFF_VERSION  tag to install (default: latest), e.g. v0.1.0
#   PASTE_BUFF_BIN_DIR  where to put the binary (default: ~/.local/bin)
#   GITHUB_TOKEN        needed only for a private fork of the repo
#   PASTE_BUFF_NO_AGENT set to skip the launchd agent (just install the binary)
#
# After installing, the script registers a launchd agent so paste-buff starts
# now and at every login, like Raycast. `install.sh --uninstall` reverses it.
set -eu

REPO="${PASTE_BUFF_REPO:-jjroush/paste-buff}"
VERSION="${PASTE_BUFF_VERSION:-latest}"
BIN_DIR="${PASTE_BUFF_BIN_DIR:-$HOME/.local/bin}"
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
LABEL="io.roush.paste-buff"
AGENTS="$HOME/Library/LaunchAgents"
PLIST="$AGENTS/$LABEL.plist"
LOG="$HOME/Library/Logs/paste-buff.log"
DOMAIN="gui/$(id -u)"

stop_agent() {
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
  stop_agent
  rm -f "$PLIST" "$BIN_DIR/paste-buff"
  echo "removed $BIN_DIR/paste-buff and the launchd agent"
  echo "history is still in ~/Library/Application Support/paste-buff; delete it if you want"
  exit 0
fi

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
# Stop a running agent before swapping the binary under it.
stop_agent
install -m 755 "$TMP/paste-buff-$TARGET/paste-buff" "$BIN_DIR/paste-buff"

echo "installed $BIN_DIR/paste-buff ($("$BIN_DIR/paste-buff" --version 2>/dev/null || echo "$VERSION"))"

if [ -n "${PASTE_BUFF_NO_AGENT:-}" ]; then
  echo "run it with: paste-buff &   (then press ⌘⇧V)"
  exit 0
fi

mkdir -p "$AGENTS" "$(dirname "$LOG")"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_DIR/paste-buff</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  <string>$LOG</string>
  <key>StandardErrorPath</key>
  <string>$LOG</string>
</dict>
</plist>
PLIST

launchctl bootstrap "$DOMAIN" "$PLIST"
# RunAtLoad only applies at login; start (or restart) it now.
launchctl kickstart -k "$DOMAIN/$LABEL"
echo "started paste-buff in the background; it will start again at login"
echo "press ⌘⇧V to open it. First time only, macOS asks for Accessibility access so"
echo "it can press ⌘V for you (System Settings > Privacy & Security > Accessibility)."
echo "If Raycast is running, turn off its Clipboard History hotkey, which is also ⌘⇧V."
echo "logs: $LOG    uninstall: install.sh --uninstall"
