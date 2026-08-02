# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a

# Justfile for rusticprofile
# Run with: just <recipe>

# Required for shebang recipes to receive *ARGS as real argv ($@) instead of
# losing quoting via textual {{ARGS}} interpolation (see open-pr).
set positional-arguments := true

BASH_COMP  := `echo "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"`
ZSH_COMP   := `echo "${XDG_DATA_HOME:-$HOME/.local/share}/zsh/site-functions"`
FISH_COMP  := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"`
ELVISH_COMP := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/elvish/lib"`
NU_COMP    := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/nushell/autoload"`
PS_COMP    := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/powershell"`

# Default recipe
default:
    @just --list

# Build the project (debug mode)
build:
    cargo build

# Build the project (release mode)
build-release:
    cargo build --release

# Run tests
test:
    cargo test

# Clean build artifacts
clean:
    cargo clean

# Format code
fmt:
    cargo fmt

# Run clippy lints
lint:
    cargo clippy -- -D warnings

# Run strict checks (formatting, linting, golden argv files) as done in CI
check: golden-is-current
    cargo fmt -- --check
    cargo clippy -- -D warnings

# Regenerate the golden argv files under tests/golden
golden:
    RP_UPDATE_GOLDEN=1 cargo test --test cli_tests golden_

# Fail if any golden argv file is stale or untracked (regenerate with `just golden`)
golden-is-current:
    #!/usr/bin/env bash
    set -euo pipefail

    # Staleness is checked by content, not by git state: regenerate and compare hashes.
    # Comparing against `git status` instead would fail on goldens that are staged but not
    # yet committed, which is a normal mid-commit state and not a problem.
    hashes() { find tests/golden -type f -name '*.txt' -exec sha256sum {} + 2>/dev/null | sort; }
    before="$(hashes)"
    RP_UPDATE_GOLDEN=1 cargo test --test cli_tests golden_ -q >/dev/null 2>&1
    after="$(hashes)"

    if [ "$before" != "$after" ]; then
        echo "tests/golden is stale — the argv rusticprofile would run has changed." >&2
        echo "Review the diff below; if the change is intended, commit it." >&2
        git --no-pager diff -- tests/golden >&2
        exit 1
    fi

    # A golden nobody added to git is as bad as a stale one: CI would regenerate it from
    # scratch and never notice a change.
    untracked="$(git ls-files --others --exclude-standard -- tests/golden)"
    if [ -n "$untracked" ]; then
        echo "untracked golden files — git add them:" >&2
        echo "$untracked" >&2
        exit 1
    fi

# Run security audit (requires cargo-audit)
audit:
    @command -v cargo-audit >/dev/null || cargo install cargo-audit
    cargo audit

# Install the binary, man page, and shell completions
install: install-man install-completions
    cargo install --path .

# The version is read from Cargo.toml into the .TH footer, so the man page and the
# package can never disagree about which release is being documented.
# Generate the man page from Markdown using mandown
man:
    @mkdir -p docs
    @VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d '"' -f2); \
    DATE=$(date +"%B %Y"); \
    mandown docs/rusticprofile.1.md RUSTICPROFILE 1 | sed -e 's/\\fB\\fB/\\fB/g' -e 's/\\fP\\fP/\\fP/g' -e "s/\\.TH \"RUSTICPROFILE\" 1/\\.TH \"RUSTICPROFILE\" \"1\" \"$DATE\" \"rusticprofile $VERSION\" \"Backup Scheduler\"/" > docs/rusticprofile.1

# Install man page to XDG user location (~/.local/share/man)
install-man: man
    @mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/man/man1"
    install -m 644 docs/rusticprofile.1 "${XDG_DATA_HOME:-$HOME/.local/share}/man/man1/rusticprofile.1"
    @echo "Man page installed to ${XDG_DATA_HOME:-$HOME/.local/share}/man/man1/"

# Install shell completions for all supported shells to XDG user locations
install-completions: build
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "{{BASH_COMP}}" "{{ZSH_COMP}}" "{{FISH_COMP}}" "{{ELVISH_COMP}}" "{{NU_COMP}}" "{{PS_COMP}}"
    BIN="{{justfile_directory()}}/target/debug/rusticprofile"
    "$BIN" --completions bash        > "{{BASH_COMP}}/rusticprofile"
    "$BIN" --completions zsh         > "{{ZSH_COMP}}/_rusticprofile"
    "$BIN" --completions fish        > "{{FISH_COMP}}/rusticprofile.fish"
    "$BIN" --completions elvish      > "{{ELVISH_COMP}}/rusticprofile.elv"
    "$BIN" --completions nushell     > "{{NU_COMP}}/50rusticprofile-completions.nu"
    "$BIN" --completions power-shell > "{{PS_COMP}}/rusticprofile.ps1"
    echo "Installed completions for rusticprofile"
    echo ""
    echo "Notes:"
    echo "  bash       source {{BASH_COMP}}/rusticprofile  (or restart shell)"
    echo "  zsh        auto-loaded from {{ZSH_COMP}}"
    echo "  fish       auto-loaded from {{FISH_COMP}}"
    echo "  elvish     add to rc.elv:  eval (slurp < {{ELVISH_COMP}}/rusticprofile.elv)"
    echo "  nushell    auto-loaded from {{NU_COMP}}"
    echo "  powershell add to \$PROFILE:  . {{PS_COMP}}/rusticprofile.ps1"

# Run criterion micro-benchmarks (none yet — see the NOTES.md backlog)
bench:
    @echo "No benchmarks yet. Add benches/ and the [[bench]] table when config parsing lands."

# Install git hooks (run once after cloning)
install-hooks:
    bash scripts/install_hooks.sh

# One-time repo setup: install git hooks and any other local tooling
setup: install-hooks
    @echo "Repo setup complete."

# Full development setup
dev: setup fmt lint test build
    @echo "Development build complete."

# Dry-run publish check (no upload)
publish-check:
    cargo publish --dry-run

# Publish to crates.io
publish:
    cargo publish

# Build and lint the AUR package in a container — no Arch install needed
aur-verify:
    #!/usr/bin/env bash
    set -euo pipefail
    BOLD='\033[1m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
    pass() { echo -e "${GREEN}[✓]${NC} $1"; }
    fail() { echo -e "${RED}[✗]${NC} $1"; exit 1; }
    info() { echo -e "${YELLOW}[→]${NC} $1"; }

    command -v podman >/dev/null || fail "podman is required (or edit this recipe for docker)"

    # `rustic` is installed in the container ON PURPOSE, and it is the whole reason this
    # recipe exists rather than a one-line makepkg. Without it the build still succeeds,
    # but the rustic-backed integration tests skip themselves with a printed notice — so a
    # green makepkg proves considerably less than it looks like it does. That mistake was
    # made once here already; encoding the fix in a recipe is how it stays fixed.
    info "Building the package in archlinux:base-devel (this compiles the crate)..."
    podman run --rm -v "{{justfile_directory()}}/packaging/aur:/pkg:ro,Z" archlinux:base-devel bash -c '
        set -e
        pacman -Syu --noconfirm --quiet >/dev/null 2>&1
        pacman -S --noconfirm --quiet --needed namcap rust rustic gcc-libs >/dev/null 2>&1
        echo "container has: $(rustic --version)"
        useradd -m builder
        mkdir -p /home/builder/b && cp /pkg/PKGBUILD /home/builder/b/
        chown -R builder /home/builder/b
        cd /home/builder/b
        su builder -c "makepkg --noconfirm" > /tmp/mk.log 2>&1 || { echo "MAKEPKG FAILED"; tail -30 /tmp/mk.log; exit 1; }
        echo "--- tests run by check() ---"
        grep -E "^test result" /tmp/mk.log
        # makepkg also emits a -debug package. Globbing both hands tar a second argument
        # it reads as a member name ("Not found in archive"), and because that sits in a
        # pipeline the failure is swallowed and the recipe reports success anyway. Select
        # the real package explicitly, and check the payload listing outside a pipeline.
        PKGFILE=$(find . -maxdepth 1 -name "*.pkg.tar.zst" ! -name "*-debug-*" | head -1)
        [ -n "$PKGFILE" ] || { echo "no package was produced"; exit 1; }
        echo "--- package payload ($PKGFILE) ---"
        tar tf "$PKGFILE" > /tmp/payload.txt
        grep -v "^\.\|/$" /tmp/payload.txt | sort
        echo "--- namcap PKGBUILD ---"
        namcap PKGBUILD
        echo "--- namcap package ---"
        namcap "$PKGFILE" || true
    ' || fail "container verification failed"
    pass "package builds, tests pass, payload and namcap shown above"
    echo
    echo -e "${BOLD}Two namcap package warnings are expected and correct:${NC}"
    echo "  rustic    — namcap reads linked libraries; rusticprofile *spawns* rustic, so it cannot see this"
    echo "  gcc-libs  — implicitly satisfied via libgcc_s, and the standard Rust dependency on Arch"

# Regenerate packaging/aur/.SRCINFO from the PKGBUILD (never edit it by hand)
aur-srcinfo:
    #!/usr/bin/env bash
    set -euo pipefail
    # The AUR rejects a .SRCINFO that disagrees with its PKGBUILD, and the file is pure
    # derived data — so it is generated, never written.
    podman run --rm -v "{{justfile_directory()}}/packaging/aur:/pkg:ro,Z" archlinux:base-devel bash -c '
        useradd -m builder; mkdir -p /home/builder/b && cp /pkg/PKGBUILD /home/builder/b/
        chown -R builder /home/builder/b; cd /home/builder/b
        su builder -c "makepkg --printsrcinfo"' > "{{justfile_directory()}}/packaging/aur/.SRCINFO"
    echo "packaging/aur/.SRCINFO regenerated"

# Point the PKGBUILD at a released tag: bump pkgver, reset pkgrel, refresh the checksum
aur-bump VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
    fail() { echo -e "${RED}[✗]${NC} $1"; exit 1; }
    V="{{VERSION}}"
    URL="https://github.com/l1a/rusticprofile/archive/refs/tags/v${V}.tar.gz"

    # The tag must exist first: the PKGBUILD builds from the release tarball, so bumping
    # ahead of the release produces a package nobody can build.
    curl -sfIL -o /dev/null "$URL" || fail "no release tarball at $URL — tag and release v$V first"

    SHA=$(curl -sL "$URL" | sha256sum | cut -d' ' -f1)
    P="{{justfile_directory()}}/packaging/aur/PKGBUILD"
    sed -i -e "s/^pkgver=.*/pkgver=${V}/" -e "s/^pkgrel=.*/pkgrel=1/" \
           -e "s/^sha256sums=.*/sha256sums=('${SHA}')/" "$P"
    echo -e "${GREEN}[✓]${NC} pkgver=${V} pkgrel=1 sha256=${SHA}"
    just aur-srcinfo
    echo "Now run: just aur-verify"

# Push packaging/aur to the AUR — verifies the checksum and refuses if the AUR is down
aur-publish:
    #!/usr/bin/env bash
    set -euo pipefail
    BOLD='\033[1m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
    pass() { echo -e "${GREEN}[✓]${NC} $1"; }
    fail() { echo -e "${RED}[✗]${NC} $1"; exit 1; }
    info() { echo -e "${YELLOW}[→]${NC} $1"; }

    DIR="{{justfile_directory()}}/packaging/aur"
    [ -f "$DIR/PKGBUILD" ] && [ -f "$DIR/.SRCINFO" ] || fail "packaging/aur is missing PKGBUILD or .SRCINFO"

    PKGVER=$(sed -n 's/^pkgver=//p' "$DIR/PKGBUILD")
    pass "PKGBUILD pkgver: $PKGVER"

    # .SRCINFO must agree with the PKGBUILD. The AUR rejects a mismatch, and finding that
    # out from a push rejection is a worse way to learn it than finding out here.
    grep -q "pkgver = $PKGVER" "$DIR/.SRCINFO" \
        || fail ".SRCINFO disagrees with PKGBUILD — run: just aur-srcinfo"
    pass ".SRCINFO agrees with PKGBUILD"

    # The classic AUR breakage is a bumped pkgver with a stale checksum, which fails on the
    # user's machine and nowhere else. Check it against the tarball that will actually be
    # downloaded.
    info "Verifying sha256sums against the real tarball..."
    URL="https://github.com/l1a/rusticprofile/archive/refs/tags/v${PKGVER}.tar.gz"
    ACTUAL=$(curl -sL "$URL" | sha256sum | cut -d' ' -f1)
    DECLARED=$(sed -n "s/^sha256sums=('\(.*\)')/\1/p" "$DIR/PKGBUILD")
    [ "$ACTUAL" = "$DECLARED" ] \
        || fail "checksum mismatch for v$PKGVER — declared $DECLARED, actual $ACTUAL. Run: just aur-bump $PKGVER"
    pass "sha256 matches the v$PKGVER tarball"

    # The AUR takes maintenance windows, and during one the SSH endpoint authenticates and
    # then refuses. Reporting that plainly beats a confusing git failure.
    info "Checking the AUR is reachable..."
    OUT=$(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new aur@aur.archlinux.org help 2>&1 || true)
    if echo "$OUT" | grep -qi 'maintenance'; then
        fail "the AUR is in a maintenance window — it said: $(echo "$OUT" | head -1)"
    fi
    echo "$OUT" | grep -qi 'Permission denied' \
        && fail "the AUR refused this SSH key; register it at https://aur.archlinux.org/account/"
    pass "AUR reachable and the SSH key is accepted"

    echo
    echo -e "${BOLD}About to publish rusticprofile $PKGVER to the AUR.${NC}"
    echo "This is public and immediate."
    echo ""
    # Same three-way answer as the pre-PR gate: an env var for non-interactive callers, a
    # terminal for humans, piped input otherwise — and never a block that hangs.
    if [ -n "${AUR_CONFIRM:-}" ]; then
        CONFIRM="$AUR_CONFIRM"
        echo "Publish to the AUR? [y/N] $CONFIRM   (answered by AUR_CONFIRM)"
    elif [ -t 0 ]; then
        echo -n "Publish to the AUR? [y/N] "; read -r CONFIRM
    else
        echo -n "Publish to the AUR? [y/N] "
        read -r -t 10 CONFIRM || CONFIRM=""
        echo "$CONFIRM"
        [ -n "$CONFIRM" ] || fail "no terminal and nothing on stdin. Re-run with AUR_CONFIRM=y"
    fi
    [ "$CONFIRM" = "y" ] || [ "$CONFIRM" = "Y" ] || { echo -e "${RED}Aborted.${NC}"; exit 1; }

    CLONE=$(mktemp -d)
    trap 'rm -rf "$CLONE"' EXIT
    git clone "ssh://aur@aur.archlinux.org/rusticprofile.git" "$CLONE/pkg" 2>&1 | tail -2
    cp "$DIR/PKGBUILD" "$DIR/.SRCINFO" "$CLONE/pkg/"
    cd "$CLONE/pkg"
    if git diff --quiet --exit-code -- PKGBUILD .SRCINFO && [ -z "$(git status --porcelain)" ]; then
        pass "the AUR already matches these files — nothing to push"
        exit 0
    fi
    git add PKGBUILD .SRCINFO
    git -c user.name="$(git -C {{justfile_directory()}} config user.name)" \
        -c user.email="$(git -C {{justfile_directory()}} config user.email)" \
        commit -q -m "rusticprofile $PKGVER"
    git push origin master 2>&1 | tail -3
    pass "published rusticprofile $PKGVER to the AUR"
    echo "  https://aur.archlinux.org/packages/rusticprofile"

# Merge the active PR, switch to main, pull, and delete the branch (requires gh)
merge-pr:
    #!/usr/bin/env bash
    set -euo pipefail
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    if [ "$BRANCH" = "main" ]; then
        echo "Error: You are already on main."
        exit 1
    fi
    echo "Merging PR for branch $BRANCH..."
    gh pr merge --squash --delete-branch
    echo "Switching to main and pulling..."
    git checkout main
    git pull
    echo "Deleting local branch $BRANCH..."
    git branch -D "$BRANCH" 2>/dev/null || true
    git fetch --prune
    echo ""
    echo "Reminder: update WIP.md (active branch, latest commit, open tasks) before"
    echo "ending the session or switching machines — AGENTS.md Part 1 section 3."

# Pre-PR gate: automated checks plus a manual checklist, all of which must pass
pr:
    #!/usr/bin/env bash
    set -euo pipefail
    BOLD='\033[1m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
    pass() { echo -e "${GREEN}[✓]${NC} $1"; }
    fail() { echo -e "${RED}[✗]${NC} $1"; exit 1; }
    info() { echo -e "${YELLOW}[→]${NC} $1"; }

    echo -e "\n${BOLD}=== Pre-PR Gate ===${NC}\n"

    # 1. Must be on a feature branch
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    [ "$BRANCH" = "main" ] && fail "On main — create a feature branch first"
    pass "Feature branch: $BRANCH"

    # 2. Version must be bumped past the last tag.
    #    With no tags yet, `git describe` finds nothing and any version passes — that is
    #    correct for the pre-1.0 scaffold and starts enforcing itself at the first tag.
    CARGO_VER=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")
    [ "$LAST_TAG" = "v$CARGO_VER" ] && fail "Version not bumped — Cargo.toml is still $CARGO_VER (matches last tag)"
    pass "Version: $CARGO_VER (last tag: $LAST_TAG)"

    # 3. NOTES.md Current State header must match
    grep -q "## Current State (v$CARGO_VER)" NOTES.md \
        || fail "NOTES.md Current State header not updated to v$CARGO_VER"
    pass "NOTES.md Current State header: v$CARGO_VER"

    # 4. Regenerate man page and verify it was committed
    info "Regenerating man page..."
    just man
    MAN_DIRTY=$(git diff --name-only docs/rusticprofile.1)
    [ -n "$MAN_DIRTY" ] && fail "docs/rusticprofile.1 was regenerated but not committed — stage and commit it first"
    pass "docs/rusticprofile.1 is current and committed"

    # 5. cargo check — updates Cargo.lock; verify it was committed
    info "Running cargo check..."
    cargo check -q 2>&1
    LOCK_DIRTY=$(git diff --name-only Cargo.lock)
    [ -n "$LOCK_DIRTY" ] && fail "Cargo.lock was updated but not committed — stage and commit it first"
    pass "Cargo.lock is current and committed"

    # 6. fmt + clippy
    info "Running just check..."
    just check
    pass "fmt + clippy passed"

    # 7. Tests
    info "Running cargo test..."
    cargo test -q 2>&1
    pass "All tests passed"

    # 8. Security audit (advisory — surfaces RustSec advisories locally before CI,
    #    but does NOT block: advisories can be newly published against unchanged
    #    transitive deps, which shouldn't hard-fail otherwise-ready work).
    info "Running cargo audit..."
    if ! command -v cargo-audit >/dev/null 2>&1; then
        info "cargo-audit not installed — installing (cargo install cargo-audit)..."
        cargo install cargo-audit || info "cargo-audit install failed — skipping audit this run"
    fi
    if command -v cargo-audit >/dev/null 2>&1; then
        if cargo audit; then
            pass "cargo audit: no advisories"
        else
            info "cargo audit reported advisories (above) — advisory only, NOT blocking the gate"
        fi
    fi

    # Manual checklist
    echo -e "\n${BOLD}Automated checks passed.${NC}\n"
    echo -e "${BOLD}Manual checklist — confirm each before proceeding:${NC}"
    echo "  [ ] README.md reviewed and updated (new commands, flags, config keys)"
    echo "  [ ] NOTES.md release log entry added under Release Log"
    echo "  [ ] PLAN.md updated if a design decision changed (it is the design record)"
    echo "  [ ] No live infrastructure identifiers added to tracked files (see WIP.md)"
    echo "  [ ] Safety rules observed: no prune against the shared repo, no snapshots deleted"
    echo ""

    # The checklist is a human gate, so it must stay a deliberate act — but it must not
    # deadlock a caller that has no terminal. An agent shell, a CI step or a pipeline all
    # reach this line with stdin closed or connected to something that will never answer,
    # and a bare `read` there blocks or dies without saying why. Three sources of an
    # answer, tried in order:
    #
    #   1. PR_CONFIRM in the environment — the explicit answer for any non-interactive
    #      caller. It is not a bypass: setting it is the same act of confirmation as
    #      typing y, just recorded where a script can supply it.
    #   2. An interactive stdin — a human at a terminal, prompted exactly as before.
    #   3. Neither, so read whatever was piped in, bounded by a timeout. `echo y | just
    #      open-pr` keeps working, and a stdin that never answers costs ten seconds
    #      instead of hanging until someone notices.
    #
    # The failure message names PR_CONFIRM, because a gate that cannot be satisfied from
    # the context it failed in is not a gate, it is a wall.
    if [ -n "${PR_CONFIRM:-}" ]; then
        CONFIRM="$PR_CONFIRM"
        echo "All manual items confirmed? [y/N] $CONFIRM   (answered by PR_CONFIRM)"
    elif [ -t 0 ]; then
        echo -n "All manual items confirmed? [y/N] "
        read -r CONFIRM
    else
        echo -n "All manual items confirmed? [y/N] "
        read -r -t 10 CONFIRM || CONFIRM=""
        echo "$CONFIRM"
        [ -n "$CONFIRM" ] \
            || fail "No terminal to confirm the checklist on, and nothing on stdin. Re-run with PR_CONFIRM=y once each item above is actually checked."
    fi
    [ "$CONFIRM" = "y" ] || [ "$CONFIRM" = "Y" ] \
        || { echo -e "${RED}Aborted.${NC} Complete the checklist first."; exit 1; }

    echo -e "\n${GREEN}Gate passed. You may now run: gh pr create${NC}\n"

# Run the pre-PR gate, then gh pr create — always use this, never gh pr create directly
open-pr *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    #   just open-pr --title "..." --body-file body.md      # at a terminal
    #   PR_CONFIRM=y just open-pr --title "..." --fill      # script, CI or agent
    #
    # This recipe is the only thing that can gate PR creation: neither `gh` nor `git`
    # has a hook for "a PR is about to open". Being a Justfile recipe rather than
    # editor or agent configuration, it binds every contributor and tool identically.
    #
    # At a terminal gh keeps stdin, so its interactive flow still works when open-pr is
    # called with no arguments. Without one, gh is given an explicitly empty stdin: it
    # would otherwise inherit whatever the gate's checklist prompt drained (`echo y |
    # just open-pr`), and gh reads stdin itself for `--body-file -`. Failing at the last
    # step on an unrelated dead pipe is a confusing way to lose a passed gate.
    just pr
    if [ -t 0 ]; then
        gh pr create "$@"
    else
        gh pr create "$@" </dev/null
    fi

# Generate a flamegraph for execution profiling (requires perf on Linux or dtrace on macOS)
flamegraph *ARGS="":
    @command -v cargo-flamegraph >/dev/null || (echo "Installing cargo-flamegraph..." && cargo install flamegraph)
    @if [ "$(uname)" = "Linux" ] && ! command -v perf >/dev/null; then \
        echo "Error: 'perf' is not installed. Please install 'perf' (e.g., 'sudo dnf install perf' on Fedora)"; \
        exit 1; \
    fi
    cargo flamegraph --profile profiling -- {{ARGS}}
