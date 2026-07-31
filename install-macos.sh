#!/usr/bin/env bash
# Install the prebuilt claude-watch binary on macOS (Apple Silicon) —
# no Rust required, and clears the quarantine/Gatekeeper friction.
set -euo pipefail

REPO="rob-mcgrail/claude-watch"
ASSET="claude-watch-macos-arm64"
DEST="$HOME/.local/bin"
BIN="$DEST/claude-watch"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
    echo "the prebuilt binary is macOS Apple Silicon only." >&2
    echo "build from source instead: git clone https://github.com/$REPO && cd claude-watch && ./install.sh" >&2
    exit 1
fi

mkdir -p "$DEST"
echo "downloading latest ${ASSET}..."
curl -fsSL "https://github.com/$REPO/releases/latest/download/$ASSET" -o "$BIN"
chmod +x "$BIN"

# strip the "downloaded from the internet" quarantine flag and re-sign
# ad-hoc so Gatekeeper has nothing to complain about
xattr -d com.apple.quarantine "$BIN" 2>/dev/null || true
xattr -c "$BIN" 2>/dev/null || true
codesign --force -s - "$BIN" 2>/dev/null || true

LINE='export PATH="$HOME/.local/bin:$PATH"'
case ":$PATH:" in
  *":$DEST:"*) ;;
  *)
    if ! grep -qsF "$LINE" "$HOME/.zshrc"; then
        printf '\n# claude-watch\n%s\n' "$LINE" >> "$HOME/.zshrc"
        echo "added $DEST to PATH in ~/.zshrc — restart your shell (or run: exec zsh)"
    fi
    ;;
esac

echo
echo "installed: $BIN"
echo "run 'claude-watch' in any folder with a live Claude Code session"
