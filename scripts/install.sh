#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Source cargo env if not already in PATH
if ! command -v cargo &>/dev/null; then
    if [[ -f "$HOME/.cargo/env" ]]; then
        source "$HOME/.cargo/env"
    else
        echo "ERROR: cargo not found. Install Rust first: https://rustup.rs"
        exit 1
    fi
fi

echo "Building fastgrep..."
cd "$PROJECT_ROOT"
cargo build --release -p fastgrep-cli -p fastgrep-bench

# Install binaries (remove first to handle "Text file busy")
INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "$INSTALL_DIR"
rm -f "$INSTALL_DIR/fastgrep" "$INSTALL_DIR/fastgrep-bench"
cp target/release/fastgrep "$INSTALL_DIR/"
cp target/release/fastgrep-bench "$INSTALL_DIR/"
echo "Installed fastgrep to $INSTALL_DIR/fastgrep"
echo "Installed fastgrep-bench to $INSTALL_DIR/fastgrep-bench"

# Install skill
SKILL_DIR="${HOME}/.claude/skills"
mkdir -p "$SKILL_DIR"
cp skill/fastgrep.md "$SKILL_DIR/"
echo "Installed skill to $SKILL_DIR/fastgrep.md"

# Check PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "WARNING: $INSTALL_DIR is not in your PATH."
    echo "Add this to your shell profile:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi

echo ""
echo "Done! Run 'fastgrep --help' to get started."
