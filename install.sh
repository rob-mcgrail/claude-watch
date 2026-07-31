#!/usr/bin/env bash
# Build claude-watch in release mode, install it, and make sure it's on PATH.
set -euo pipefail

cd "$(dirname "$0")"
cargo install --path . --locked 2>/dev/null || cargo install --path .

CARGO_BIN="$HOME/.cargo/bin"
if ! command -v claude-watch >/dev/null 2>&1; then
    LINE='export PATH="$HOME/.cargo/bin:$PATH"'
    if ! grep -qsF "$LINE" "$HOME/.zshrc"; then
        printf '\n# cargo binaries (added by claude-watch install.sh)\n%s\n' "$LINE" >> "$HOME/.zshrc"
        echo "added $CARGO_BIN to PATH in ~/.zshrc"
    fi
    echo
    echo "installed: $CARGO_BIN/claude-watch"
    echo "restart your shell (or run: exec zsh) then run 'claude-watch'"
else
    echo
    echo "installed: $(command -v claude-watch)"
    echo "run 'claude-watch' in any folder with a live Claude session"
fi
