# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a

# Justfile for rusticprofile
# Run with: just <recipe>

# Required for shebang recipes to receive *ARGS as real argv ($@) instead of
# losing quoting via textual {{ARGS}} interpolation (see open-pr).
set positional-arguments := true

# **There is deliberately no `set windows-shell` here. Adding one broke Windows in `0.2.0`.**
#
# `0.2.0` set it to `["bash", "-cu"]`, reasoning that the recipes are POSIX and should not depend
# on whichever `sh` is first on PATH. That was wrong in the worst direction: `bash` is *not* on a
# default Windows PATH, while `sh` often is — so the setting turned a Justfile that worked into
# one where **every backtick variable below failed to evaluate**, taking down `just install` and
# anything else that reads them:
#
#     error: backtick could not be run because just could not find the shell: program not found
#
# just's own default is already `sh -cu`, which resolves. So the setting bought nothing and cost
# the whole file. It shipped because it was only ever tested in a shell that had already been
# fixed up with the PATH entry below — the environment the fix creates, never the one users have.
#
# **The shebang recipes are a separate matter and do need a setup step.** Three things, and the
# first has the least helpful error message of the three:
#
#   1. **`cygpath`** — on Windows, just translates a shebang recipe's temporary script path with
#      it, for *any* interpreter. Without it every shebang recipe dies before running a line:
#
#          error: could not find `cygpath` executable to translate recipe
#          `install-completions` shebang interpreter path: program not found
#
#      That names neither PATH nor Git, so it reads as a just bug rather than a missing tool.
#   2. **`bash`** — the interpreter the shebang recipes actually ask for.
#   3. The **coreutils they call**: `sha256sum`, `date`, `install`, `grep`, `sed`, `cut`. None are
#      on a default Windows PATH, and `find` there resolves to `C:\Windows\system32\find.exe` — a
#      text-search tool, the same shadowing trap `~/AGENTS.md` records for `bfs`/`find` and
#      `eza`/`ls`.
#
# Git for Windows ships all of them in `usr\bin`, so prepending that one directory satisfies all
# three — verified: `just check` (golden staleness gate included), `just man` and
# `just install-completions` all run green there. Add it to PATH once, in whichever shell you
# actually use:
#
#   PowerShell:
#     $env:PATH = "$env:USERPROFILE\scoop\apps\git\current\usr\bin;$env:PATH"
#
#   nushell — note the FORWARD slashes. nu treats `\` as an escape inside double quotes, so the
#   backslash form fails with `unrecognized escape sequence '\s'`. Windows accepts `/` in PATH.
#     $env.PATH = ($env.PATH | prepend ($env.USERPROFILE + "/scoop/apps/git/current/usr/bin"))
#
# Adjust the prefix for a non-scoop install (typically `C:\Program Files\Git\usr\bin`).

# ===== PROJECT — the only part of the install family this repo owns =====
#
# The COMMON block below is written against these, so it can be byte-identical across
# repos that ship different binaries. `etr` sets BINS to two names; this repo has one.
BINS      := "rusticprofile"
MAN_PAGES := "docs/rusticprofile.1"

# Default recipe — list available commands (must precede the COMMON block so bare `just` does not run `install`)
default:
    @just --list

# Do NOT edit inside the markers below. Edit templates/justfile-common.just, bump the
# version in the marker, and propagate to the sibling repos in their own PRs.
# `just standard-check` fails if this block and that file disagree, and `just check`
# depends on it — so drift is a build failure rather than a discovery years later.
# >>> COMMON (template v6)
# The interpreter is resolved ONCE per line, and a missing one is a hard error. The
# `python3 … 2>/dev/null || python …` idiom is deliberately NOT used: it retries on ANY
# failure, so a real error inside the script gets re-run and reported as if the
# interpreter were the problem.
PY := `command -v python3 || command -v python || echo PYTHON-NOT-FOUND`

# Install from this checkout: binary, man page(s) and completions.
#
# The dependencies are the point. `cargo install` alone replaces the binary and leaves the
# man page and completions at whatever version last ran their recipe — measured on a host
# whose page was ELEVEN releases stale with nothing reporting it.
install: install-man install-completions
    cargo install --path .

# Install a RELEASED tag: binary, man page(s) and completions, all three FROM THAT TAG.
#
# **It deliberately does NOT depend on `install-man`/`install-completions`**, because those
# work from the checkout. Reusing them would pair a tag's binary with the worktree's man
# page and completions — on a checkout one release ahead, a v0.2.22 binary with a v0.2.23
# page. Mismatched artefacts that each look fine is the failure class this standard exists
# to remove, so the three sources are made to agree: binary from the tag, completions from
# THE INSTALLED BINARY (`--from-path`), man page from the tag (`--from-tag`).
#
# Never `--path`: on a Syncthing-shared checkout that builds from a directory other
# machines write into. Takes a bare version and normalises a leading `v`.
install-tag VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    V="{{VERSION}}"; V="${V#v}"
    [ -n "$V" ] || { echo "error: install-tag needs a version, e.g. just install-tag 0.2.22" >&2; exit 1; }
    git rev-parse -q --verify "refs/tags/v${V}" >/dev/null || {
        echo "error: tag v${V} is not in this clone. Run: git fetch --tags" >&2; exit 1; }
    REPO=$(git config --get remote.origin.url)
    echo "Installing from tag v${V} of ${REPO}"
    cargo install --git "$REPO" --tag "v${V}" --locked --force
    # POST-CONDITION: cargo prints a replacement line, but only a version query proves which
    # binary is on PATH now.
    for b in {{BINS}}; do
        command -v "$b" >/dev/null 2>&1 || { echo "error: $b is not on PATH after install" >&2; exit 1; }
        echo "  $b -> $("$b" --version)"
    done
    "{{PY}}" scripts/install_man.py {{MAN_PAGES}} --from-tag "v${V}"
    "{{PY}}" scripts/install_completions.py {{BINS}} --from-path

# Install the man page(s) to the XDG man directory.
#
# Installs the COMMITTED page(s) and deliberately does NOT depend on `man` (v6). Every repo
# using this block commits its pages and its gate refuses a stale one, so rebuilding here
# only made `just install` require mandown -- a documentation build tool -- to install a
# finished page, and rewrote a tracked file in the user's checkout. Edited the page source?
# Run `just man` first; the gate would refuse the stale page at PR time anyway.
install-man:
    @"{{PY}}" scripts/install_man.py {{MAN_PAGES}}

# Generate and install shell completions for every binary.
#
# Python rather than a just recipe, which is retch's finding and the more portable
# mechanism: no `sh`, no `cygpath`, no coreutils, nothing from Git's `usr\bin` on Windows.
# A `bash` shebang recipe cannot run on Windows without `cygpath` at all, and even a plain
# `sh` recipe still needs an `sh` on PATH.
install-completions: build
    @"{{PY}}" scripts/install_completions.py {{BINS}}

# Prove the vendored helpers still behave the way the standard requires.
#
# **This runs the helpers' own self-tests rather than diffing text**, and that is the whole
# point: three separate repositories cannot diff each other's files, but each can prove its
# copy still behaves correctly — which is the property that was actually violated when two
# repos quietly shipped the pre-fix nushell path for months. A text diff would also have
# passed happily on a repo that had never adopted the standard at all.
standard-check:
    @"{{PY}}" scripts/install_completions.py --self-test
    @"{{PY}}" scripts/install_man.py --self-test
    @"{{PY}}" scripts/gate_conformance.py --self-test
    @"{{PY}}" scripts/gate_conformance.py "{{justfile()}}"
# <<< COMMON

# ===== PROJECT-SPECIFIC — everything below is this repo's own =====

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

# Check code formatting without modifying files
fmt-check:
    cargo fmt -- --check

# Run clippy lints
lint:
    cargo clippy --all-targets -- -D warnings

# Alias for lint (cross-repo standard)
clippy: lint

# Run strict checks (formatting, linting, golden argv files, man page, packaging and byte drift) as done in CI
check: fmt-check lint standard-check golden-is-current man-check render-check aur-check copr-check metadata-check text-check wip-check
    @echo "All checks passed."

# Regenerate the golden argv files under tests/golden
golden:
    RP_UPDATE_GOLDEN=1 cargo test --test cli_tests golden_

# Fail if any golden argv file is stale or untracked (regenerate with `just golden`)
golden-is-current:
    @"{{PY}}" scripts/golden_check.py --self-test
    @"{{PY}}" scripts/golden_check.py

# Run security audit (requires cargo-audit)
audit:
    @command -v cargo-audit >/dev/null || cargo install cargo-audit
    cargo audit

# Generate the man page from Markdown using mandown
man:
    @"{{PY}}" scripts/man_pages.py --build

# Verify docs/rusticprofile.1 is current in the worktree and at HEAD (offline, shell-free)
man-check:
    @"{{PY}}" scripts/man_pages.py --self-test
    @"{{PY}}" scripts/man_pages.py --check

# Run criterion micro-benchmarks (none yet — see the NOTES.md backlog)
bench:
    @echo "No benchmarks yet. Add benches/ and the [[bench]] table when config parsing lands."

# Install git hooks (run once after cloning)
install-hooks:
    @"{{PY}}" scripts/install_hooks.py

# One-time repo setup: install git hooks and any other local tooling
setup: install-hooks
    @echo "Repo setup complete."

# Full development setup
dev: setup fmt lint test build
    @echo "Development build complete."

# Dry-run publish check (no upload)
publish-check:
    cargo publish --dry-run

# Publish to crates.io — refuses unless HEAD is the release tag
publish:
    #!/usr/bin/env bash
    set -euo pipefail
    RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
    fail() { echo -e "${RED}[✗]${NC} $1" >&2; exit 1; }
    pass() { echo -e "${GREEN}[✓]${NC} $1"; }

    VER=$("{{PY}}" scripts/render_packaging.py --print-version)
    TAG="v${VER}"
    if [ "${PUBLISH_ANY_REF:-0}" != "1" ]; then
        TAG_COMMIT=$(git rev-list -n 1 "refs/tags/${TAG}" 2>/dev/null || true)
        [ -n "$TAG_COMMIT" ] \
            || fail "tag ${TAG} does not exist. Tag and push the release first (or set PUBLISH_ANY_REF=1 to override)."
        HEAD_COMMIT=$(git rev-parse HEAD)
        [ "$HEAD_COMMIT" = "$TAG_COMMIT" ] \
            || fail "HEAD ($(git rev-parse --short HEAD)) is not ${TAG} ($(git rev-parse --short "$TAG_COMMIT")). Check out ${TAG} or main at the tag first."
        pass "HEAD is ${TAG} ($(git rev-parse --short HEAD))"
    fi
    cargo publish

# Run all offline packaging guards
packaging-check: render-check aur-check copr-check metadata-check

# Verify scripts/render_packaging.py still substitutes and still refuses (offline)
render-check:
    @"{{PY}}" scripts/render_packaging.py --self-test

# Verify packaging/aur/PKGBUILD is still a template recording no version (offline)
aur-check:
    @"{{PY}}" scripts/aur_check.py --self-test
    @"{{PY}}" scripts/aur_check.py

# Generate .SRCINFO beside a RENDERED PKGBUILD in DIR (needs podman)
aur-srcinfo DIR:
    #!/usr/bin/env bash
    set -euo pipefail
    RED='\033[0;31m'; NC='\033[0m'
    fail() { echo -e "${RED}[✗]${NC} $1" >&2; exit 1; }

    DIR="$(cd "{{DIR}}" && pwd)"
    [ -f "$DIR/PKGBUILD" ] || fail "$DIR/PKGBUILD not found — render it first with: just aur-render <version> <dir>"
    OUT="$DIR/.SRCINFO"
    command -v podman >/dev/null || fail "podman is required (or edit this recipe for docker)"

    TMP="$(mktemp "$DIR/.SRCINFO.XXXXXX")"
    trap 'rm -f "$TMP"' EXIT

    podman run --rm -v "$DIR:/pkg:ro,z" archlinux:base-devel bash -c '
        useradd -m builder; mkdir -p /home/builder/b && cp /pkg/PKGBUILD /home/builder/b/
        chown -R builder /home/builder/b; cd /home/builder/b
        su builder -c "makepkg --printsrcinfo"' > "$TMP"

    [ -s "$TMP" ] || fail ".SRCINFO came back empty — $OUT left untouched"
    grep -q '^pkgbase = ' "$TMP" || fail "output has no 'pkgbase =' line — $OUT left untouched"

    chmod 0644 "$TMP"
    mv "$TMP" "$OUT"
    trap - EXIT
    "{{PY}}" "{{justfile_directory()}}/scripts/aur_check.py" --dir "$DIR"

# Render packaging/aur/{PKGBUILD,.SRCINFO} for a released tag into DIR (needs network + podman)
aur-render VERSION DIR:
    #!/usr/bin/env bash
    set -euo pipefail
    RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
    fail() { echo -e "${RED}[✗]${NC} $1" >&2; exit 1; }
    V="{{VERSION}}"; V="${V#v}"
    DIR="{{DIR}}"
    URL="https://github.com/l1a/rusticprofile/archive/refs/tags/v${V}.tar.gz"

    curl -sfIL -o /dev/null "$URL" || fail "no release tarball at $URL — tag and push v$V first"
    SHA=$(curl -sfL "$URL" | sha256sum | cut -d' ' -f1)
    mkdir -p "$DIR"
    "{{PY}}" "{{justfile_directory()}}/scripts/render_packaging.py" \
        --target aur --version "$V" --sha256 "$SHA" --out "$DIR/PKGBUILD"
    just aur-srcinfo "$DIR"
    echo -e "${GREEN}[✓]${NC} rendered PKGBUILD + .SRCINFO for v${V} (sha256 ${SHA}) in $DIR"

# Render and build-verify the AUR package for a released tag in a container (default: latest tag)
aur-verify VERSION="":
    #!/usr/bin/env bash
    set -euo pipefail
    BOLD='\033[1m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
    pass() { echo -e "${GREEN}[✓]${NC} $1"; }
    fail() { echo -e "${RED}[✗]${NC} $1"; exit 1; }
    info() { echo -e "${YELLOW}[→]${NC} $1"; }

    command -v podman >/dev/null || fail "podman is required (or edit this recipe for docker)"
    V="{{VERSION}}"; V="${V#v}"
    if [ -z "$V" ]; then
        TAG=$(git describe --tags --abbrev=0 2>/dev/null || fail "no tags found; pass a version")
        V="${TAG#v}"
    fi

    STAGE=$(mktemp -d)
    trap 'rm -rf "$STAGE"' EXIT
    just aur-render "$V" "$STAGE"

    info "Building the package in archlinux:base-devel (this compiles the crate)..."
    podman run --rm -v "$STAGE:/pkg:ro,z" archlinux:base-devel bash -c '
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

# Render and install locally with makepkg -si, without the AUR (Arch Linux only; default: latest tag)
aur-local VERSION="":
    #!/usr/bin/env bash
    set -euo pipefail
    command -v makepkg >/dev/null || { echo "error: makepkg not found (Arch Linux only)" >&2; exit 1; }
    V="{{VERSION}}"; V="${V#v}"
    if [ -z "$V" ]; then
        TAG=$(git describe --tags --abbrev=0)
        V="${TAG#v}"
    fi
    URL="https://github.com/l1a/rusticprofile/archive/refs/tags/v${V}.tar.gz"
    SHA=$(curl -sfL "$URL" | sha256sum | cut -d' ' -f1)
    STAGE=$(mktemp -d)
    trap 'rm -rf "$STAGE"' EXIT
    "{{PY}}" scripts/render_packaging.py --target aur --version "$V" --sha256 "$SHA" --out "$STAGE/PKGBUILD"
    cd "$STAGE" && makepkg -si

# Render for a released tag and push to the AUR (asks before publishing; default: Cargo.toml version)
aur-publish VERSION="":
    #!/usr/bin/env bash
    set -euo pipefail
    BOLD='\033[1m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
    pass() { echo -e "${GREEN}[✓]${NC} $1"; }
    fail() { echo -e "${RED}[✗]${NC} $1"; exit 1; }
    info() { echo -e "${YELLOW}[→]${NC} $1"; }

    V="{{VERSION}}"; V="${V#v}"
    if [ -z "$V" ]; then
        V=$("{{PY}}" scripts/render_packaging.py --print-version)
    fi

    STAGE=$(mktemp -d)
    trap 'rm -rf "$STAGE"' EXIT
    just aur-render "$V" "$STAGE"

    info "Checking the AUR is reachable..."
    OUT=$(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new aur@aur.archlinux.org help 2>&1 || true)
    if echo "$OUT" | grep -qi 'maintenance'; then
        fail "the AUR is in a maintenance window — it said: $(echo "$OUT" | head -1)"
    fi
    echo "$OUT" | grep -qi 'Permission denied' \
        && fail "the AUR refused this SSH key; register it at https://aur.archlinux.org/account/"
    pass "AUR reachable and the SSH key is accepted"

    echo
    echo -e "${BOLD}About to publish rusticprofile $V to the AUR.${NC}"
    echo "This is public and immediate."
    echo ""
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

    git clone "ssh://aur@aur.archlinux.org/rusticprofile.git" "$STAGE/pkg" 2>&1 | tail -2
    cp "$STAGE/PKGBUILD" "$STAGE/.SRCINFO" "$STAGE/pkg/"
    cd "$STAGE/pkg"
    if git diff --quiet --exit-code -- PKGBUILD .SRCINFO && [ -z "$(git status --porcelain)" ]; then
        pass "the AUR already matches these files — nothing to push"
        exit 0
    fi
    git add PKGBUILD .SRCINFO
    git -c user.name="$(git -C {{justfile_directory()}} config user.name)" \
        -c user.email="$(git -C {{justfile_directory()}} config user.email)" \
        commit -q -m "rusticprofile $V"
    git push origin master 2>&1 | tail -3
    pass "published rusticprofile $V to the AUR"
    echo "  https://aur.archlinux.org/packages/rusticprofile"

# Alias for aur-publish (cross-repo standard)
publish-aur VERSION="": (aur-publish VERSION)

# ===== COPR =====

# Verify packaging/copr/rusticprofile.spec is still a template and copr.yml's credentials guard holds (offline)
copr-check:
    @"{{PY}}" scripts/copr_check.py --self-test
    @"{{PY}}" scripts/copr_check.py

# Print packaging/copr/rusticprofile.spec rendered for Cargo.toml's version (offline)
copr-render:
    @"{{PY}}" scripts/render_packaging.py --target copr

# Verify every channel's description and licence agrees with packaging/metadata.toml (offline)
metadata-check:
    @"{{PY}}" scripts/metadata_check.py --self-test
    @"{{PY}}" scripts/metadata_check.py

# Sync GitHub repo description and topics from packaging/metadata.toml (pass --dry-run to preview)
github-metadata *ARGS:
    "{{PY}}" scripts/metadata_check.py --sync-github {{ARGS}}

# Refuse control bytes and carriage returns in tracked text (offline, no network)
text-check:
    @"{{PY}}" scripts/text_check.py --self-test
    @"{{PY}}" scripts/text_check.py

# Prove reset_wip.py rewrites only its marked block, and that WIP.md is still LF (offline; passes when absent)
wip-check:
    @"{{PY}}" scripts/reset_wip.py --self-test
    @"{{PY}}" scripts/reset_wip.py --check-endings

# Merge the active PR, switch to main, pull, delete the branch, and update WIP.md (requires gh)
merge-pr:
    #!/usr/bin/env bash
    set -euo pipefail
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    if [ "$BRANCH" = "main" ]; then
        echo "Error: You are already on main."
        exit 1
    fi

    # Verify the PR's remote head commit matches local HEAD so unpushed local commits are never silently dropped.
    LOCAL_HEAD=$(git rev-parse HEAD)
    PR_HEAD=$(gh pr view --json headRefOid --jq .headRefOid 2>/dev/null || echo "")
    if [ -n "$PR_HEAD" ] && [ "$PR_HEAD" != "$LOCAL_HEAD" ]; then
        echo "Error: PR head ($PR_HEAD) does not match local HEAD ($LOCAL_HEAD). Push your changes first."
        exit 1
    fi

    echo "Checking CI on this branch..."
    STATES=$(gh pr view --json statusCheckRollup \
        --jq '[.statusCheckRollup[]? | select(.conclusion != "SKIPPED") | .conclusion]' 2>/dev/null || echo '[]')

    if [ "$(printf '%s' "$STATES" | tr -d '[:space:]')" = "[]" ]; then
        echo "Error: no checks have reported for this commit at all."
        echo "       That is not the same as passing. GitHub sometimes fails to create a run;"
        echo "       force one with: gh workflow run ci.yml --ref $BRANCH"
        exit 1
    fi

    if echo "$STATES" | grep -q '""'; then
        echo "Error: checks are still running. Wait for them, or merge deliberately with gh."
        exit 1
    fi
    if echo "$STATES" | grep -qE 'FAILURE|TIMED_OUT|CANCELLED|ACTION_REQUIRED'; then
        echo "Error: CI is not green on this branch:"
        gh pr view --json statusCheckRollup \
            --jq '.statusCheckRollup[]? | select(.conclusion != "SKIPPED" and .conclusion != "SUCCESS") | "  \(.conclusion)  \(.name)"'
        echo "Fix it, or merge deliberately with gh if you have a reason."
        exit 1
    fi
    echo "CI is green."

    echo "Merging PR for branch $BRANCH..."
    gh pr merge --squash --delete-branch
    echo "Switching to main and pulling..."
    git checkout main
    git pull
    echo "Deleting local branch $BRANCH..."
    git branch -D "$BRANCH" 2>/dev/null || true
    git fetch --prune
    "{{PY}}" scripts/reset_wip.py

# Prune remote branches already merged into main, plus local branches whose upstream is gone
prune-branches:
    #!/usr/bin/env bash
    set -euo pipefail
    git fetch --prune
    for b in $(git branch -r --merged origin/main | sed 's/^[[:space:]]*//' | grep '^origin/' | grep -v -E '^origin/(HEAD|main|gh-pages)$' | sed 's#^origin/##'); do
        echo "Deleting merged remote branch origin/$b..."
        git push origin --delete "$b"
    done
    for b in $(git for-each-ref --format='%(refname:short) %(upstream:track)' refs/heads | awk '$2 == "[gone]" {print $1}'); do
        echo "Deleting local branch $b (upstream gone)..."
        git branch -D "$b"
    done

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
    echo "  [ ] NOTES.md: Current State, backlog (4), hard-won lessons (5) -- NOT a changelog"
    echo "  [ ] WIP.md reflects what is in flight (not a session log -- see its own header)"
    echo "  [ ] NOTES.md §6 updated if a design decision changed (before the code, if it moves the boundary)"
    echo "  [ ] No live infrastructure identifiers in the diff, commit or PR body (NOTES.md 3, 5.6)"
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
    #   PR_CONFIRM=y PR_TITLE="..." PR_BODY="..." just open-pr
    #
    # This recipe is the only thing that can gate PR creation: neither `gh` nor `git`
    # has a hook for "a PR is about to open". Being a Justfile recipe rather than
    # editor or agent configuration, it binds every contributor and tool identically.
    #
    # At a terminal gh keeps stdin, so its interactive flow still works when open-pr is
    # called with no arguments. Without one, gh is given an explicitly empty stdin: it
    # would otherwise inherit whatever the gate's checklist prompt drained (`echo y |
    # just open-pr`), and gh reads stdin itself for `--body-file -`.
    #
    # In non-interactive mode, args passed to open-pr are forwarded. Environment variables
    # PR_TITLE, PR_BODY, PR_BODY_FILE, and PR_FILL can also supply options. If no args or
    # title/body/fill options are supplied, non-interactive mode defaults to --fill so
    # gh pr create finishes cleanly rather than failing with "must provide --title and --body".
    just pr

    # Push the branch if it has no upstream yet. Until 2026-08-10 this recipe pushed nothing,
    # so on a never-pushed branch `gh pr create` had no remote branch to open a PR from and
    # failed non-interactively — *after* the gate had printed "Gate passed", which reads as the
    # gate refusing a change it had just approved. `0.0.21`'s rule applies: a gate that cannot
    # be satisfied from the context it failed in is a wall rather than a gate.
    #
    # Deliberately only when there is no upstream. Pushing unconditionally would make this
    # recipe silently publish commits on a branch that already has one, which is a different
    # and more surprising act than "put this branch somewhere gh can see it".
    if ! git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' >/dev/null 2>&1; then
        BRANCH="$(git rev-parse --abbrev-ref HEAD)"
        [ "$BRANCH" != HEAD ] || { echo "detached HEAD — check out a branch first" >&2; exit 1; }
        echo "no upstream for $BRANCH — pushing it so gh has a remote branch to open from"
        # pre-push runs `just check`, so this cannot publish a branch the gate would refuse.
        git push -u origin "$BRANCH"
    fi

    # Filter out empty arguments passed by just when *ARGS is empty
    CLEAN_ARGS=()
    for arg in "$@"; do
        if [ -n "$arg" ]; then
            CLEAN_ARGS+=("$arg")
        fi
    done

    ENV_ARGS=()
    if [ ${#CLEAN_ARGS[@]} -eq 0 ]; then
        if [ -n "${PR_TITLE:-}" ]; then
            ENV_ARGS+=("--title" "$PR_TITLE")
        fi
        if [ -n "${PR_BODY:-}" ]; then
            ENV_ARGS+=("--body" "$PR_BODY")
        fi
        if [ -n "${PR_BODY_FILE:-}" ]; then
            ENV_ARGS+=("--body-file" "$PR_BODY_FILE")
        fi
        if [ -n "${PR_FILL:-}" ] && [ "$PR_FILL" != "0" ] && [ "$PR_FILL" != "n" ] && [ "$PR_FILL" != "false" ]; then
            ENV_ARGS+=("--fill")
        fi
    fi

    if [ -t 0 ]; then
        gh pr create "${CLEAN_ARGS[@]}" "${ENV_ARGS[@]}"
    else
        if [ ${#CLEAN_ARGS[@]} -eq 0 ] && [ ${#ENV_ARGS[@]} -eq 0 ]; then
            ENV_ARGS+=("--fill")
        fi
        gh pr create "${CLEAN_ARGS[@]}" "${ENV_ARGS[@]}" </dev/null
    fi

# Generate a flamegraph for execution profiling (requires perf on Linux or dtrace on macOS)
flamegraph *ARGS="":
    @command -v cargo-flamegraph >/dev/null || (echo "Installing cargo-flamegraph..." && cargo install flamegraph)
    @if [ "$(uname)" = "Linux" ] && ! command -v perf >/dev/null; then \
        echo "Error: 'perf' is not installed. Please install 'perf' (e.g., 'sudo dnf install perf' on Fedora)"; \
        exit 1; \
    fi
    cargo flamegraph --profile profiling -- {{ARGS}}
