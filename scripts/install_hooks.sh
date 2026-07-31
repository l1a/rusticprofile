#!/usr/bin/env bash
# Install git hooks for this repo. Run once after cloning.
# Usage: bash scripts/install_hooks.sh
#
# Git hooks are the enforcement layer here, not agent config: they fire the same way
# whether a human, Claude Code, Gemini or any other tool is driving. See AGENTS.md
# Part 1 section 4.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_DIR="$REPO_ROOT/.git/hooks"

install_hook() {
    local name="$1"
    local src="$REPO_ROOT/scripts/hooks/$name"
    local dst="$HOOKS_DIR/$name"

    if [ ! -f "$src" ]; then
        echo "ERROR: hook source not found: $src" >&2
        exit 1
    fi

    cp "$src" "$dst"
    chmod +x "$dst"
    echo "Installed: .git/hooks/$name"
}

install_hook pre-push

echo "All hooks installed."
