# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
#
# ============================================================================
# CANONICAL COMMON JUSTFILE BLOCK — template v1
# ============================================================================
#
# The text below `---- BEGIN CANONICAL ----` is the reference for the block
# delimited by `# >>> COMMON (template v1)` and `# <<< COMMON` in a project's
# Justfile. `just standard-check` extracts that block and diffs it against this
# file, so drift is a failure rather than something noticed years later.
#
# Vendored in: rusticprofile, retch, etr.
#
#
# WHY A VENDORED COPY RATHER THAN `just import`
# ---------------------------------------------
# These are three separate git repositories. An `import` still needs the
# imported file to exist in each one, so it moves the duplication rather than
# removing it, and a submodule for one file costs more than it saves. What
# makes a copy safe is not that it is a copy — it is that something CHECKS it.
#
# This is not a hypothetical. rusticprofile measured and fixed three completion
# defects; both sibling repos then carried all three for months, because
# nothing compared them:
#
#   1. nushell completions written to the XDG path, which Windows nushell never
#      reads (retch and etr both).
#   2. output claiming zsh completions are "auto-loaded from the default
#      $fpath", which is false on every distribution (etr).
#   3. `install-completions` as a shebang recipe, which cannot run on Windows
#      at all without `cygpath` (etr).
#
# That is the standing rule — a fact nothing verifies is a fact that rots —
# demonstrated across repos rather than within one file.
#
#
# WHY EVERY BODY IS WRITTEN AGAINST VARIABLES
# -------------------------------------------
# `etr` ships two binaries (`etr`, `etrs`); the others ship one. A block with a
# hardcoded binary name cannot be copied, and a standard that cannot be copied
# is decoration. So project facts live ABOVE the block, in a PROJECT header
# each repo owns:
#
#     BINS      := "etr etrs"                          # space separated
#     MAN_PAGES := "man/build/etr.1 man/build/etrs.1"  # space separated
#
# Adding a project-specific recipe is normal — put it OUTSIDE the markers,
# under `# ===== PROJECT-SPECIFIC =====`. Editing anything INSIDE the markers
# means changing the standard: edit this file, bump the template version in the
# marker, and propagate to the siblings in their own PRs.
#
#
# SCOPE OF v1, STATED SO THE GAPS ARE NOT MISTAKEN FOR OVERSIGHTS
# ---------------------------------------------------------------
# v1 covers the completion/man/install family and `standard-check` — the area
# where the measured defects were. It deliberately does NOT yet cover
# `check`/`lint`/`test`/`pr`/`open-pr`/`merge-pr`, because those legitimately
# differ today (`--workspace` in retch, `--all-targets` in etr, bare in
# rusticprofile) and reconciling them is a behaviour change per repo rather
# than a copy. `man` stays project-specific: one repo commits its page, another
# gitignores it, and one builds two.
#
# Known divergences left alone by v1, for whoever extends it:
#   * `Justfile` (rusticprofile, retch) vs `justfile` (etr)
#   * `lint` (rusticprofile, retch) vs `clippy` (etr)
#   * `## Current State (vX)` vs `## Current state: vX` in NOTES.md
#   * etr has no `install-hooks` and no `open-pr`, so its pre-push gate is
#     installed by nothing and its PR path has no gated call site
#
#
# THE MEASUREMENTS BEHIND THE TWO NON-OBVIOUS EXPRESSIONS
# -------------------------------------------------------
# NU_COMP. `$nu.user-autoload-dirs` on Windows is exactly
# `%APPDATA%\nushell\autoload`, one entry; nushell there does not read
# `~/.config/nushell/autoload` at all, whatever XDG_CONFIG_HOME says. Getting
# this wrong is SILENT — the recipe prints "Installed" and delivers nothing.
# Four details, each of which breaks it if dropped:
#   * `${APPDATA:-}`, not `$APPDATA` — just's default shell is `sh -cu`, and
#     `-u` aborts on an unset variable, i.e. on every Unix host. A Windows-only
#     fix would otherwise break the entire Unix fleet.
#   * `tr` — `$APPDATA` is full of backslashes and `sh` treats those as
#     escapes. Windows accepts forward slashes in every path API.
#   * FOUR backslashes, not two. Two survive just and `sh` as one, which `tr`
#     reads as a trailing unescaped backslash: it substitutes correctly but
#     warns `tr: warning: an unescaped backslash at end of string is not
#     portable` on every invocation. Measured through `just --evaluate`, not a
#     `sh -c` probe — PowerShell rewrites backslashes in native arguments and
#     reports the opposite.
#   * the else branch is byte-for-byte the plain XDG expression, and `tr` runs
#     only in the then-branch, so this quoting cannot reach a Unix host.
#
# PS_COMP is very likely wrong on Windows in the same way and is deliberately
# left as the XDG path. `~/.config/powershell` is right for pwsh on Linux and
# macOS; on Windows the profile directory is `Split-Path $PROFILE`, which
# OneDrive folder-redirection can move and which no other platform can compute.
# Measured: `$PROFILE` was under `OneDrive\Documents\PowerShell` and did not
# source `~/.config/powershell`, so the file written there is dead. Replacing a
# known-harmless wrong answer with a guessed one is the wrong trade; this wants
# a design decision, not a substitution.
#
# ---- BEGIN CANONICAL ----

# Completion and man directories. Full rationale for NU_COMP and PS_COMP is in
# templates/justfile-common.just — both are measured, and both are silent when wrong.
BASH_COMP   := `echo "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"`
ZSH_COMP    := `echo "${XDG_DATA_HOME:-$HOME/.local/share}/zsh/site-functions"`
FISH_COMP   := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"`
ELVISH_COMP := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/elvish/lib"`
MAN_DIR     := `echo "${XDG_DATA_HOME:-$HOME/.local/share}/man"`
NU_COMP     := `if [ -n "${APPDATA:-}" ]; then printf '%s/nushell/autoload' "$(printf %s "$APPDATA" | tr '\\\\' '/')"; else echo "${XDG_CONFIG_HOME:-$HOME/.config}/nushell/autoload"; fi`
PS_COMP     := `echo "${XDG_CONFIG_HOME:-$HOME/.config}/powershell"`

# Install from this checkout: binary, man page(s) and completions.
#
# The dependencies are the point. `cargo install` alone replaces the binary and leaves the
# man page and completions at whatever version last ran their recipe — measured on a host
# whose page was ELEVEN releases stale with nothing reporting it.
install: install-man install-completions
    cargo install --path .

# Install a RELEASED tag: binary, man page(s) and completions, all three FROM THAT TAG.
#
# This is what a fleet upgrade wants, and its absence is what let the stale page above
# happen — the upgrade was a hand-typed `cargo install --git --tag`, which is binary-only.
#
# **It deliberately does NOT depend on `install-man`/`install-completions`, and that is the
# whole correctness argument.** Those two work from the CHECKOUT: `install-man` copies the
# working tree's page and `install-completions` runs `target/debug/<bin>`. Reusing them here
# would install a binary from the tag alongside a man page and completions from whatever the
# working tree happens to be — on a checkout one release ahead, a v0.2.22 binary with a
# v0.2.23 man page. Mismatched artefacts that each look fine is the exact failure class this
# standard exists to remove, so the three sources are made to agree instead:
#
#   binary       cargo install --git --tag
#   completions  generated by THE INSTALLED BINARY, so they cannot disagree with its CLI
#   man page(s)  `git show v<V>:<path>`, i.e. the tag's own text, not the worktree's
#
# Never `--path`: on a Syncthing-shared checkout that builds from a directory other machines
# write into. Takes a bare version and normalises a leading `v`, because both get typed.
install-tag VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    V="{{VERSION}}"; V="${V#v}"
    if [ -z "$V" ]; then echo "error: install-tag needs a version, e.g. just install-tag 0.2.22" >&2; exit 1; fi
    git rev-parse -q --verify "refs/tags/v${V}" >/dev/null || {
        echo "error: tag v${V} is not in this clone. Run: git fetch --tags" >&2; exit 1; }
    REPO=$(git config --get remote.origin.url)
    echo "Installing from tag v${V} of ${REPO}"
    cargo install --git "$REPO" --tag "v${V}" --locked --force
    # POST-CONDITION: cargo prints a replacement line, but only a version query proves which
    # binary is on PATH now — the same reason the man recipe asserts its own output.
    for b in {{BINS}}; do
        command -v "$b" >/dev/null 2>&1 || { echo "error: $b is not on PATH after install" >&2; exit 1; }
        echo "  $b -> $("$b" --version)"
    done
    mkdir -p "{{MAN_DIR}}/man1"
    for m in {{MAN_PAGES}}; do
        # A repo that gitignores its built man pages has nothing to extract; say so rather
        # than failing, since the binary and completions are still correctly installed.
        if git cat-file -e "v${V}:$m" 2>/dev/null; then
            git show "v${V}:$m" > "{{MAN_DIR}}/man1/$(basename "$m")"
            echo "  man page $(basename "$m") <- v${V}"
        else
            echo "  note: $m is not tracked at v${V}; man page left as-is (run just install-man from a checkout at that tag)"
        fi
    done
    mkdir -p "{{BASH_COMP}}" "{{ZSH_COMP}}" "{{FISH_COMP}}" "{{ELVISH_COMP}}" "{{NU_COMP}}" "{{PS_COMP}}"
    for b in {{BINS}}; do
        "$b" --completions bash        > "{{BASH_COMP}}/$b"
        "$b" --completions zsh         > "{{ZSH_COMP}}/_$b"
        "$b" --completions fish        > "{{FISH_COMP}}/$b.fish"
        "$b" --completions elvish      > "{{ELVISH_COMP}}/$b.elv"
        "$b" --completions nushell     > "{{NU_COMP}}/50$b-completions.nu"
        "$b" --completions power-shell > "{{PS_COMP}}/$b.ps1"
        echo "  completions for $b <- the installed binary"
    done

# Install the man page(s) to the XDG man directory.
install-man: man
    @mkdir -p "{{MAN_DIR}}/man1"
    @for m in {{MAN_PAGES}}; do install -m 644 "$m" "{{MAN_DIR}}/man1/$(basename "$m")"; done
    @echo "Man page(s) installed to {{MAN_DIR}}/man1"
    @echo "  add {{MAN_DIR}} to MANPATH if it is not already there"

# Generate and install shell completions for every binary.
#
# **Deliberately NOT a shebang recipe** — on Windows just translates a shebang recipe's
# script path with `cygpath`, for any interpreter, so a shebang version dies before
# running a line. A plain recipe runs through just's own `sh -cu` and needs no setup. The
# cost is that each LINE is its own shell, which is why the loop is one long line.
install-completions: build
    @mkdir -p "{{BASH_COMP}}" "{{ZSH_COMP}}" "{{FISH_COMP}}" "{{ELVISH_COMP}}" "{{NU_COMP}}" "{{PS_COMP}}"
    @for b in {{BINS}}; do B="{{justfile_directory()}}/target/debug/$b"; "$B" --completions bash > "{{BASH_COMP}}/$b"; "$B" --completions zsh > "{{ZSH_COMP}}/_$b"; "$B" --completions fish > "{{FISH_COMP}}/$b.fish"; "$B" --completions elvish > "{{ELVISH_COMP}}/$b.elv"; "$B" --completions nushell > "{{NU_COMP}}/50$b-completions.nu"; "$B" --completions power-shell > "{{PS_COMP}}/$b.ps1"; echo "Installed completions for $b"; done
    @echo ""
    @# zsh reads completion functions ONLY from directories on `fpath`, and
    @# ~/.local/share/zsh/site-functions is NOT on it by default on any distribution.
    @# Writing the file there and printing "auto-loaded" is a lie on such a machine: the
    @# completion silently never loads and `<cmd> <tab>` produces nothing. Check instead
    @# of claiming. `zsh -i` is required — a non-interactive zsh sources neither .zshrc
    @# nor anything it includes, so checking the default fpath reported NOT ACTIVE on a
    @# machine where completion worked perfectly, the mirror of the bug it replaced.
    @if command -v zsh >/dev/null 2>&1; then if zsh -i -c 'print -l $fpath' 2>/dev/null | grep -qx "{{ZSH_COMP}}"; then echo "  zsh        auto-loaded from {{ZSH_COMP}}"; else printf '  zsh        NOT ACTIVE — %s is not on your $fpath.\n             The file is written but zsh will never read it. Add this to\n             ~/.zshrc BEFORE compinit runs, then restart the shell:\n\n                 fpath+=(%s)\n\n' "{{ZSH_COMP}}" "{{ZSH_COMP}}"; fi; fi
    @echo "Notes:"
    @echo "  bash       source {{BASH_COMP}}/<cmd>  (or restart shell)"
    @echo "  fish       auto-loaded from {{FISH_COMP}}"
    @echo "  elvish     add to rc.elv:  eval (slurp < {{ELVISH_COMP}}/<cmd>.elv)"
    @echo "  nushell    auto-loaded from {{NU_COMP}}"
    @echo "  powershell add to \$PROFILE:  . {{PS_COMP}}/<cmd>.ps1"
    @echo ""
    @echo "  Shell aliases do not inherit completions. For an alias, tell your shell they"
    @echo "  are the same command:   zsh  compdef <alias>=<cmd>"
    @echo "                          fish complete -c <alias> -w <cmd>"
    @echo "                          bash complete -o default -F _<cmd> <alias>"

# Fail if this repo's COMMON block has drifted from templates/justfile-common.just.
#
# A missing marker is a FAILURE, not "no drift" — an extraction that silently matched
# nothing would report a clean result for a Justfile with no common block at all.
standard-check:
    #!/usr/bin/env bash
    set -euo pipefail
    JF="{{justfile()}}"
    REF="{{justfile_directory()}}/templates/justfile-common.just"
    [ -f "$REF" ] || { echo "error: $REF is missing — nothing to check against" >&2; exit 1; }
    grep -q '^# >>> COMMON' "$JF" || { echo "error: no '# >>> COMMON' marker in $JF" >&2; exit 1; }
    grep -q '^# <<< COMMON' "$JF" || { echo "error: no '# <<< COMMON' marker in $JF" >&2; exit 1; }
    ACT=$(mktemp); EXP=$(mktemp)
    trap 'rm -f "$ACT" "$EXP"' EXIT
    awk '/^# >>> COMMON/{f=1;next} /^# <<< COMMON/{f=0} f' "$JF" > "$ACT"
    awk '/^# ---- BEGIN CANONICAL ----/{f=1;next} f' "$REF" > "$EXP"
    [ -s "$EXP" ] || { echo "error: extracted nothing from $REF — is the BEGIN CANONICAL marker there?" >&2; exit 1; }
    [ -s "$ACT" ] || { echo "error: extracted nothing between the COMMON markers in $JF" >&2; exit 1; }
    if ! diff -u "$EXP" "$ACT"; then
        echo "" >&2
        echo "error: the COMMON block has drifted from templates/justfile-common.just." >&2
        echo "       Editing inside the markers means changing the STANDARD: edit the" >&2
        echo "       template, bump its version in the marker, and propagate to the" >&2
        echo "       sibling repos in their own PRs." >&2
        exit 1
    fi
    echo "COMMON block matches templates/justfile-common.just (template v1)"
