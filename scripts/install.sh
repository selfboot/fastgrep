#!/usr/bin/env bash
# One-line install for fastgrep as a Claude Code skill:
#
#   git clone https://github.com/user/fastgrep && cd fastgrep && bash install.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Support being called from project root (install.sh) or scripts/ (scripts/install.sh)
if [[ -f "$SCRIPT_DIR/Cargo.toml" ]]; then
    PROJECT_ROOT="$SCRIPT_DIR"
else
    PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
fi

# --- 1. Build binary ---
if ! command -v cargo &>/dev/null; then
    if [[ -f "$HOME/.cargo/env" ]]; then
        source "$HOME/.cargo/env"
    else
        echo "ERROR: cargo not found. Install Rust first: https://rustup.rs"
        exit 1
    fi
fi

echo "==> Building fastgrep..."
cd "$PROJECT_ROOT"
cargo build --release -p fastgrep-cli -p fastgrep-bench

# --- 2. Install binaries ---
INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "$INSTALL_DIR"
rm -f "$INSTALL_DIR/fastgrep" "$INSTALL_DIR/fastgrep-bench"
cp target/release/fastgrep "$INSTALL_DIR/"
cp target/release/fastgrep-bench "$INSTALL_DIR/"
echo "    Installed fastgrep       -> $INSTALL_DIR/fastgrep"
echo "    Installed fastgrep-bench -> $INSTALL_DIR/fastgrep-bench"

# --- 3. Install Claude Code skill ---
SKILL_DIR="${HOME}/.claude/skills/fastgrep"
mkdir -p "$SKILL_DIR"
cp "$PROJECT_ROOT/.claude/skills/fastgrep/SKILL.md" "$SKILL_DIR/SKILL.md"
echo "    Installed skill           -> $SKILL_DIR/SKILL.md"

# --- 4. PATH check ---
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "WARNING: $INSTALL_DIR is not in your PATH."
    echo "Add this to your shell profile:"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
fi

echo ""
echo "Done! Claude Code can now use /fastgrep in any project."
echo "Run 'fastgrep --help' to verify."
