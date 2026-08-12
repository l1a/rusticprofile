# rusticprofile — plan and session handoff

> [!IMPORTANT]
> **This is the historical design record. It is not a status page, and it is no longer the
> place to look for what is true today.**
>
> | you want | read |
> |---|---|
> | what is built, what is released, what is next | `NOTES.md` — "Current State" and the release log |
> | **the rules that will bite you** — retention, locking, prune, the delegation boundary | **`NOTES.md` §3a, "Operating invariants"** |
> | *why* the design is shaped this way, and what was rejected | **this file**, Parts 1–3 |
> | the measurements against rustic 0.11.3 | **this file**, Parts 5 and 7 |
>
> **Section numbers here are permanent anchors.** Seventeen of them are cited by
> `NOTES.md`, `AGENTS.md`, `WIP.md`, the shipped `config --example` and the source. Nothing
> is renumbered or deleted when material is promoted — the section stays, with a pointer.

**Milestones 1, 2, 3 and 5 are complete; M6 is effectively delivered.** M4 (lock coordination) is
deliberately deferred and is defence in depth rather than a precondition — see §7.6. `NOTES.md` is
authoritative for all of this, and **which version is released is not stated in any file** —
`git describe --tags --abbrev=0`, the GitHub release and the crates.io API are the record.

*This line used to read "**Status as of 2026-08-04: … `v0.1.31` is released**", and by the time
anyone read it that was nine releases stale. It is the second copy of the claim `NOTES.md` deleted
for the same reason, and finding it took going to look — which is exactly `0.2.4`'s finding, where
`0.1.32` fixed this file's "pre-code" header and nobody checked whether `AGENTS.md` said the same
thing. It did, for thirty-five releases. **Duplicated state goes stale one copy at a time, and the
copy nobody re-reads is the one that survives**, so the fix is to stop writing the fact down twice
rather than to correct it twice.*

*The line that stood here until 2026-08-04 read "pre-code. Nothing has been implemented." It
was written on 2026-07-30, before the first commit, and stayed through thirty-one releases —
long enough to be the first thing every new session read, and false for nearly all of them.
Kept as a note rather than silently deleted, because a document that opens by describing a
state five milestones out of date is the same silent-staleness failure this project exists to
catch, turned inward.*

This document is the full design plus the reasoning and discoveries behind it. The Part 7
decision — the one thing that blocked all code — was settled on 2026-07-30 in favour of
**Option B** (named snapshot sets, selected per host); Part 7 records it along with the test
results that shaped it.

**Why it reads like a narrative:** it was written as a handoff across Claude accounts, so a
fresh session has no prior context. Everything needed to continue is here or explicitly
pointed at. Written 2026-07-30 on host `host-f`.

**Once the repo is scaffolded**, the forward-looking parts belong in `NOTES.md` (the house
convention for living project state) and this file can shrink to the historical record.

*Acted on 2026-08-04, thirty-one releases later than intended. The operating rules — §7.3,
§7.5, §7.6, §7.7 and §7.8 — are promoted to `NOTES.md` §3a, which is now where they are
maintained. Each section below keeps its number, its full text and its measurements, and
gains a pointer at the top. Nothing was moved out; the authority moved, not the evidence.*

---

# Part 1 — How we got here

The request began as "rewrite resticprofile in Rust and call it rusticprofile." Investigation
changed the shape of the problem twice. Both pivots matter, because the final design is much
smaller than the original request and the reasons are not obvious.

## Pivot 1 — a faithful port is neither tractable nor desirable

resticprofile (Go) is **~30,176 LOC non-test plus ~26,781 LOC of tests** — a 0.89:1 ratio, so
the tests are effectively the specification. Full config compatibility would require
reimplementing three Go libraries that have no Rust equivalent, *before* writing any backup
logic:

- **viper + mapstructure.** Their untyped-`map[string]any` merge semantics, custom key
  delimiter and case-folding *are* the profile-inheritance and mixin implementation
  (`config/config.go:632` `applyProfileInheritanceAndMixins`). `figment` and `config-rs` are not
  substitutes.
- **Go `text/template`.** The entire config file is rendered as a template *before* parsing, and
  re-rendered **per profile** (`config/config.go:164`, `:592`), with 22 custom functions
  (`util/templates/functions.go`). One of them, `randInt`, seeds `math/rand/v2` PCG from an MD5
  hash — not reproducible bit-for-bit in Rust.
- **HCL v1** with repeated-block→slice-of-maps decoding (`config/config_v1.go:138`). No Rust
  implementation exists.

## Pivot 2 — rustic already owns most of what a wrapper would provide

This is the decisive finding and it is easy to miss.

**rustic's native config** (`-P <profile>`, `~/.config/rustic/rustic.toml`,
<https://github.com/rustic-rs/rustic/blob/main/config/README.md>) already covers:
`[global]`, `[global.hooks]`, `[global.env]`, `[global.metrics-labels]`, `[repository]`,
`[repository.options]` (+ hot/cold variants), `[repository.hooks]`, `[snapshot-filter]`,
`[backup]`, `[backup.hooks]`, `[backup.metrics-labels]`, `[[backup.snapshots]]`,
`[forget]`, `[copy]`, `[webdav]` — with env > config > CLI precedence.

That includes **hooks** (`run-before`, `run-after`, `run-failed`, `run-finally` at global,
repository, backup *and* per-source level) and **Prometheus push + labels + OpenTelemetry**.
Those are large parts of resticprofile. A profile-and-flags wrapper for rustic would largely
reinvent rustic.

**`rustic_scheduler` also already exists** in the same org —
<https://github.com/rustic-rs/rustic_scheduler> — described as *"schedule rustic backups for many
clients to a common repository"*, which sounds exactly like this fleet.

**But it does not fit.** It is client/server: an always-on central scheduling server, clients
attached over websockets (`rustic-scheduler client --url ws://…`), backups triggered centrally.
This fleet is 7 intermittently-online personal machines. It is also marked *"early development
stage"*, last pushed **2024-11-30**, 14 open issues.

### The conclusion

The genuinely unfilled gap is **local, per-machine OS-level scheduling for rustic, with per-host
config variation and no central server.** That is what resticprofile does well, what rustic
lacks, and what this tool is for. Everything else is delegated.

---

# Part 2 — Discoveries worth keeping

These were expensive to find. A fresh session should not re-derive them.

## 2.1 Four bugs in the live resticprofile setup, all silent

Found while working in the fork earlier the same day. Full detail in
`~/Sync/git/resticprofile/UPSTREAMING.md` Appendix B. Summarised because they justify specific
design decisions here.

| Symptom | Root cause |
|---|---|
| Retention matched **zero** snapshots for months | `config/profile.go:262` copies `backup.source` into retention's `--path` filters. restic requires a snapshot to contain **all** listed `--path` values. Verified: 6 paths → 0 matches; the 4 real paths → 4 matches. |
| Retention never ran at all; **2810 snapshots** accumulated under a policy that should cap ~49/host | Two source paths (`.gnupg`, `.local/share/nushell`) do not exist on this host. restic prints "does not exist, skipping" and **exits 3**. resticprofile treated that as failure and aborted the profile before `retention.after-backup`. Reproduced deterministically: without those paths exit 0; with them exit 3. |
| Config failed to load with `missing value for if` | The whole file is a Go template rendered *before* YAML parsing, so a literal `{{ if }}` inside a `#` **comment** is still compiled. |
| `.Env` scanning silently disabled | The fork's `envscanner` (`envscanner/scanner.go:22-28`) reads the **raw** file with a plain YAML parser — it must, because it regex-scans for `.Env.X` before templating substitutes them away. A structural `{{ if }}` block is therefore invalid YAML to it. |

Every one degraded silently rather than failing. For a backup tool that is the important failure
class, and it is the core motivation for "validate loudly at load time" below.

## 2.2 The `gcs:` block in the live config has never done anything

Independently verified. In `~/.config/resticprofile/profiles.yaml` the `default` profile carries:

```yaml
    gcs:
      project-id: "example-project-000000"
      credentials-file: "..."
      connections: 10
```

`config/flag.go:110` calls `stringifyValueOf` with `onlySimplyValues = true`; the `reflect.Map`
arm at `flag.go:215` then returns `("ERROR: unexpected type map", false)`, and `ok == false`
means `addArgsFromMap` adds nothing. Even if rendered, `--gcs=…` would be stripped again by
`validArgumentsFilter` (`wrapper.go:302-331`) because `gcs` is not in `commands.json`.

**So `connections: 10` has never taken effect.** Credentials work only because they are *also*
set in the `env:` block. The correct restic mechanism would be `-o gs.connections=10`.

This is the single best argument for making unknown keys a **hard error** rather than silently
dropping them.

## 2.3 resticprofile's shell dependency is the source of its worst complexity

`shell/command.go:88-94` gets a shell and calls `exec.CommandContext(shell, args...)`;
`composeShellArguments` flattens every argument into one string for `sh -c`. That single decision
is the entire reason for the 8-variant `ArgType` matrix, `escapeNoGlobCharacters`,
`doubleQuotePattern`, `quoteArgument`, and a literal `""` for empty values — ~1,364 LOC plus
1,391 LOC of tests, and the highest-risk area in the project (get it wrong and you either corrupt
the command or leak passwords into logs).

**Design consequence: never use a shell.** Direct `std::process::Command` with a
`Vec<OsString>` argv deletes that whole class of bug. Glob patterns like `**/node_modules` reach
the child literally; `--tag=` empty values are just one argv element; `source-relative` becomes
`Command::current_dir()`. What remains is masking secrets in *log output* — one pure function
over an already-built argv.

## 2.4 Naming and prior art

| Name | Status |
|---|---|
| `rusticprofile` | **free** — no crates.io crate (404), zero GitHub repos |
| `rustic-profile` | free |
| `resticprofile` | free on crates.io, taken on GitHub (19 repos incl. upstream) |
| `rustic-rs` | taken — the active restic-compatible replacement, v0.11.3, updated 2026-06-03, 41.7k downloads, 3151 stars |
| `rustic-backup` | taken — *"Restic wrapper for convenient backups"*, i.e. this exact idea, **abandoned**: v0.2.1, 3 releases, last updated **2020-08-23** |

Naming caveat: `rustic` is a restic *replacement*, so "rusticprofile" reads as "profiles for
rustic". With rustic chosen as the backend that is now **accurate**, not misleading — it only
would have been misleading under the earlier restic-only design.

## 2.5 Scaffolding conventions (from `~/git/etr` and `~/git/retch`)

Both are the same author's, GPL-3.0, `just`-driven, and explicitly keep a synced convention
layer (`AGENTS.md` Part 1 "Portable Core" says so). **Model on `retch`** — 456 commits vs 134,
and its `AGENTS.md` is the superset that `etr` was synced *from* — but take **`edition = "2024"`**
and the `Cargo.toml` **`exclude`** list from `etr`.

The house standard:

- **`just` is the only task runner.** `Justfile` (capital J in retch), SPDX header,
  `set positional-arguments := true`, XDG paths precomputed as backtick variables,
  `default: @just --list`, a `#` doc comment above **every** recipe, heavy logic in
  `#!/usr/bin/env bash` + `set -euo pipefail` shebang recipes.
- **The `check` / `pr` / `open-pr` / `merge-pr` triad.** `check` = fmt-check + clippy
  `-D warnings`. `pr` is an interactive hard-fail gate: feature branch only → version bumped past
  `git describe --tags --abbrev=0` → `NOTES.md` has a header matching the version → man page
  builds and is diff-clean → `cargo check` leaves `Cargo.lock` clean → `just check` → tests →
  manual checklist requiring an interactive `y`. `merge-pr` = `gh pr merge --squash
  --delete-branch` then main/pull/branch -D and a `WIP.md` update.
- **Deliberately absent, and this absence is the convention:** no `rustfmt.toml`, `clippy.toml`,
  `deny.toml`, `rust-toolchain.toml`, `CHANGELOG.md`, MSRV declaration, `[lints]` table,
  `#![deny(...)]`.
- **Docs:** `NOTES.md` *is* the changelog and living state (`## Current State (vX.Y.Z)`,
  reverse-chronological). `AGENTS.md` Part 1 portable core + Part 2 project-specific, with a
  2-line `CLAUDE.md` pointer. Gitignored, Syncthing-synced `WIP.md` for cross-machine handoff.
  Community-health set: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, PR template, 2
  issue templates. A GitHub wiki treated as a PR checklist item. README carries an explicit
  "vibe coded — real programmers welcome" disclosure.
- **Version bump on every PR** (patch for fixes/docs/tests, minor for features); tag `v$VERSION`
  from clean `main`; never `cargo publish --allow-dirty`. Branches `{feature,fix,chore}/<name>`.
  Commits imperative ≤50 chars with an **`Assisted-By: <model name>`** trailer (not
  `Co-Authored-By:`).
- **Deps house style:** `clap` 4 + derive, `clap_complete` (+nushell), `serde` + derive,
  `toml`/yaml, `dirs`, `anyhow`, `owo-colors`, `criterion` with `[[bench]] harness = false`,
  `[profile.profiling] inherits = "release", debug = true`. **No `tracing`/`log`**, no `figment`,
  no `thiserror`.
- **Layout:** lib + thin bin, `src/lib.rs` as a documented module index, two-line SPDX header on
  every file, unit tests inline as `#[cfg(test)] mod tests`, **one** `tests/cli_tests.rs` using
  `env!("CARGO_BIN_EXE_…")` — no `assert_cmd`.
- **Six-shell completions** generated from the binary; man page authored in Markdown and built
  with **`mandown`**.
- **CI:** GitHub Actions, `dtolnay/rust-toolchain@stable` (stable only), OS/arch matrix incl. ARM,
  SHA-pinned actions in retch, weekly `cargo audit` cron, dependabot, tag-triggered multi-platform
  release with release pruning. Real git hooks in `scripts/hooks/{pre-push,post-merge}` installed
  by `scripts/install_hooks.sh` — the documented rationale is *"prefer real git hooks and Justfile
  recipes over anything under `.claude/`, because `.claude/` only binds Claude Code."*

`retch` additionally has `.cargo/audit.toml` (with justified ignores), `.gitattributes`
(`* text=auto eol=lf`, to stop Syncthing-across-OSes phantom diffs), and a `flake.nix`.

## 2.6 Environment facts

- **`~/git` is a symlink to `Sync/git`.** So `~/git/rusticprofile` **is**
  `/home/user-a/Sync/git/rusticprofile`. `~/Sync` is Syncthing-synced across machines. Note
  `stat` on `~/git` shows the symlink's own inode unless `-L` is passed — this caused confusion
  earlier.
- **Toolchain present:** cargo 1.97.1, rustc 1.97.1, just 1.57.0, `mandown`, `cargo-audit`,
  `hyperfine`, `gh`, restic 0.19.1, **rustic 0.11.3** (already installed, and the latest release).
  `cargo-nextest` is *not* installed and is *not* used by these projects.
- **The fleet:** 7 hosts share one GCS repository `gs:example-backup-bucket:/dot-files`, 2810
  snapshots, spanning 2025-08-21 → present.

  | Host | Snapshots | Last seen | Platform |
  |---|---|---|---|
  | host-a | 1407 | active | Linux |
  | host-b | 952 | active | Linux |
  | host-c | 364 | 52d idle | Linux |
  | host-d | 55 | active | Linux — **designated prune host** |
  | host-e.local | 25 → **23** | **cut over to rusticprofile 2026-08-03** | macOS |
  | host-f | 4 | active | Linux (this machine, reinstalled 2026-07-22) |
  | host-g.local | 3 | 269d idle | macOS, home is `/Users/user-b` |

  `host-h` (2 snapshots) was forgotten by explicit ID on 2026-07-30.

  **The counts above are the 2026-07-30 census and are no longer current** — the repository
  held 687 snapshots after `host-e.local`'s cutover. Two rows have moved since: `host-f` was
  cut over on 2026-08-01, and `host-e.local` on 2026-08-03 (+3 backed up, −5 forgotten, other
  hosts unchanged at 664, verified from the repository). The table is kept as the census it
  was, because the *ratios* are what the retention findings rest on; read it as history.
- **Paths differ across the fleet:** `/home/user-a`, `/Users/user-a`, `/Users/user-b`. 16
  distinct path-sets exist in the repo. This is *why* copying backup sources into retention
  `--path` filters is dangerous here.
- Repository access needs `GOOGLE_PROJECT_ID` and `GOOGLE_APPLICATION_CREDENTIALS`, plus
  `~/.config/resticprofile/gcs.example.dot-files.pw.txt` as the password file.
- **Opening upstream PRs requires `ghpub`, not `gh`** — the default `GH_TOKEN` is a fine-grained
  PAT and only *classic* PATs can write to repos you do not own. See `~/AGENTS.md` §3.

---

# Part 3 — Decisions

## Locked

1. **Local scheduler/orchestrator.** Owns scheduling, per-host variation, run sequencing, lock
   coordination and exit classification. Backup configuration is **delegated to rustic**.
2. **rustic only.** No *backup-configuration* flags — invoke `rustic -P <profile> <op>`,
   plus `--name` per snapshot set and `--json` on `backup`. *(Amended v0.0.5: `--json` is an
   output-format flag, not a setting. Without it, exit classification would have to match
   English text in a log, because rustic exits 1 for both a partial and a failed backup. It
   configures nothing about the backup itself, and progress still reaches stderr.)*
3. **Milestone 1 = one run end-to-end** (config → invoke rustic → classify exit → chain forget).
   No scheduling in M1.
4. **Name `rusticprofile`.**

## Rejected, and why

| Rejected | Why |
|---|---|
| Full resticprofile config compatibility | Requires reimplementing viper, `text/template` and HCL first. That is the majority of the project before any backup logic. |
| Reading the existing `profiles.yaml` directly | Inherits the implicit coupling that caused three of the four bugs. |
| A `migrate` subcommand | Was in an earlier draft. Dropped when config moved to rustic's own TOML — rustic's config is small enough to hand-write once per fleet, and a migrator to *someone else's* format is not this tool's job. |
| restic as a backend | Needs a flag catalog, which is the one piece that could not be made independent of resticprofile (`restic/commands.json` is generated by *their* Go tool from restic's man pages). |
| Vendoring `restic/commands.json` | Same reason. It was in the earlier draft and was the main thing making the design derivative. |
| Using resticprofile's `--dry-run` as a golden-test oracle | Same reason — an independence leak. It survives only as a *one-off cross-check* in the verification ladder, never as a test fixture. |
| `rustic_scheduler` | Client/server with an always-on central server; wrong architecture for intermittently-online laptops. Early-stage, 20 months stale. |
| Contributing to `rustic-backup` | Abandoned since 2020. |

## Independence check

Nothing in the final design is derived from resticprofile: no vendored data files, no ported flag
catalog, no test oracle, no config compatibility. The only inheritance is *design knowledge* about
which failure modes to make impossible.

---

# Part 4 — The plan

## Division of responsibility

| Concern | Owner |
|---|---|
| Repository, sources, excludes, forget policy, hooks, metrics, env | **rustic** (`rustic.toml`) |
| **Secrets** — repository password, master key, cloud credentials | **rustic** — see §4.1 |
| Not leaking those secrets into logs, argv or a process list | **rusticprofile** |
| Which jobs exist, when they run, on which hosts | **rusticprofile** |
| Which named `[[backup.snapshots]]` sets run on this host | **rusticprofile** (Part 7 / option B) |
| Validating that every emitted `--name` exists in `rustic.toml` | **rusticprofile** — read-only, see below |
| systemd units / launchd plists; install, remove, status | **rusticprofile** |
| Per-host variation (rustic's TOML has no hostname conditionals) | **rusticprofile** |
| Operation sequencing, exit classification, lock coordination | **rusticprofile** |
| restic flag construction, version catalog | **nobody — does not exist** |

## Config schema

Small and job-oriented, because rustic holds the backup detail:

```yaml
# ~/.config/rusticprofile/jobs.yaml
schema: 1

defaults:
  rustic-binary: rustic
  rustic-config-dir: "${env:HOME}/.config/rustic"

jobs:
  dot-files:
    profile: dot-files              # -> rustic -P dot-files
    operations: [backup, forget]    # ordered; forget runs if backup succeeded OR warned
    snapshot-sets:                  # -> one `--name <set>` per resolved entry (Part 7, option B)
      - name: core                  # runs everywhere
      - name: gnupg
        enabled-on-hosts: [host-a, host-b]
      - name: nushell
        enabled-on-hosts: [host-a, host-d]
    schedule:
      at: hourly
      permission: user              # user | system
      priority: background          # -> Nice= / IOSchedulingClass= in the unit
    log: "${config_dir}/logs/${job}-${date:%Y-%m-%d}.log"

  dot-files-prune:
    profile: dot-files
    operations: [prune]
    enabled-on-hosts: [host-d]       # replaces {{ if eq .Hostname "host-d" }}
    schedule:
      at: weekly
      permission: user
      priority: background
```

Design rules, each closing an observed failure mode:

- **`enabled-on-hosts` removes the job entirely** on other hosts — it does not render a schedule
  expression to the empty string, which is what the template gate it replaces did. Inspectable
  via `--as-host`; a CI test renders the shipped config for all 7 hostnames and asserts exactly
  one produces a prune job. The template gate never had such a test.
- **Parse YAML first, interpolate second.** Order: read → parse YAML → host gating → `${…}`
  interpolation → batch validation. Comments are discarded before substitution, so the two
  templating traps are structurally impossible.
- **Interpolation is not a language.** Closed set only: `${env:NAME}` (unset ⇒ error),
  `${host}`, `${host_short}` (matters — `host-e.local`), `${job}`, `${profile}`, `${config_dir}`,
  `${temp_dir}`, `${os}`, `${arch}`, `${date:%Y-%m-%d}` (evaluated per run, never baked into a
  unit file), `$${` for a literal. No conditionals, functions, pipelines or loops. Anything else
  is a load-time error naming the key and listing valid names.
- **Every emitted `--name` is checked against `rustic.toml` at load time.** rustic silently ignores
  an unknown `--name` as long as at least one *valid* name is also present (§7.2) — so a typo, or a
  set renamed in `rustic.toml`, would quietly back up less than intended and still exit 0. Load
  resolves `<rustic-config-dir>/<profile>.toml`, reads only the `name` field of each
  `[[backup.snapshots]]` entry, and reports every unknown name at once with the list of valid ones.
  A missing or unparseable `rustic.toml` is itself exit 2, naming the path tried. This is the one
  place rusticprofile reads rustic's config, it is strictly read-only, and it exists precisely
  because the delegated tool fails silently here.
- **A job whose `snapshot-sets` resolve to empty on a host is a load-time error**, never a silent
  no-op run. If the intent is "this job does not run here," that must be spelled out with a
  job-level `enabled-on-hosts`, which removes the job entirely. Same principle as the gate above:
  doing nothing must always be something the config *says*, not something it *becomes*.
- **Validation is batched** — all violations at once, exit 2, before any process spawns. Exit 2
  keeps "config is wrong" distinguishable from "backup failed".

Four more rules emerged while implementing this (M1 step 2, v0.0.2). Same family — each one
turns a config that would quietly do less than it says into a load-time error:

- **Snapshot-set names are validated as *declared*, not as *resolved*.** Checking only the sets
  that survive gating means a name mistyped inside a set gated to another machine is caught
  only on that machine — the one place nobody is running `--check`. All declared names are
  checked everywhere.
- **`enabled-on-hosts: []` is refused**, for a job or a set. An empty list means "exists, runs
  nowhere", which is a no-op wearing a disguise; deleting the entry says the same thing out loud.
- **Log paths must be absolute** after interpolation. A scheduled unit has no predictable
  working directory, so a relative path writes somewhere nobody will look for it.
- **`${job}` and `${profile}` are errors inside `defaults`**, where they have no value. Resolving
  them to the empty string there is how a path silently loses a component.

A fifth arrived with M1 step 3 (v0.0.3), found by a test that was asserting the wrong thing:

- **A snapshot-set name may not start with `-`.** Each name becomes its own argv element after
  `--name`, so a set called `--password` would put that string on a rustic command line —
  argument injection by config. rusticprofile constructs no flags of its own beyond `-P`,
  the operation and `--name`, and this rule is what keeps that true regardless of what the
  config says. There is a test asserting those are the *only* flags any built argv contains.

Host matching accepts the short form as well as the full name, so `host-e` selects `host-e.local`.
Exact-match-only would turn a reasonable-looking config into a job that silently never runs.

## 4.1 Secrets

**rusticprofile holds no secrets and has no secret configuration.** rustic already offers
three shapes for each one, and the third is the one to prefer:

| Secret | Mechanisms |
|---|---|
| Repository password | `--password` (env `RUSTIC_PASSWORD`), `-p/--password-file`, **`--password-command`** (env `RUSTIC_PASSWORD_COMMAND`) — read from the command's stdout |
| Master key | `--key`, `--key-file`, **`--key-command`** |
| Cloud credentials | `[repository.options]` or `OPENDAL_*` env vars. **No `-o` CLI flag exists** (§5.1) |

**Verified 2026-07-31** against rustic 0.11.3, in a throwaway local repository: a
`password-command` declared in `[repository]` of a `rustic.toml` works, including when it
points at a script standing in for a secret-store lookup, and a *wrong* password fails —
so the command's output is genuinely used rather than silently ignored.

That is the seam worth preferring, because **rustic spawns the command itself**. The secret
never enters rusticprofile's memory, argv or environment, so there is nothing for redaction
to get wrong. Redaction (M1 step 4) remains, but as a backstop rather than the primary
control.

Two hard rules follow:

- **rusticprofile never emits `--password`, `--key`, or any other secret-bearing value into
  an argv.** rustic's own help warns that `--password` "can reveal the password in the
  process list", and a process list is world-readable. Since rusticprofile constructs no
  flags beyond `-P`, the operation and `--name`, this is enforceable by construction and is
  asserted by a unit test over every built argv rather than left to review.
- **No shell, ever** (§2.3), which is what keeps a secret from being composed into a command
  string in the first place.

### On secret managers

Mechanically, any of them can sit behind `password-command`. For *this* fleet the useful
distinction is whether the store works **unattended**, since backups run from timers with
nobody logged in to approve anything:

- **gnome-keyring via `secret-tool lookup`** — local, no network, already the store this
  machine's browser and XDG portal use, and `secret-tool` is installed. Fits a **user**
  timer once the keyring is unlocked at graphical login; **not** a system unit.
- **Proton Pass (`pass-cli`)** — *rejected for the scheduled path.* `pass-cli login --pat`
  does give non-interactive auth, but the PAT then has to live somewhere machine-readable,
  which relocates the secret rather than removing it; `~/AGENTS.md` reaches the same
  conclusion for `ghpub`. It also puts a network round-trip and a session-expiry failure
  mode in front of every backup — a bad trade on the availability side of a backup tool.
  Keep it for interactive human workflows.
- **A plain `0600` file** — what the GCP service-account key should stay as. opendal's
  `OPENDAL_CREDENTIAL_PATH` wants a *path*, so sourcing it from a secret manager would mean
  materialising plaintext JSON to a temp file on every run, which is strictly worse.
  (opendal's gcs service also has an inline `credential` option; whether rustic accepts it
  through `[repository.options]` is **unverified** — test before relying on it.)

## Architecture

Single crate, lib + thin bin, **no workspace**. `retch` is a workspace only because
`retch-sysinfo` is separately publishable, and that costs ~40 lines of
`crates_io_has_version.py` gymnastics in its publish recipes to work around an unresolvable
`=0.1.49` pin during dry-run. `etr` (newer, edition 2024) is a single crate. There is no second
consumer here.

```
src/
  lib.rs        M1  documented module index
  main.rs       M1  parse -> dispatch -> Report -> process::exit
  cli.rs        M1  clap derive
  config/{mod,job,hosts,interp,validate,paths,schedule}.rs   M1 (schedule parsed, unused until M2)
  config/rustic_toml.rs                                      M1  read-only: names from rustic.toml
  rustic/{mod,invoke,version,exit}.rs                        M1  build `rustic -P …` argv; classify exit
  exec/{mod,env,redact,outcome}.rs                           M1  spawn, mask secrets in logs
  run/{mod,steps,lock}.rs                                    M1  ordering; LockBudget seam
  report.rs     M1  owo-colors output
  schedule/{mod,calendar,systemd}.rs                         M2
  schedule/launchd.rs                                        M3
  schedule/schtasks.rs                                       0.2.0 (Windows; §7.9)
```

**No shell, ever** — see §2.3 for why this is architecture rather than preference.

## Milestone 1 — one run, end to end

1. **Scaffold** per §2.5. *Verify:* `just check && just test` green; `just install-hooks`.
2. **Config** — all of `config/`. Lands `rusticprofile config --check [--as-host H]` (exit 0/2,
   all violations at once) and `config --show -n <job> [--as-host H]`, neither needing the rustic
   *binary*. `--check` does read `rustic.toml` for the snapshot-set name validation (§7.2); a
   `--no-rustic-config` escape is **not** provided, because skipping that check is what would let
   the silent-drop bug back in.
3. **Invocation planning** — `rustic/invoke.rs` builds argv as a pure function of (job, host).
   `rusticprofile plan -n <job> --format lines` prints one argv element per line. Golden-tested
   and hermetic: no rustic binary, no real hostname, no clock.
4. **Exec + redaction** — spawn, forward SIGINT to the child (`nix::sys::signal`).
   Redaction on by default over argv and logged env; `--show-secrets` warns to stderr first.

   *Amended during implementation (v0.0.4).* This step originally said "inherit stdio", which
   contradicts §5.8: classification needs the `--json` objects rustic writes to **stdout**, and
   inherited stdout cannot be read. The measurement resolves it — progress and diagnostics go to
   **stderr**, snapshot objects to **stdout** — so `exec` **captures stdout and inherits stderr**.
   The operator watches progress live and step 5 still gets its machine-readable output. Also
   settled here: an interrupt is forwarded to the child and then *waited on*, never exited
   through, because orphaning a running rustic leaves a lock held on the shared repository.
5. **Exit classification** — `rustic/exit.rs`. **Characterise rustic's actual exit codes
   empirically** (Part 5); do not inherit restic's table. Exit 1 is ambiguous, so classify on
   requested-name count vs. snapshot objects parsed from `--json` stdout (§5.8), not on the code
   and not on log-text matching. A warning must **not** abort the job, so `forget` still runs —
   the structural fix for the exit-3 chain.
6. **Runner** — local `flock`, ordered operations, stop on failure but continue on warning, emit
   a report. `run/lock.rs` defines a `LockBudget` seam returning `None` in M1, documented as
   deferred rather than faked.
7. **CLI** — `run -n <job>`, `--config`, `--rustic-binary`, `--dry-run`, `--as-host`,
   `--completions <6 shells>`. Exits: 0 success/warning, 1 run failure, 2 config error, 130
   interrupted.

### Verification ladder against the live 2810-snapshot repo

Strictly ordered; each rung provably safe.

| # | Action | Risk |
|---|---|---|
| 1 | `plan -n dot-files --format lines` — inspect argv only | none, nothing spawned |
| 2 | `--rustic-binary ./scripts/shim.sh` — recording shim logs argv + filtered env, exits 0 | none, rustic never runs |
| 3 | Read-only rustic against the repo: `snapshots`, `repoinfo`, `check` | none |
| 4 | `run --dry-run` → rustic's own dry-run; reads sources, writes nothing | none |
| 5 | **`forget --dry-run`** scoped to this host; diff the would-remove ID set against resticprofile's `forget --dry-run` | none — the only way to prove retention scoping without deleting |
| 6 | Full write path against a **throwaway local repo** in a temp dir | isolated |
| 7 | Real backup to the real repo — additive, worst case one extra snapshot | negligible |
| 8 | Real `forget`, prune disabled, from **host-d only** (55 snapshots — smallest active host, and the designated prune host) | first irreversible step, smallest blast radius |
| 9 | Roll out to host-a (1407) and host-b (952) only after 8 confirms | as documented |

Two hard rules: **`prune` is never run against the GCS repo before the lock work lands**, and the
four idle hosts are untouched during M1 — they are the control group.

## Later milestones

**Milestone status is maintained in `NOTES.md`, not here.** As of 2026-08-04: **M1, M2, M3
and M5 complete; M4 deferred by decision; M6 effectively delivered** (man page, six-shell
completions and crates.io all shipped by `v0.1.31`). The entries below are the *specifications
as written on 2026-07-30*, kept for the reasoning in them.

- **M2 — systemd scheduling. Complete (0.0.18–0.0.19).** `schedule`/`unschedule`/`status`; `at:` subset → `OnCalendar=`;
  `permission` → user vs system unit dir; `priority` → `Nice=`/`IOSchedulingClass=` **in the unit
  file**, which means in-process priority/ionice code is never written. Covers 5 Linux hosts.
  `status` must surface the `enabled-on-hosts` gate so "host-d has a prune timer and nobody else
  does" is inspectable.
- **M3 — launchd. Complete (0.1.26–0.1.27).** `~/Library/LaunchAgents`,
  `StartCalendarInterval`, `launchctl bootstrap gui/$UID` — all three as sketched. Covers
  host-e.local and host-g.local. Done after M2 because 5 hosts > 2.

  **The vocabulary was shared, as predicted, and four platform differences were not.** Each
  was measured on macOS 26.6 rather than reasoned about, and each is a property of launchd:

  | | systemd | launchd |
  |---|---|---|
  | files per job | **2** — a timer cannot run a command | **1** — schedule and program in one job |
  | fleet spread | `RandomizedDelaySec=` | no equivalent; the offset is a real minute inside `StartCalendarInterval`, chosen once and reused |
  | missed run | `Persistent=true` | free for **sleep** (documented, coalesced into one run); **not** caught up if the agent was unloaded |
  | runs with nobody logged in | `linger` | **no equivalent** — a user agent needs a login session; `permission: system` (LaunchDaemon) is the only way |
  | next fire time | `NextElapseUSecRealtime` | **not reported at all**, so `next_run` is `null` |

  The absolute-path requirement from `0.1.10` carries over unchanged and for the same reason:
  a launchd agent gets `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and `PWD=/` (measured), so a
  Homebrew rustic is as invisible to it as a cargo-installed one is to a systemd user manager
  started at boot.

  **The login limitation is the one that matters operationally**, and it is not a gap that can
  be closed by more code: a Mac sitting at the login window takes no backups, nothing fails,
  and the only evidence is an absence. That is why `schedule` and `status` both state it, and
  why `last_success` is the field to alert on there even more than on Linux.
- **M4 — lock coordination. NOT BUILT, and deliberately so.** `LockBudget` would implement a
  wait budget, execution-time crediting and stale-lock handling.

  > **The paragraph that stood here was wrong, and is preserved below because the error cost
  > real time.** It said M4 was the precondition for any `prune` against the shared
  > repository. §7.6's own correction, and `NOTES.md` `0.1.3`/`0.1.11`, establish the
  > opposite: **rustic is lock-free by design**, `prune` defers deletion by `--keep-delete`
  > (23 h), and the only unsafe combination is `restic prune` against a rustic writer.
  > Prune returned to the designated host as a `rustic prune` on 2026-08-03. **M4 is defence
  > in depth, not permission.**

  *Superseded text, 2026-08-02:* "Reclassified from prospective to load-bearing (§7.6): rustic
  takes no repository lock at all, so the moment a host was cut over it became invisible to
  the prune scheduled on `host-d`. That prune is now disabled, and M4 is what has to land
  before it can come back — until then nothing reclaims space."

  What survives unchanged: if M4 is ever built, the exclusion must be written in **restic's
  own `locks/` format**, since coordinating only rusticprofile instances would still leave
  the predecessor and hand-run `restic` outside it. A fleet running one tool needs none of it.
- **M5 — observability. Complete (0.1.17–0.1.23).** Log targets (`O_APPEND`), status file, `--json`.
- **M6 — polish and publish. Effectively delivered.** Man page via `mandown`, six-shell
  completions, crates.io via `just publish-check` → `just publish` — all shipped; `v0.1.31` is
  on crates.io. Never formally closed as a milestone, because it had no single landing point.
  The one outstanding piece is the AUR package, blocked on their maintenance window.

## Dependencies

`clap` 4 + derive, `clap_complete` + `clap_complete_nushell`, `serde` + derive,
**`serde_yaml_ng`** (dtolnay archived `serde_yaml` in 2024; `just audit` is a first-class gate and
an unmaintained advisory would turn it red for unrelated reasons — `serde_norway` is the fallback),
**`toml`** (read-only, to enumerate `[[backup.snapshots]]` names from `rustic.toml` for the §7.2
validation — rusticprofile never writes it), **`serde_json`** (streaming parse of `rustic backup
--json` output for exit classification, §5.8),
**`jiff`** (`${date:}` strftime *and* duration parsing in one maintained crate, versus `chrono` +
`humantime`), `anyhow` plus one hand-written `ValidationErrors(Vec<Violation>)` with a manual
`Display` so all config errors print together, `dirs`, `owo-colors`, `nix` (flock, SIGINT
forwarding, `gethostname` — already in etr's vocabulary, so no micro-crates), `semver` (rustic
version checks), `criterion` + `tempfile` dev-only.

Deliberately not used: `tracing`/`log`, `figment`, `config`, any template engine,
`assert_cmd`/`predicates`, `thiserror`.

## Testing

Inline `#[cfg(test)] mod tests` for interpolation (each variable, `$${` escape, unknown-name
error, comment invisibility), host gating (override merge, `enabled-on-hosts` removing a job, the
7-hostname matrix), validation batching, exit classification, redaction.

Three more that exist because of the Part 7 decision, all cheap and all guarding a silent failure:
snapshot-set gating per host (including the "all sets resolve away ⇒ error, not empty run" case);
`--name` validation against a fixture `rustic.toml` (unknown name rejected, all unknowns reported
at once); and parsing a two-object `--json` stdout capture with the objects **concatenated, not
newline-separated** (§5.8) — a fixture that a line-based parser fails.

One `tests/cli_tests.rs` via `env!("CARGO_BIN_EXE_rusticprofile")` — no `assert_cmd`. Covers
`--help`/`--version`/6 completions, `config --check` exit 0 vs 2 per fixture, the golden runner,
exit-code assertions, and a real-repo smoke test skipped with a printed notice if `rustic` is
absent.

Golden argv files under `tests/golden/`, one element per line so diffs are readable, regenerated
by `RP_UPDATE_GOLDEN=1 just golden`, with `just check` failing if any golden file is dirty — the
same discipline as retch's man-page check.

## Risks and non-goals

**Risks.** (a) **rustic behaving differently from restic** against a repo holding 2810 real
snapshots — addressed by the verification ladder and gated by the prerequisites in Part 5.
(b) **`forget` is irreversible** — mitigated by a hard invariant that a `forget` resolving to no
host/path/tag filter is **refused at load time**, because "forget across every host in a 7-host
shared repo" must be spelled out, never defaulted. (c) **Migrating 7 machines** — managed by
keeping Go resticprofile installed and authoritative through M1–M3 and rolling out host-d-first.
(d) **Two tools sharing one repository is itself the risk**, and it materialised twice in two
days: as competing retention (§7.5) and as competing locks (§7.6). Both were configurations
that were individually correct and only wrong in combination, and neither was detectable from
inside rusticprofile — which is the argument for the `doctor` command rather than for more
validation rules. Assume the next one exists and has not been found yet.

**Non-goals for v1.** Reading resticprofile config; a `migrate` command; restic as a backend;
~~Windows; crond/schtasks~~ — **REVERSED 2026-08-06 for Windows; see §5.10 and §7.9**; groups;
**restore** (use rustic directly — say so in the README's first
screen); hooks (rustic has them); Prometheus/metrics (rustic has them); templating in any form,
including a "just one small conditional" escape hatch.

*Struck rather than deleted, per this file's convention. What changed is not the reasoning but the
fleet: the development machine now runs Windows, so "the scheduling backends are systemd and
launchd" stopped describing the machine this work is done on. Task Scheduler is in scope as a
third backend; **`crond` remains a non-goal**, and so does restic-as-a-backend.*

**No compatibility promises** with resticprofile — not the schema, CLI, exit codes or output. The
name is a lineage marker, not a compatibility claim, and the README must open by saying so.

---

# Part 5 — Prerequisites: RESULTS (tested 2026-07-30, rustic 0.11.3)

All six answered. GCS access is **cleared**; one result invalidates part of the design and is
flagged as an open decision in Part 7.

## 5.1 Repository URL form — `opendal:gcs`, not `gs:`

`gs:example-backup-bucket:/dot-files` fails: *"The backend type `gs` is not supported."* Only four
schemes exist (`rustic_core/crates/backend/src/choose.rs:158` `SupportedBackend`): **`local`,
`rclone`, `rest`, `opendal`** — no restic-compatible aliases.

For opendal, the location after `opendal:` is split on `:` and its first element is the opendal
*service* name (`crates/backend/src/opendal.rs:147`), so the repository string is just
`opendal:gcs`. **All service parameters come from options, and there is no `-o` CLI flag** — they
are config-file only (`[repository.options]`), or `OPENDAL_*` environment variables.

Verified working:

```bash
rustic -r opendal:gcs -p <password-file> repoinfo
# with:
OPENDAL_BUCKET=example-backup-bucket
OPENDAL_ROOT=/dot-files
OPENDAL_CREDENTIAL_PATH=$GOOGLE_APPLICATION_CREDENTIALS
```

This **validates the delegation model**: repository access must go through rustic's own config,
so rusticprofile owning it was never an option.

## 5.2 Reading the existing restic-format GCS repo — YES

```
[INFO] repository opendal:gcs:example-backup-bucket: password is correct.
| Snapshot |   536 |  444.9 KiB |
| Pack     |   563 |    5.6 GiB |
```

The go/no-go is cleared. rustic reads the repo restic has been writing since 2025-08.

## 5.3 Exit codes — coarse, and no warning tier

| Case | rustic exit |
|---|---|
| `init`, `backup`, repeat `backup`, `snapshots`, `forget --dry-run` | 0 |
| Backup with a **nonexistent** source | **1** |
| Backup where *all* sources are nonexistent | **1** |
| Wrong password | 1 |
| Missing repository | 1 |

Everything that is not success is `1`. There is no equivalent of restic's 0/1/2/3/10/11/12.
**Consequence:** `rustic/exit.rs` cannot classify on the exit code alone — it must inspect
stderr, or use structured output if available. Distinguishing "wrong password" from "backup
failed" matters for whether a retry is sensible.

## 5.4 Backup does not chain forget

`rustic backup` has `--delete-never` / `--delete-after <DURATION>` (marking a snapshot's
lifetime), but nothing that runs `forget` afterwards. `rustic forget --prune` *does* chain prune.
**rusticprofile owns backup→forget sequencing**, as designed.

## 5.5 Forget filters — different names, same traps

Flags: `--filter-host`, `--filter-label`, `--filter-paths`, `--filter-paths-exact`,
`--filter-tags`, `--filter-tags-exact`, `--filter-after/-before/-size/-size-added/-last`, and
`--filter-jq`. Policy flags mirror restic (`--keep-hourly` etc., with `-1` meaning keep-all).

Two carry-overs that matter, both confirming the design is **not** restic-specific:

- **`--group-by` defaults to `host,label,paths`** — the same fragmentation that let 2810
  snapshots survive a policy that should cap ~49/host. rusticprofile must set it explicitly.
- **`--filter-paths` matches supersets; `--filter-paths-exact` is *"exactly (no superset) as
  given"***. The non-exact form is the same "snapshot must contain all these paths" trap that
  made retention match zero snapshots. The explicit `retention.scope` design carries over intact.

## 5.6 `--dry-run` — supported on `backup`, `forget` and `prune`

The verification ladder is viable end to end.

## 5.7 NEW: rustic is *stricter* than restic on missing sources

Not on the original checklist; found while characterising exit codes, and it is the most
consequential result.

| | restic 0.19.1 | rustic 0.11.3 |
|---|---|---|
| One source path absent | warns `does not exist, skipping`, backs up the remaining sources, exits **3** | `[ERROR] error backing up …: error sanitizing source`, **no snapshot is created at all**, exits **1** |

```
[ERROR] error backing up /tmp/x/data,/tmp/x/does-not-exist: error sanitizing source=...
error: Not all snapshots were generated successfully!
```

There is **no option to tolerate a missing source** — nothing in `rustic backup --help` or the
config reference (`--skip-if-unchanged` is unrelated).

This matters because the live config has two sources (`.gnupg`, `.local/share/nushell`) that are
absent on this host and present on others. Under restic + `no-error-on-warning` that still
produced a snapshot. Under rustic it would produce **none** — strictly worse, and a silent
backup gap rather than a noisy one.

**It also breaks pure delegation.** If source lists live in `rustic.toml`, rusticprofile cannot
filter absent paths before invoking rustic. See Part 7.

## 5.8 `--json` is the right basis for exit classification — with two traps

Found 2026-07-30 while testing option B. Both matter for `rustic/exit.rs` and `rustic/version.rs`.

`rustic backup --json` writes **snapshot objects to stdout and diagnostics to stderr**, so "how many
snapshots were actually saved" is machine-readable and does not depend on matching the English
string `Not all snapshots were generated successfully`. Given §5.3 (everything non-success exits 1)
and §7.2 (partial success also exits 1), this is what makes exit 1 disambiguable at all:

> **requested-name count vs. snapshot objects on stdout** is the classification input. The exit
> code only says "not everything worked."

Trap 1 — **the objects are concatenated pretty-printed JSON with no separator and no newline
between them** (`}` immediately followed by `{`). It is *not* JSON-lines. A line-based count
(`grep -c '^{'`) returns **1** for a run that saved **2** snapshots — measured. Use a streaming
value parser (`serde_json::Deserializer::into_iter`), never line splitting.

Trap 2 — **`program_version` inside that JSON does not report the binary's version.** The binary is
`rustic 0.11.3`; every emitted snapshot object says `"program_version": "rustic 0.12.0"`. Version
gating must come from `rustic --version`, not from snapshot output.

## 5.9 rustic expands nothing in paths — and §5.7's strictness does not apply (2026-08-02)

Found while trying to write **one** `rustic.toml` for the whole fleet. It cannot be done,
and the way it fails is the dangerous part.

rustic expands neither `~` nor `$VAR` in `sources`, and there is no env-var route to the
host filter either:

| written | result |
|---|---|
| `sources = ["$HOME/Sync"]` | not expanded |
| `sources = ["~/Sync"]` | not expanded |
| `sources = ["${HOME}/Sync"]` | not expanded |
| `filter-hosts = ["$HOSTNAME"]` | literal string; matches **0 snapshots**, no error |
| `RUSTIC_FILTER_HOST`, `RUSTIC_FILTER_HOSTS`, `RUSTIC_SNAPSHOT_FILTER_FILTER_HOSTS` | all ignored |

**§5.7 does not save you here.** That section established that rustic hard-fails a whole
snapshot set when a source is missing. An unexpanded path does not take that route, because
`~/…` and `$HOME/…` are *relative* paths as far as rustic is concerned, and the sanitising
step that produces the hard error only applies to absolute ones. Measured, same repository,
same rustic 0.11.3:

```text
$HOME/Sync            -> [WARN] ignoring error … No such file or directory
                         processed 0 files, snapshot b7dafdf9 successfully saved, exit 0
/definitely/not/here  -> [ERROR] error sanitizing source=s"PathList([...])"
                         "Not all snapshots were generated successfully!"
```

So the config produces **a real, successful, 0-byte snapshot** whose recorded path is the
literal string `$HOME/Sync`. And by §7.3, that empty snapshot competes for the same
retention slot as the real one and, being newer, wins — the exact mechanism that already
destroyed a 395 MiB snapshot. A "portable" config of this shape does not fail to back up;
it replaces the backups with nothing, and reports success while doing it.

`filter-hosts = ["$HOSTNAME"]` fails in the other direction: it matches nothing, so
retention silently never runs. That is bug #1 from §2.1 reproduced exactly.

### Consequences

**One `rustic.toml` cannot be shared across hosts.** Both the hostname and the home path
must be materialised into the file, so the fleet-wide config has to be *generated* — one
chezmoi template, rendered per host. That is not a design failure to be fixed; it is what
rustic supports.

**`jobs.yaml` needs no templating at all.** It is genuinely portable: `${env:HOME}` is
resolved by rusticprofile's own interpolation, and `enabled-on-hosts` names hosts in a list
that is identical everywhere. **One byte-identical file on all seven hosts**, including the
prune-host gate. Worth stating plainly, because it is the difference between a config format
designed for a fleet and one designed for a machine.

**Corollary, found on macOS 2026-08-03: rusticprofile's own paths must not vary by OS
either.** The same argument that makes `jobs.yaml` fleet-portable makes a per-platform
config location wrong. `dirs::config_dir()` returns `~/Library/Application Support` on
macOS, so every command on a Mac looked for `jobs.yaml` somewhere the man page never
mentioned and exited 2 — while `${state_dir}` in the shipped `log:` would have resolved
under `~/.local/state` on the Linux hosts and `~/Library/Application Support` on the macOS
pair, from one byte-identical line. rusticprofile now applies the XDG rules on every Unix
(0.1.25). This is not a preference for XDG over Apple's conventions; it is the requirement
that one line of one file mean one thing across the fleet.

### REVERSED 2026-08-04 — rusticprofile now owns the hostname (`0.1.34`)

**The rejection below stood for four days and was wrong. It is kept in full because the
reason it was wrong is not that the reasoning was sloppy — it is that the reasoning answered
a question nobody was asking.**

It rejected emitting `--filter-host` on the grounds that *"the home path would still need
templating regardless, so it buys nothing."* That is true and irrelevant. It measures the
change against **saving a chezmoi template**, when the thing that actually matters is what a
user gets **with no configuration at all**:

| | recorded host, no config |
|---|---|
| Linux | `foo` |
| **macOS** | **`foo.local`** |

One fleet, two naming conventions, forever — and every filter, query and census has to know
which hosts are which. It produced a real false alarm on 2026-08-04, and it is the direct
cause of `0.1.4`'s `.chezmoi.hostname` / `.chezmoi.fqdnHostname` bug. A user not using
chezmoi has to hand-write the right hostname into two places and has no way to know that
`.local` will bite them.

**Measured 2026-08-04, and it is what makes the fix possible: for these flags the CLI
overrides the config file.** `--host from-cli` beat `host = "from-config"`, and
`--filter-host` beat `[snapshot-filter] filter-hosts`. (Note this contradicts Part 2's
"env > config > CLI" summary, which is wrong at least here.) So rusticprofile can supply
both and the answer stops depending on what any file says.

**Decision: rusticprofile emits the hostname.**

| operation | flag |
|---|---|
| `backup` | `--host <name>` — the only operation that accepts it |
| `forget`, `prune` | `--filter-host <name>` — scopes the destructive operations |
| `snapshots` passthrough | **nothing.** §7.8's rule holds: read-only, adds no flags |

`defaults.hostname` chooses the name: **`short`** (default — the OS hostname up to the first
dot), `full` (as the OS reports it), or `rustic` (emit neither and defer entirely, which is
the pre-`0.1.34` behaviour).

**Why an escape hatch, when this project's instinct is closed sets with no escape hatches:**
because the two cases it covers are data-integrity cases, not preferences.

1. **Changing the recorded name splits an existing repository.** Stored snapshots keep their
   old name; under `group-by = "host,label"` old and new are different groups, so the old one
   stops being selected and is never retained down again. It accumulates silently — this
   project's own failure class. `hostname: rustic` is the migration path.
2. **Short names collide across domains.** `web1.prod` and `web1.staging` both become `web1`,
   sharing one retention group and forgetting each other's snapshots — §7.5 breached by
   default. `hostname: full` is the answer there.

**What stops the change being invisible:** `config --check` states the name that will be
recorded and flags it when it differs from what the OS reports. A behaviour change that can
strand snapshots must be visible without reading the source.

**Consequences accepted deliberately:**

- **The delegation boundary moves, and the flag inventory grows** from `-P`/operation/`--json`/
  `--name` to include `--host`/`--filter-host`. The test asserting it is updated *after* this
  section, per its own instruction.
- **`check_forget_is_scoped` no longer requires `filter-hosts` in the profile** under `short`
  or `full`, because rusticprofile now supplies the scoping and a config that omits it is
  correct. Under `rustic` it still refuses, unchanged.
- **The `snapshots` passthrough becomes unscoped** if a profile drops `filter-hosts`. That is
  the honest cost of taking scoping out of the file: the fleet-wide view becomes the default
  and a per-host view needs `-- --filter-host <name>`. Not emitting it there is deliberate —
  `--filter-host` is repeatable and unions rather than overrides, so a flag we added would
  silently widen a caller's own filter rather than yield to it.

**Superseded by the above, kept as the record:** *"Rejected: emitting `--filter-host
<hostname>` from rusticprofile."* It would solve the
hostname half without templating, and it is the wrong trade. `-P`, the operation, `--name`
and `--json` are the only flags this tool emits, a test asserts exactly that, and the
delegation boundary is the thing that keeps this project from becoming the wrapper it was
written not to be. The home path would still need templating regardless, so it buys nothing.

**Accepted: refuse it at load time.** `check_sources_are_expanded` rejects any `sources`
entry containing `~` or `$` for a job that backs up. This is the same bargain as the
`--name` check in §7.2 — rusticprofile reads a file it does not own, because nothing else
in the chain can catch the mistake, and the mistake is silent. A tool whose stated purpose
is preventing silent degradation cannot let this particular one through.

## 5.10 What Windows actually breaks — measured 2026-08-06, rustic 0.11.3, rustc 1.97.1

Measured on the development machine after it was reinstalled from Fedora to Windows 11, rather
than reasoned about from the non-goal. The headline is that **rustic itself is fine on Windows**;
what needed work was smaller than expected in one place and larger in another, and two of the
findings are traps rather than gaps.

### The build: five sites, and `nix` is not one of them

`cargo check --all-targets` gave **15 errors in 5 places** — `gethostname`, `flock`,
the signal machinery, `getuid` for launchd's `gui/<uid>` domain, and the SIGPIPE reset.

**`nix` compiles on Windows.** It resolves and builds as an *empty* crate, so an unconditional
dependency entry is not itself an error — every use site fails instead, with
`could not find sys in nix`, which reads like a feature-flag mistake rather than a platform one.
It is now under `[target.'cfg(unix)'.dependencies]` so the error names the real cause.

### `%COMPUTERNAME%` is NOT the hostname, and using it would split the repository

This is the finding with teeth, because the wrong answer looks purely cosmetic:

| source | value on this machine |
|---|---|
| `%COMPUTERNAME%` (NetBIOS) | **`HOST-A`** |
| `hostname`, and `GetComputerNameExW(ComputerNamePhysicalDnsHostname)` | **`host-a`** |

Since `0.1.34` rusticprofile *records* this name. Emitting the upper-cased form would have done
two silent things: `hosts::host_matches` is plain equality, so `enabled-on-hosts: [host-a, …]`
would stop selecting the machine and its gated snapshot sets would simply not run; and under
`group-by = "host,label"` the host's existing history is a **different retention group**, so it
stops being selected and accumulates forever. That is §3a invariant 1 breached by a value nobody
would think to check. Lower-casing `%COMPUTERNAME%` was rejected — right here, and silently wrong
for a host genuinely named `Web1`.

### rustic cannot address a local repository by bare Windows path

`repository = "C:/Users/user/repo"` fails with:

```
The backend type `C` is not supported. Please check the given backend and try again.
```

§5.1 records that rustic splits the repository string on `:` and reads the first element as a
backend type. **A Windows drive letter is exactly that shape.** The fix is the explicit form,
verified working end to end — `init`, then `backup --json` producing a snapshot:

```toml
repository = "local:C:/Users/user/repo"
```

`opendal:gcs` is unaffected, so the fleet's production path never sees this. It bites local and
removable-drive repositories, and it bit the three rustic-backed integration tests.

### Backslashes: TOML rejects them, YAML mis-reads them, and both look like our bug

The same `\` -> `/` idiom the fleet's chezmoi template already applies, now with the two failures
it prevents measured:

| where | what a raw `C:\Users\…` does |
|---|---|
| TOML string | `TOML parse error at line 3, column 19` — `\U` is not a valid escape |
| YAML **double-quoted** scalar | `did not find expected hexadecimal number` — `\U` opens an 8-digit Unicode escape |
| YAML plain scalar | harmless; backslashes are literal |

Both surface as rusticprofile refusing a config, which reads as a defect in the validator.

### Two smaller platform facts, both honest gaps rather than bugs

- **`Path::is_absolute()` needs a drive or UNC prefix.** So `/var/log/x.log` is *relative* on
  Windows and `check_log_is_absolute` refuses it — correctly, since a driveless path resolves
  against whatever drive the scheduler chose. Consequence worth naming: a shared `jobs.yaml`
  with a rooted-but-driveless `log:` loads on the Unix hosts and is refused on a Windows one. The
  shipped example cannot hit it, because `${state_dir}` is always fully qualified.
- **`HOME` is normally unset**, so a `jobs.yaml` built around `${env:HOME}` — which the shipped
  `config --example jobs` still is — fails to load on Windows with an unset-variable error. That
  is the right *direction* to fail in, but the example should not be the thing that fails; see
  §7.9 for the two ways out.

### §2.3's guarantee is weaker on Windows, and that is a design fact worth recording

`PLAN.md` §2.3 argues that never using a shell deletes a whole class of bug because arguments
reach the child byte for byte. **That is a property of Unix passing an argv.** Windows passes one
command line which the child re-parses, so the round trip depends on the child's parser rather
than on the OS. rustic is a Rust program and uses the same MSVCRT rules `Command` quotes for, so
it holds in practice — but it holds *for this child*, not structurally, and the
`arguments_reach_the_child_byte_for_byte` unit test is therefore Unix-only. The honest place to
assert it on Windows is `tests/cli_tests.rs`, where `CARGO_BIN_EXE_rusticprofile` provides a
cooperating child.

The architectural conclusion does not change — a shell would be worse on Windows too, and there
is still no quoting logic in this crate — only the strength of the claim.

### A repeating trigger with a past boundary runs the moment it is registered

Found by registering a real task and reading `Last Run Time` back, not by reading the schema —
and it is the finding that would have shipped a §7.5 violation. The obvious way to say "every
hour" is a daily `<CalendarTrigger>` with `<Repetition><Interval>PT1H</Interval></Repetition>`.
With a `StartBoundary` in the past, **Task Scheduler treats that as currently due and runs the
task immediately.** For this tool that means `schedule` takes a backup *and runs `forget`* as a
side effect of scheduling — exactly what launchd's absent `RunAtLoad` exists to prevent.

Measured on Windows 11, `Last Run Time` read seconds after registering:

| trigger | ran on registration? | next fire |
|---|---|---|
| past boundary + `<Repetition>` | **yes, immediately** | next hour |
| past boundary, no repetition | no — never ran | tomorrow |
| future boundary + `<Repetition>` | no | **tomorrow** — loses hourly for a day |
| **24 × past boundary, one per hour, no repetition** | **no** | **the next hour** |

`StartWhenAvailable` was the first suspect and is **not** the cause: with it set to `false` the
task still ran on registration. Worth recording, because it is the setting that *sounds*
responsible and disabling it would have cost the `Persistent=true` equivalent for nothing.

So hourly is emitted as **24 plain daily triggers**, one per hour, all sharing the fixed
boundary date. It is the only construction that gets all three properties at once: no run at
registration, a correct next fire time inside the hour, and a boundary that never changes — so
generation stays pure, needs no clock, and re-scheduling is byte-identical.

Verified end to end afterwards, on this machine, against a throwaway local repository:
`schedule` → `last run: never`, `0 snapshots`, next fire at the next hour; **three** successive
`schedule` runs → still `0 snapshots`, reporting `installed` then `unchanged` twice; a manual
`schtasks /Run` → one snapshot written, `status` reporting `last success`; `unschedule` → task
gone and definition removed.

### A scheduled run opens a terminal window, and no task setting can stop it (2026-08-07)

Not found until the schedule had been armed for a day and a person watched the machine: every
hourly run popped a terminal window that appeared and vanished. Measured by enumerating visible
top-level windows during a real scheduled run —

```
class=CASCADIA_HOSTING_WINDOW_CLASS  proc=WindowsTerminal  title='C:\Users\user\.cargo\bin\rusticprofile.exe'
```

— and the cause is structural rather than a setting. Task Scheduler can run a task as the
logged-on user **only** through `<LogonType>InteractiveToken</LogonType>`, which starts it inside
that desktop session, and Windows gives a console-subsystem program a console there.

**`<Hidden>` is the setting that sounds responsible and is not.** It hides the task in the Task
Scheduler UI; it has no bearing on the process's window.

**The logon types that run outside the interactive session all need rights an ordinary account
does not hold**, which is what makes this a code problem rather than a definition problem:

| logon type | runs in | why not |
|---|---|---|
| `InteractiveToken` | the desktop session | the only one available; gets a console |
| `S4U` | session 0 | needs `SeBatchLogonRight` — **measured: `ERROR: Access is denied.`** registering from an account whose only groups are `Users` and `Authenticated Users` |
| stored password | session 0 | same right, plus it relocates a credential |
| `LocalSystem` | session 0 | needs elevation to register, and changes the state dir and credential store |

So the fix is `run --background`: `FreeConsole` at startup plus `CREATE_NO_WINDOW` on every child,
emitted into the task's argv by `schedule`. Three things about it were measured rather than
assumed, and each would have been got wrong by reasoning:

1. **`FreeConsole`, not `ShowWindow(GetConsoleWindow(), SW_HIDE)`.** Where the default terminal is
   Windows Terminal the console is a pseudoconsole, and `GetConsoleWindow` returns the
   pseudoconsole's own window rather than the visible one — hiding it hides the wrong window.
2. **Both halves are required.** A child console program started from a process with no console
   gets a *new* one, so detaching alone moves the window to rustic rather than removing it.
3. **`FreeConsole` closes this process's standard handles**, so leaving `Stdio::inherit()` in place
   makes `CreateProcess` refuse the spawn outright with `ERROR_NOT_SUPPORTED` — surfacing as
   `could not run rustic.EXE: The request is not supported. (os error 50)`, which names neither the
   console nor the handles. A detached run uses `Stdio::null()` for stdin and stderr; `Capture` is
   untouched, because §5.8's `--json` objects are needed regardless of who is watching.

**A sub-50 ms flash remains and is accepted.** The console exists from process start, before any of
this crate's code runs. Sampling every 50 ms across a run: visible in **1 sample of ~900**, against
every sample for the whole ~7 s run beforehand. Removing it entirely means the task launching
something that is not a console program — a GUI-subsystem launcher binary — which is a second
artefact through build, release, packaging and crates.io, for one frame.

### The gate is one PATH entry away from working — not a rewrite

Not a code problem, but it decides whether the house workflow survives on this machine. With the
**default** PATH nothing runs: `bash`, `sha256sum`, `date` and `install` are absent, and `find`
resolves to `C:\Windows\system32\find.exe`, a text-search tool — the same shadowing trap
`~/AGENTS.md` records for `bfs`/`find` and `eza`/`ls`. Every shebang recipe fails, including
`golden-is-current`, which `check` depends on.

**Git for Windows ships all of them in `usr\bin`.** With that directory prepended, `just check`
— golden staleness gate included — and `just man` both ran green here. So the Justfile is fine;
`set windows-shell` is worth adding only so the non-shebang recipes stop depending on which `sh`
is first on PATH.

*Also surfaced here, and platform-independent: `just check` runs `cargo clippy` **without**
`--all-targets`, so test code has never been linted. Running it with `--all-targets` found one
real lint in `exec`'s test module.*

### `RestartOnFailure` cannot retry a failed backup — measured 2026-08-10

Reported by Ken watching his own machine again, and the same shape as the console window: **every
run triggered by a resume from Modern Standby failed within ~0.2 s**, `backup saved nothing
(exit 1)`, `forget` skipped. Four of the ten runs in one day. `StartWhenAvailable` fires the
missed calendar time as soon as the machine wakes, which is *before* the network is back, so on a
laptop that sleeps most of the hour the catch-up run is the hourly run and it is systematically
spent at the one moment guaranteed to fail.

The obvious fix is Task Scheduler's own retry — `<RestartOnFailure>` with an `Interval` and a
`Count`, the GUI's *"If the task fails, restart every…"*. **It does not work for this, and the
reason is that it does not mean what the wording suggests.** Four probes on Windows 11, each a
throwaway task under `\rpretryprobe\` with `RestartOnFailure` `PT1M` × 2, counting real
invocations by appending to a file:

| probe | how the run started | what the action did | restarts |
|---|---|---|---|
| `fail` | `schtasks /Run` | `cmd /c exit 1` | **0** |
| `trigfail` | a real `TimeTrigger` | `cmd /c exit 1` | **0** |
| `ok` | `schtasks /Run` | `cmd /c exit 0` | 0 — control |
| `nolaunch` | `schtasks /Run` | command does not exist | **2**, one minute apart, then stopped |

So `RestartOnFailure` responds to *the task failing to start* — `nolaunch` reported
`0x80070002` (`ERROR_FILE_NOT_FOUND`) and its `LastRunTime` advanced from 09:53:33 to 09:55:35,
exactly the initial attempt plus `Count` restarts — and **not to the action's exit code**, whether
the run was on demand or trigger-fired. rustic exiting 1 because it cannot reach the repository is
a completely successful *launch*, so the setting is inert for every failure this project can have.

**The `trigfail` probe is the one that earns its place.** There is a real distinction between
on-demand and triggered runs in Task Scheduler, so an on-demand-only probe would have left the
negative result attributable to the wrong cause — the `hostname(1)`/`fpath`/named-time-zone
family of wrong oracles this project keeps rediscovering. Measuring both is what makes the
conclusion safe to build on.

**Third member of a family now worth naming.** `<Hidden>` sounds like it hides the window and
hides the task; `StartWhenAvailable` sounded like the cause of the run-on-registration bug and
was not; `RestartOnFailure` sounds like a retry and is a launch-failure retry. Every one of the
three cost a session's worth of confident reasoning. **On this platform, read the setting's
behaviour off a registered task rather than off its name.**

## 5.11 The resume race under systemd — measured 2026-08-11 on a Fedora 44 laptop

§7.10 shipped the retry for Task Scheduler only and said the Linux half wanted *"the measurement
first, not the code — how long after a resume a systemd catch-up run actually fires, and whether
it fails as reliably as it does here."* This is that measurement, taken on `host-b`, a Fedora 44
laptop cut over on 2026-08-03. **It answers both questions, and it retires the second one's
premise.**

### The mechanism: a `Persistent=true` catch-up fires immediately, and the spread does not delay it

Measured with three throwaway `Persistent=true` user timers whose service appended to a file —
the systemd analogue of §5.10's `\rpretryprobe\` tasks, and touching no repository. A missed
elapse was simulated by backdating the timer's stamp file in `~/.local/share/systemd/timers/`,
which needs neither a suspend nor a reboot and so is deterministic:

| arm | setup | fired |
|---|---|---|
| no stamp file | first activation | **never** — stamp written at activation, next elapse normal |
| stamp −3 h, `RandomizedDelaySec=0` | 3 elapses missed | **5 ms** after the timer unit started |
| stamp −3 h, `RandomizedDelaySec=300s` | as the real unit | **14 ms** |
| stamp −3 h, **`RandomizedDelaySec=3600s`** | 12× the real value | **5 ms** |

**`RandomizedDelaySec` does not apply to a `Persistent=true` catch-up.** The fourth row is why
that is a measurement rather than an observation: a twelvefold larger spread changed nothing, so
the immediate fire is not a small-window artefact. **So systemd is not gentler than Task
Scheduler here** — `Persistent=true` spends the missed hour at the same worst instant
`StartWhenAvailable` does, and the fleet-spread directive that looks like it would soften that
provides no protection at all.

**The first row is a finding in its own right, and it is the good news.** §5.10 records that a
Task Scheduler repeating trigger with a past boundary *runs on registration*, which made
`schedule` take a backup and run `forget` as a side effect — a §7.5 violation that needed 24
plain triggers to avoid. **systemd has no such bug: arming a timer whose stamp file does not yet
exist triggers nothing.** That was assumed on Linux and is now measured, and it was re-confirmed
in production — re-arming a live timer left its stamp file byte-for-byte unchanged and `status`
reporting the same `last run`.

### Whether it fails as reliably: YES — measured on a real resume, and it cost this section its conclusion

> **This subsection was written the wrong way round and was falsified 56 minutes later, on the
> first suspend this host took.** It is corrected in place rather than rewritten, because the
> error is the more useful half. What stood here was:
>
> > *"**The host does not suspend.** Zero `systemd-suspend.service` invocations and zero kernel
> > `PM: suspend entry` records since 2026-08-01. It is shut down and booted instead, and it
> > stays awake overnight — there is a successful run in every hour through the night. So the
> > event §7.10 measured on Windows, four times in one day, has had **no opportunities here at
> > all**, and the `Persistent=true` catch-up on boot is the only form of the race this host can
> > express."*
>
> Every number in that paragraph is still correct. **The claim built on them is not.** "No
> invocations in the ten days I looked at" is a fact about the *sample*; "the host does not
> suspend" is a claim about the *host*, and the second does not follow from the first. This is
> the project's most-repeated failure shape — **an absence of evidence written down as an
> impossibility** — and it survived less than an hour. The lesson is not "look at a longer
> window": a suspend is a thing a laptop may do at any moment, so no window could have licensed
> that sentence. **Absence can bound a rate. It cannot establish a property.**

**Measured 2026-08-11 across two suspends on the same afternoon, and it is the sample the whole
section was missing. Both catch-ups failed — 2 of 2.** The first suspend (`s2idle`) ran 08:47:05 →
09:20:04, the second resumed at 10:44:41, and each slept through an hourly slot:

| | first resume | second resume |
|---|---|---|
| `PM: suspend exit` | 09:20:04 | 10:44:41 |
| **catch-up fires** | **09:20:04 — the same second** | **10:44:41 — the same second** |
| outcome | `backup saved nothing (exit 1)`, `forget` skipped | identical |
| cause | **DNS** failure resolving the cloud token endpoint | **the same DNS failure, same endpoint** |
| network usable (`CONNECTED_GLOBAL` + lease) | 09:20:15 | 10:44:53 |
| **margin missed by** | **11 s** | **12 s** |

**So the catch-up misses the network by about eleven seconds, reproducibly**, and the Windows
finding reproduces exactly: `Persistent=true` spends the missed hour at the one instant guaranteed
to fail. Both failed on the *same endpoint* as `WIP.md` §12's single Linux data point, which is now
confirmed under controlled resumes rather than inferred from one log line — and the two margins
agreeing to within a second is what makes this a measurement rather than an anecdote.

**Neither run had the retry**, because the flag that enables it (§7.12) is emitted by `schedule` and
the units on this host had not yet been regenerated. A retry two minutes out would have found the
network up for well over a minute in both cases.

### The retry, then measured against a real resume rather than reasoned about

**A third suspend was taken deliberately after §7.12's change was deployed, and it is the only
sample in this section where the race was survived.** Same host, same job, real repository, no
stand-ins:

| | |
|---|---|
| asleep | 12:16:08 → 13:26:26; the 13:00:55 elapse passed while suspended |
| catch-up fired | **13:26:26 — again the same second as `PM: suspend exit`** |
| attempt 1 | **failed**, the same DNS lookup as the other two |
| network usable | 13:26:37 — **an 11 s margin, matching the first sample exactly** |
| attempt 2, at +2 min | **succeeded — `backup saved 3 of 3 snapshot sets — after 1 further attempt 2 minutes apart`** |
| **`forget`** | **ran and succeeded** |
| unit | `Result=success`, wall clock **2 min 4.4 s**, CPU 2.1 s |
| status record | `last_success` advanced, `skipped: []` |

**Three margins now: 11 s, 12 s, 11 s.** That is what makes the two-minute interval a defensible
choice rather than a guess — it is an order of magnitude clear of the observed window, without being
so long that a retry collides with the next hourly trigger.

**The result that matters is the `forget`.** In both pre-change samples retention was skipped, which
§7.10 identifies as the failure class this project exists for; here it ran. So the claim is no longer
"a retry would have helped" — **the retry converted a resume-race failure into a complete run,
observed once, end to end, on a production host.**

*What this does not establish:* that the margin is always ~11 s. A network that is genuinely down
for longer still produces a failed run and the next scheduled run remains the backstop — §7.10 says
so and that is unchanged. It also says nothing about launchd.

**This is a clean sample, and it is clean only because the confound below was fixed first.** The
argv carried the absolute `--rustic-binary`, so the failure cannot be the PATH defect — it is the
network and nothing else. `last success` stayed at the previous hour, `last run` recorded the
failure, and the next scheduled run covered the gap, so **no backup was lost and retention was
skipped for one hour** — precisely §7.10's accounting.

**Resume and boot are two different latencies, and only one is understood.** The resume catch-up
fired at ~0 s, agreeing with the probe's ~5 ms. The two *boot* catch-ups fired at 185 s and 204 s.
Same directive, same host, two behaviours — so the residual recorded at the end of this section is
specific to the boot path, and the resume path is now measured and immediate.

### The eight-day window, and why it was measuring the wrong thing

Over eight days on the armed timer: **153 runs, 6 failures (3.9%), and none of them was a network
failure.**

| failures | cause |
|---|---|
| 5 | `could not run rustic: No such file or directory (os error 2)` |
| 1 | exit 2, a configuration error, on the cutover day itself |

**The five are the finding, and they were also a mask.** Every one is the first scheduled run
after a boot, failing for the reason in the next subsection — and because they all failed *loudly
and identically*, for a reason that had nothing to do with the network, **the resume race was
invisible behind them.** The instant the louder defect was fixed, the very next suspend produced
the failure above. That is the transferable part: **a defect that fails every candidate run can
hide a second defect on the same runs**, and the count of failures says nothing about how many
causes are in it.

Over eight days on the armed timer: **153 runs, 6 failures (3.9%), and not one of them was a
network failure.**

| failures | cause |
|---|---|
| 5 | `could not run rustic: No such file or directory (os error 2)` |
| 1 | exit 2, a configuration error, on the cutover day itself |

**So the Windows finding does not generalise, and the reason is not that Linux is better at the
race — it is that this laptop never enters it.** Any claim about the systemd resume race still
needs a Linux host that actually suspends; `host-b` cannot supply one.

### What the failures actually were: `0.1.10`'s bug, still live, on a stale unit

Every one of the five is the first scheduled run after a boot, and the cause is that the
*installed* unit predates `0.1.10`:

```
ExecStart=…/rusticprofile run --name dot-files --config …/jobs.yaml          # installed
ExecStart=…/rusticprofile run --name dot-files --config …/jobs.yaml \
          --rustic-binary /home/user/.cargo/bin/rustic                       # what 0.2.13 emits
```

`--rustic-binary` has been emitted unconditionally since **`v0.1.10`**, so the binary that wrote
this unit was older than that. The units were generated at the 2026-08-03 cutover and **never
regenerated across upgrades to `0.1.31`, `0.2.5`, `0.2.9` and `0.2.13`** — arming a timer is a
one-time act, and nothing in the tool, the gate or the rollout procedure re-emits a unit when the
binary changes. With `linger` enabled the user manager starts at boot with
`PATH=/usr/local/bin:/usr/bin`, the bare `rustic` is unresolvable, and the run fails in
milliseconds; once a graphical login imports the session environment `~/.cargo/bin` appears and
every later run succeeds. **`0.1.10`'s own sentence applies unchanged: the working runs are the
accident.**

**Proved rather than inferred, with a negative control, by reproducing the boot environment
instead of waiting for a boot** (`env -i`, `PATH=/usr/local/bin:/usr/bin`, `--dry-run` so nothing
is written — ladder rung 4):

| argv | result under the boot PATH |
|---|---|
| the installed unit's (bare `rustic`) | `could not run rustic: No such file or directory` — the historical failure, reproduced |
| the regenerated unit's (absolute path) | `password is correct`, index read, dry-run backup proceeds |

**The control that makes this a per-host defect rather than a code defect is another host.**
`host-d`, the designated prune host, was cut over the *same day, 69 minutes later*, with a
byte-identical `jobs.yaml` — and its units **do** carry `--rustic-binary`. Same fleet, same
config, one host correct and one not, so the variable is the generating binary and nothing else.
A third Linux host was powered off and could not be checked.

Regenerating fixed `host-b`: the unit diff is exactly the one added flag, the timer file is
unchanged, `plan --format lines` is byte-identical before and after (§4a's contract check), and
no run was triggered.

### A directive in our own generated unit that does nothing

The generated service carries, with a comment explaining itself:

```
# A backup that starts before the network is up fails slowly and confusingly.
After=network-online.target
Wants=network-online.target
```

**`systemctl --user show network-online.target` reports `LoadState=not-found`.** There is no
user-level `network-online.target`, so on a *user* timer — which is what `permission: user`
installs, and what the whole fleet runs — both directives are inert and the comment describes a
protection that has never existed. `Wants=` on a missing unit is ignored rather than fatal, so
this fails silently in the one direction this project cares about.

**That is a fourth member of the family §5.10 names**, after `<Hidden>`, `StartWhenAvailable` and
`RestartOnFailure` — and the first one inside an artefact rusticprofile generates itself rather
than in a platform's API. It is documented here and deliberately **not** fixed in the same
change, on `0.2.11`'s precedent: the replacement is a design decision, not a substitution. The
mechanism a user unit would need does exist on this host — `podman-user-wait-network-online.service`,
*"Wait for system level network-online.target as user"*, which ran on the boot analysed above —
but it ships with podman, so it cannot be depended on, and `permission: system` would not need it.

### One result left unexplained, stated rather than guessed at

The probe says a catch-up fires within milliseconds. **The two real boot catch-ups fired 185 s
and 204 s after the timer unit started.** Two samples of the same shape are a pattern, not noise,
and the probe does not reproduce it.

A pre-NTP clock step was the leading hypothesis and is **disproved**: on both boots chrony
selected a source about five seconds after starting and logged no step, and no clock adjustment
appears anywhere in either window. The user-manager journal shows nothing queued against our
service between the timer starting and the run. **Cause unknown.** It matters, because zero
seconds and three minutes are the difference between a retry being necessary and being marginal,
so the honest position is that the *mechanism* is measured and the *live latency* is not yet.

### Consequence for §7.10 — the race is confirmed on systemd, and the mechanism is now the open question

> **This subsection reversed once, in the hour between the two halves of the measurement.** What
> stood here concluded that §7.10's *"not extended to systemd or launchd"* **stands**. It was
> written from the eight-day window alone, i.e. before this host had ever been observed
> suspending, and the resume above overturns it. Kept per this file's convention:
>
> > *"The retry's premise **holds mechanically** … But it is not what this host needed, and
> > shipping it here would have been the mirror of the error §7.10 was written to avoid: a retry
> > would have made every one of the five failures worse, and fixed none of them … So §7.10's
> > 'not extended to systemd or launchd' **stands**, and now stands on a measurement rather than
> > on the absence of one."*
>
> **The bullet about the five failures was true and is still true** — a retry does nothing for a
> `rustic` that is missing from `PATH`, and would have turned each into three failures over four
> minutes. **The conclusion drawn from it was wrong**, because those five were not the population
> the retry is for. Reasoning correctly about the only failures visible, while a second cause sat
> masked behind them, produced a confident answer to the wrong question — which is the same
> mistake in a new costume as the sentence corrected further up.

**The race is real on systemd, and the retry does prevent the failure — observed, not inferred.**
The network became usable 11 and 12 seconds after resume in the two pre-change samples, and §7.10's
policy is two further attempts at two-minute intervals. **After this change was deployed a third
suspend was taken: the catch-up fired in the resume second, failed on the same DNS lookup, and the
+2 min retry saved it — 3 of 3 snapshot sets with `forget` succeeding, where both earlier samples
had skipped retention.** §5.11 records the run. So this section rests on a measurement of the fix,
not only of the fault.

So the honest position is the opposite of what stood here: **extending the retry to systemd is
now supported by evidence rather than merely not ruled out.** Two things still stop this section
from simply prescribing it.

**First, there are now two candidate fixes for one failure, and the measurement prices both.**

| | what it costs | what it leaves |
|---|---|---|
| **retry** (§7.10's mechanism, ported) | ~2 min of latency; the first attempt still *is* a failure, so the log, the status record and any monitor see a `failure` verdict that later resolves | works whatever the cause — no assumption about *why* the run failed |
| **make the ordering real** (a working wait-for-network) | ~11-12 s of latency on these samples; the run succeeds first time and nothing ever reports a failure | only helps for a *network* cause, and needs a dependable user-level mechanism that does not currently exist |

The second is strictly better where it applies, and it is also the fix for the inert directive
this section already documents — the unit *claims* that ordering today. But `RunOnlyIfNetworkAvailable`
was rejected in `schtasks.rs` for a reason that transfers exactly: **the repository may be local**,
and gating a backup on the OS's idea of connectivity adds a silent skip. A repository on an
external disk needs no network and must not wait for one.

**Second, §7.10's own constraint 1 does not have an obvious systemd analogue.** The Windows retry
is confined to detached runs, because `--background` is emitted only by `schedule` and only there,
so a hand-typed `run` still fails immediately for the person watching. On systemd nothing
distinguishes a scheduled run from a typed one at the argv level today — the unit runs the same
`run --name …` a human would. Porting the retry therefore needs a gate to be *chosen*, not merely
reused, and that is a design decision rather than a translation.

**So: the measurement §7.10 asked for is delivered and its answer is "yes, port it" — but which
mechanism, and how the detached-only gate is expressed on systemd, are open and belong in a
decision section before any code**, on the same §5.9/§7.9/§7.10 precedent that put each of those
decisions in this file first. What is no longer open is whether Linux has the race: it does, it is
immediate, and it has now cost a real run.

## Safety rules observed during this testing

Read-only against GCS (`repoinfo`, `snapshots`); every write test in a throwaway local repo under
`/tmp`, deleted afterwards; no `prune` against GCS; no snapshots deleted on any host.

*Unchanged for the 2026-08-10 retry work: the four probe tasks ran `cmd.exe` and touched no
repository, and the end-to-end verification of the retry used a shim standing in for rustic —
ladder rung 2 — so nothing reached GCS. All four tasks and their folder were deleted afterwards
and the deletion verified, which the earlier `\rpprobe\` probes' cleanup got wrong: `schtasks
/Delete /TN` was given folder paths, which names no task, so six probe tasks survived for days
and flashed a console window every hour. **Deleting a task folder takes `Unregister-ScheduledTask`
per task — with a trailing backslash on `-TaskPath` — and then the Schedule.Service COM
`DeleteFolder`.***

*Unchanged for the Windows work (2026-08-06): the shared repository was never contacted. Every
measurement above used a throwaway local repository under the temp directory, and the two
`GetComputerNameExW` results were read from the OS.*

*Unchanged for the systemd measurement (§5.11, 2026-08-11): the three probe timers ran `/bin/sh`
appending to a file in a temp directory and named no repository, and the only thing that reached
GCS was a `--dry-run` — ladder rung 4, read-only, nothing written and no snapshot deleted. All six
probe units and their stamp files were removed afterwards and **the removal was verified four
ways** — `list-timers`, `list-unit-files`, the unit directory and the stamp directory — because
§5.10 records a probe cleanup that named the wrong kind of object, deleted nothing, and reported
success. Two further notes on oracles from the same session: the probes' own log file was a
**broken oracle** (a `$(date)` inside the unit was never expanded, so every line recorded a shell
path instead of a timestamp) and the journal was used instead; and the probe units were written
into a chezmoi-managed directory, which was checked first for an `exact_` prefix — it has none, so
an `apply` could neither delete them nor be surprised by them.*

---

# Part 7 — SETTLED: how absent sources are handled

**Decision (2026-07-30, Ken): option B — named snapshot sets, selected per host.** This was the one
open question blocking all code; it is now closed and M1 may proceed. §7.1 keeps the options and
why B won; §7.2 records what testing the mechanism actually revealed, which added two design rules
in Part 4 that were not in the original write-up.

Raised by §5.7, and it decided whether rusticprofile owns any part of the backup source list.
Under B it **does not**: `rustic.toml` keeps every source path, and rusticprofile only chooses
which named sets to invoke on this host — plus a read-only check that those names exist (§7.2).

## 7.1 The options

The constraint: rustic hard-fails a backup if any source path is absent, with no opt-out; the
fleet has paths that exist on some hosts and not others; and if sources live only in
`rustic.toml`, rusticprofile has nothing to filter.

Three viable options.

**A. Per-host `rustic.toml`, templated by chezmoi.** Each host gets a config listing only the
paths it has. rusticprofile stays a pure scheduler and owns no backup config at all.
*For:* zero new machinery — chezmoi already manages dotfiles per host and already does hostname
templating, and unlike resticprofile's config there is no `{{ }}` collision to work around here.
*Against:* the source list is no longer visible in one place; adding a path means editing a
template and running `chezmoi apply` on 7 machines.

**B. Named snapshot sets, selected per host.** rustic supports multiple `[[backup.snapshots]]`
entries and `rustic backup --name <NAME>` to run specific ones. `rustic.toml` defines one entry
per source group; rusticprofile's job config lists which names to run, gated by
`enabled-on-hosts`.
*For:* one shared `rustic.toml`; per-host variation stays in rusticprofile where the rest of the
host gating already lives; uses a first-class rustic feature.
*Against:* granularity is per named group, not per path — a group containing one absent path
still loses that whole group.

**Tested 2026-07-30, and per-entry isolation holds.** Two `[[backup.snapshots]]` entries, the
second referencing an absent path:

```
[INFO] snapshot b5f0ba45 successfully saved.        <- the good entry
[ERROR] error backing up …/good2,…/ABSENT: error sanitizing source
error: Not all snapshots were generated successfully!
exit code: 1        snapshots saved: 1 of 2
```

So one broken entry does **not** abort the others. Option B is viable.

**But note the exit code: partial success still exits 1.** rustic gives no distinct code for
"some snapshots saved, some failed", so `rustic/exit.rs` must treat exit 1 as *ambiguous* and
determine what actually happened — by counting saved snapshots, or matching the
`Not all snapshots were generated successfully` marker. Getting this wrong reintroduces exactly
the failure that started this whole project: a backup that partially worked being treated as a
total failure, aborting the run before retention. **This is the single most important behaviour
to unit-test in M1.**

**C. rusticprofile owns the source list and passes paths on the CLI.** `rustic backup [SOURCE]...`
overrides config sources entirely, so rusticprofile could hold `sources[].if-missing: skip`,
stat each path, and pass only those that exist.
*For:* directly implements the `required: false` design; the absent-source failure becomes
impossible by construction; one shared config; loud when a `required: true` path is missing.
*Against:* rusticprofile now owns part of the backup config, which is exactly the delegation
boundary the design was built to avoid. It would own sources while rustic owns excludes, tags and
policy — a split that needs justifying.

**Chosen: B.** It keeps the delegation boundary intact and puts per-host variation where all the
other host gating already is. A is the least work but scatters the fleet's source list across seven
templated files. C was held as the fallback if per-entry granularity had failed; it did not, so C
stays rejected — it would have made rusticprofile own sources while rustic owns excludes, tags and
policy, a split with no justification now that B works.

## 7.2 Testing the mechanism — results (2026-07-30, rustic 0.11.3)

Throwaway local repository under a temp dir, deleted afterwards; nothing touched GCS. Three
`[[backup.snapshots]]` entries named `core`, `extra`, `absent`, the last pointing at a path that
does not exist.

| Invocation | Exit | Result |
|---|---|---|
| `backup --name core` | 0 | only that entry ran |
| `backup --name core --name extra` | 0 | both ran — the flag is repeatable, as documented |
| `backup --name nosuch` (alone) | 1 | `error: no backup source given.` |
| **`backup --name core --name nosuch`** | **0** | **`core` ran; the unknown name was silently ignored** |
| `backup` (no `--name`, all three entries) | 1 | 2 of 3 saved |

The `name` key on a `[[backup.snapshots]]` entry is accepted by rustic 0.11.3 and is what `--name`
matches. So option B's mechanism is real and behaves as assumed.

**The fourth row is the finding that changed the design.** An unknown `--name` is only loud when it
is the *only* name given; alongside any valid name it is dropped with **exit 0 and no diagnostic**.
A set renamed or typo'd in `rustic.toml` would therefore back up less than intended, forever, with
a green run — the same silent-degradation class as the four bugs in §2.1, arriving through a
different door. rustic will not catch this, so rusticprofile validates every `--name` it emits
against `rustic.toml` at load time. That, plus "a job whose sets all resolve away is an error, not
an empty run," are the two rules added to Part 4.

**Partial success still exits 1** — no distinct code for "some saved, some failed", confirmed again
here (2 of 3, exit 1). §5.8 covers how to disambiguate it; treating it as total failure would
abort the run before retention and reintroduce exactly the bug that started this project. **This
remains the single most important behaviour to unit-test in M1.**

*Not blocking code:* what a partial backup should *do* is one match arm, not an architecture
question. The design intent is to classify it as a **warning** — loud in the report, `forget` still
runs — consistent with Part 4's "a warning must not abort the job." Settle the exact exit code when
`rustic/exit.rs` is written.

## 7.3 Option B needs label-based retention grouping — found 2026-08-01

> **Promoted to `NOTES.md` §3a (invariant 1 — group named sets by label).** That is where this rule is maintained now.
> The text below is the original finding and its measurements, kept as the evidence.

Named snapshot sets have a consequence Part 7 did not anticipate, and it is not optional.

**With `group-by = "host"`, the sets compete.** Retention keeps the newest snapshot per
period *per group*, so three sets written in the same hour land in one group and only the
last one written survives. Measured against the live repository, on a `forget --dry-run`:

| snapshot | written | verdict |
|---|---|---|
| `nushell` — **0 bytes** | 13:16:24 | **keep** |
| `gnupg` | 13:16:23 | remove |
| `core` — **6,256 files** | 13:16:22 | remove |

The empty set won because it finished last. The run reported success. This is the
project's own failure class — a backup quietly doing less than it says — reached through
a configuration choice rather than a bug.

**The fix is a stable `label` per set plus `group-by = "host,label"`**, verified in a
throwaway repository and then against the live one: each set is then retained
independently.

**Label, not paths.** rustic's default is `host,label,paths`, and grouping on paths is
what let 2810 snapshots survive a policy capping ~49 per host — every change to a source
list minted a new group with its own full quota. A set's label is stable by construction;
its path list is not.

So the rule is: **a job using named snapshot sets must group retention by label.** A
profile that names sets and groups by host alone is misconfigured, and the symptom is
silent.

## 7.4 Ladder rungs 7 and 8 — results (2026-08-01)

Both taken against the live repository from this host.

- **Rung 7, real backup.** Three sets saved. Counts moved 27 → 30 for this host and
  542 → 545 overall: exactly +3 on both, so the write was additive and no other host was
  affected. Exclusions verified against the *stored* snapshot rather than the dry run —
  password files, cloud credentials, caches and `node_modules` all absent, with positive
  controls (`.ssh` 48 files, `chezmoi` 425) so the check could actually fail.
- **Rung 8, real `forget`, prune disabled.** Removed exactly the three snapshots the dry
  run predicted. Counts 33 → 30 and 548 → 545; **pack count unchanged at 630**, confirming
  no data was rewritten and no exclusive lock taken. Every other host's count was
  untouched, so `[snapshot-filter]` held under a real irreversible operation.

### A near-miss worth keeping

While fixing §7.3, a scripted edit matched the string `[forget]` inside a *comment* and
deleted everything up to the real section — including `[snapshot-filter]`. Had the result
still parsed, a `forget` would have run unscoped across all seven hosts.

It did not parse, which was luck. But the invariant from M1 step 6 was not luck:
reconstructing the damaged profile so that it *does* parse and running `config --check`
against it produces a refusal naming the missing filter. The guard built for exactly this
case caught exactly this case.

Two lessons, both cheap: do not perform structural edits on safety-critical config by
string matching, and run `rusticprofile config --check` after *any* edit to a rustic
profile, not just after editing `jobs.yaml`.

## 7.5 The same hazard from outside the tool — found 2026-08-01

> **Promoted to `NOTES.md` §3a (invariant 2 — one retention authority).** That is where this rule is maintained now.
> The text below is the original finding and its measurements, kept as the evidence.

§7.3 says a job using named sets must group retention by label. That rule protects the sets
from *each other*. It does nothing about a **second tool applying its own retention to the
same host**, and on a machine mid-migration there is one by definition.

The predecessor's profile ends with:

```yaml
retention:
  after-backup: true      # runs after every hourly backup
  host: true
  path: false
  tag: false
  group-by: "host"        # host alone
  keep-hourly: 24
```

`host: true` scopes it to the machine, which reads as safe. But `group-by: host` with
`path` and `tag` both off puts **every snapshot on that host in one group**, including
snapshots it did not create and does not know about. `keep-hourly: 24` then keeps one per
hour: the newest. From its log, a single pass removed three of ours —

| snapshot | written | size | verdict |
|---|---|---|---|
| `nushell` | 13:16:24 | 0 B | removed |
| `core` | 13:20:44 | **395.591 MiB** | **removed** |
| `gnupg` | 13:20:45 | 2.331 MiB | removed |

— and kept a 0-byte `nushell` snapshot written at 13:20:45, one second younger than the
395 MiB one it deleted. Same failure mode as §7.3, same 0-byte winner, but the deciding
configuration was in a *different tool's* file.

**It runs both ways.** The predecessor's own hourly backup for that hour completed and
logged `snapshot c35cb636 saved`, 397.9 MiB. It is not in the repository. No pass of its own
removed it, and the only other `forget` that ran that day was ours — so our correctly
grouped retention deleted one of *its* snapshots. Correct grouping protects the groups it
knows about; an unlabelled foreign snapshot is just another member of the empty-label group.

Neither tool was scheduled against the other at the time. This happened during manual
ladder rungs, in a single afternoon, from two configurations that are each defensible alone.

So the rule §7.3 states is necessary but not sufficient. The full form:

> **Exactly one retention authority per (repository, host).** Two tools may *back up* the
> same host concurrently — backups are additive and, with prune disabled, nothing is
> destroyed. Two tools may not *forget* it. Migration means moving that authority, not
> overlapping it.

`status` shows what this host schedules; it cannot show what some other tool schedules. A
cutover therefore has an ordering that is not optional: **disable the outgoing tool's
retention before enabling the incoming tool's schedule**, and confirm from the repository
rather than from either tool's own report.

There is a design consequence too. rusticprofile emits no retention flags — `forget` is
bare, and every policy comes from the rustic profile — so it cannot detect this. A future
`doctor` command could: read the profile's `group-by`, and warn when a host's snapshots
carry a mix of labelled and unlabelled entries, which is what a second writer looks like
from inside the repository.

## 7.6 Mixing restic and rustic against one repository (found and corrected 2026-08-02)

> **Promoted to `NOTES.md` §3a (invariant 3 — one lock protocol).** That is where this rule is maintained now.
> The text below is the original finding and its measurements, kept as the evidence.

§7.5 established that a (repository, host) may have exactly one **retention** authority.
That is not the only authority a repository has, and the second one was already broken
when §7.5 was written.

**What a restic lock is, stated precisely, because the imprecise version is misleading.**
It is not a local file. It is an object *inside the repository*, under `locks/`. On object
storage it is a key in the bucket, so every machine sharing the repository sees it. That is
what makes restic's mutual exclusion work across a fleet at all, and it is why one client
can be observed waiting for another to finish.

`restic backup` takes a **shared** lock — concurrent backups from different hosts are legal
and expected. Only exclusive operations refuse: `prune`, `rebuild-index`, `forget --prune`.

**rustic 0.11.3 does not participate.** It writes no lock object and checks for none. Note
the only `lock` matches in `rustic backup --help` are the substring inside `--set-blockdev`,
which is a false positive of exactly the kind that makes "grep for the flag" an unsafe way
to answer this.

### Measured, in a throwaway local repository

A restic backup was frozen with `SIGSTOP` while holding its lock, so the lock stayed live
and could be tested against deterministically.

| step | result |
|---|---|
| **control** — `restic prune` with a restic lock held | **refused**: `repository is already locked by PID … on host-f`, with the lock's age |
| a `rustic backup` frozen mid-write — locks present in the repository | **0** |
| `restic prune` while that rustic backup is mid-write | **proceeded**: `deleting unreferenced packs … 14 / 14 files deleted`, 487.780 MiB |
| let rustic finish, then `restic check --read-data` | **`The repository is damaged and must be repaired. Fatal: repository contains errors`** — 5 data packs missing |

So this is not a race that might in principle be lost. It is a reproducible path from
"prune runs while rustic backs up" to a repository that fails its own integrity check:
prune classifies rustic's in-flight packs as unreferenced, deletes them, and the snapshot
rustic subsequently writes points at data that is gone.

**A wrong control is worth recording.** The first attempt used a second `restic backup` and
expected it to be refused. It succeeded — because backups take a *shared* lock — and for a
moment that looked like evidence that restic does not lock either. Exclusion must be tested
with an operation that actually excludes.

### Why this became true on 2026-08-01

Nothing about the prune host changed. Before the cutover, `host-f` ran restic, so the prune
on `host-d` saw its lock and waited. The cutover replaced the writer with one that does not
speak the protocol, and that removed the only thing making the two tools mutually visible.

This is the same shape as §7.5 — two tools, one repository, each configuration defensible
alone — but the object at risk is pack files rather than snapshots, so the consequence is
worse. §7.5's snapshots were unreachable but their data survived, because `prune: false`
left the packs behind. Here the packs are what gets deleted.

> **Exactly one lock *protocol* per repository.** This is a separate claim from §7.5's
> retention authority, over the same repository. Two tools that each guarantee safety by a
> different mechanism do not compose: each is correct alone, and together neither holds.

### Correction, same day — rustic is not deficient, the *mixture* is

The paragraphs above were written before reading rustic's own documentation, and they
overstate the finding in a way that led to a wrong operational decision. Recorded rather
than rewritten, because the mistake is the useful part: **"tool X lacks the mechanism I
expected" is not the same as "tool X is unsafe", and the gap between them was a day of
unnecessary exposure and a prune schedule disabled for no reason.**

rustic's FAQ is explicit:

> "Yes, all operations are designed lock-free. This means all commands can run parallel."
>
> "rustic uses the same repository format as restic, so you can use rustic and restic on the
> same repository. The only thing you have to take care of is that you don't run prune with
> restic and rustic at the same time."

The mechanism is **two-phase deletion, not locking**. `prune` marks packs and removes them
only after `--keep-delete`, which defaults to **23 hours**, so a concurrent backup that
references a marked pack has a day of grace. `--instant-delete` is the documented opt-out and
carries the warning *"Only use if you are sure the repository is not accessed by parallel
processes!"*

Verified rather than taken on faith — a throwaway repository with three unreferenced packs:

| command | packs before | packs after |
|---|---|---|
| `rustic prune` (default) | 3 | **3** — reported `to delete: 3 packs`, removed none |
| `rustic prune --instant-delete` | 3 | **0** |

So the true matrix is:

| combination | safe | why |
|---|---|---|
| `rustic prune` + rustic backup | **yes** | the 23-hour grace period |
| `restic prune` + restic backup | **yes** | restic's repository lock |
| **`restic prune` + rustic backup** | **no** | restic deletes immediately, which is safe *only* because of a lock rustic never takes |
| `rustic prune` + restic backup | probably | the grace period still applies |

The measured corruption was the third row, and only the third row.

### Consequences for this project — revised

**M4 shrinks, and stops being a precondition for prune.** The earlier conclusion — that
nothing may prune until repository-lock coordination is built — was wrong. **Finishing the
migration is the fix.** Once every host writes with rustic, prune is safe by rustic's own
design, and the designated prune host's schedule can return as a **`rustic prune`** rather
than a `restic prune`. What M4 would add is defence in depth, not permission.

**What must not happen** is a `restic prune` against this repository while any host backs up
with rustic — which is exactly the state the fleet was in between the first cutover and
2026-08-02. That is a migration-ordering hazard, and it belongs beside §7.5's: *move the
prune authority to the same tool as the writers, or move it after them.*

**`LockBudget` is no longer load-bearing.** It stays a seam. If it is ever built, restic-format
`locks/` objects remain the only design that a non-rusticprofile writer could see — but the
cheaper answer is that a fleet running one tool needs no such thing.

**The `doctor` check from §7.5 keeps its second item, restated.** Not "rustic is writing and
something else holds a lock", but the narrower and checkable: *a repository written by rustic
while a restic `prune` schedule still exists anywhere on the fleet.*

## 7.7 Shipping the findings as a config — `config --example` (decided 2026-08-02)

> **Promoted to `NOTES.md` §3a (invariant 5 — the config carries the traps).** That is where this rule is maintained now.
> The text below is the original finding and its measurements, kept as the evidence.

Everything Part 7 discovered lives in prose here and in one hand-written file on one
machine that is not in chezmoi. That is the wrong place for it. The delegation boundary
means rusticprofile owns almost nothing, so **every decision that can silently destroy data
is in the rustic config** — and a user who reads the README's 17-line sketch and writes
their own `rustic.toml` will get several of them wrong, because we did.

The guidance an example has to carry, all of it already measured, none of it guessable:

| the trap | where |
|---|---|
| `opendal:gcs`, not restic's `gs:` — the scheme does not exist in rustic | §5.1 |
| scoping filters go in `[snapshot-filter]`; under `[forget]` rustic **accepts and ignores** them, and rejects no unknown keys, so a config can look scoped and filter nothing | §5.5 |
| `group-by = "host,label"` — with `"host"` the named sets compete for one slot and a 0-byte snapshot evicts a 395 MiB one | §7.3 |
| exclusion globs need a leading `!`; a bare pattern is an *include* filter | §7.2 |
| split sets by how reliably the path exists — rustic hard-fails an entire set if any one source is missing, with no opt-out | §5.7 |
| an unknown `--name` is silently ignored whenever a valid one is also given | §7.2 |
| the password file and credentials must be excluded from the backup, or the key goes inside the lock | §4.1 |
| `path`/`tag` filters embed the source list, so a renamed directory orphans the history | §7.3 |

### Shape: `config --example <jobs|rustic>`, to stdout

**Stdout, not a file.** This is the same decision as `--completions`, for the same reason:
the tool emits, and *placing* it is the user's explicit act. It also makes the command
hermetic and unable to clobber a working config, which matters more than usual when the
thing it would overwrite is what stands between a fleet and its backups. `just
install-completions` is the precedent for a convenience wrapper if one is ever wanted.

**A flag on `config`, not an `init` verb.** `config --check` and `config --show` already
mean "inspect the configuration"; emitting a starting point belongs with them. An `init`
that writes two files into XDG paths is a different and more dangerous command, and nothing
yet needs it.

**Static placeholders, using this project's own redaction vocabulary** — `host-a`…`host-h`
and `/home/user`. Emitting the *real* hostname and `$HOME` would produce a file that runs
as-is, which is precisely the objection: a config that appears to work is one nobody reads,
and every value in it needs reading once. It also keeps the feature clearly outside the
"no templating in any form" non-goal — there is no user-supplied template, no expression
language, and nothing is substituted at all.

**The examples must pass `config --check`.** A test writes both, substitutes only the
directory, and validates them through the real binary. An example that has drifted out of
step with the validator is worse than none, because it is quoted with authority.

## 7.8 A read-only passthrough — `snapshots` (decided 2026-08-03)

> **Promoted to `NOTES.md` §3a (invariant 4 — the delegation boundary).** That is where this rule is maintained now.
> The text below is the original finding and its measurements, kept as the evidence.

The predecessor answers `resticprofile @dot-files` with a snapshot listing, because its
config sets `default-command: "snapshots"`. Migrating removes a daily habit and replaces it
with `rustic -P ~/.config/rustic/dot-files.toml snapshots` — a path the user must remember
and rusticprofile already knows, resolves and validates.

**The friction is real; the obvious fix is not.** Wrapping rustic's commands is what Pivot 2
rejected, and the README states the whole contract as
`rustic -P <profile> <operation> [--dry-run] [--json] [--name <set>]…`. Growing a `snapshots`
command invites `check`, then `ls`, then `restore` — and restore is refused on purpose.

### What is actually being added, and what is not

The value the user is missing is **profile resolution**, not a new capability. So the command
adds exactly that and nothing else:

- It emits `rustic -P <resolved-profile> snapshots` plus **whatever the user appended**. Every
  flag beyond `-P` comes from the caller. rusticprofile still constructs none.
- It is **read-only**. `snapshots` cannot alter a repository, which is what makes a
  passthrough defensible here and would not make one defensible for `forget` or `prune`.
- It is **not an `Operation`.** `Operation` stays a closed set of three — `backup`, `forget`,
  `prune` — because that enum is what a *job* may schedule. A query is not schedulable work,
  and letting it into `jobs.yaml` would be a real boundary move rather than an ergonomic one.

### The line, stated so the next request can be answered

**A passthrough is acceptable only where it is read-only and adds no flags.** By that rule
`check` would also qualify and could be added if wanted. `restore` never does — not because
it writes, but because `PLAN.md`'s non-goals already settled it: putting a restore path
behind a scheduler adds a layer between the operator and their data at the moment they least
want one. `forget` and `prune` are excluded because they are destructive, and their scoping
lives in the rustic profile where a `--filter-host` typed at a prompt could contradict it.

The flag-inventory test in `rustic/invoke.rs` continues to assert that **job** invocations
carry only `-P`, `--json` and `--name`. That guarantee is unchanged; this command is not a
job invocation and has its own test asserting rusticprofile contributes only `-P` and the
operation word.

## 7.9 SETTLED: Windows becomes a supported platform (2026-08-06)

**Decision: Windows is supported, and Task Scheduler becomes a third scheduling backend.** This
reverses a stated v1 non-goal, so it is written here before the code, per the precedent §5.9 set
when the hostname decision moved the delegation boundary.

### Why the non-goal was right, and why it stopped being right

The original reasoning was *"the scheduling backends are systemd and launchd"* — sound, because it
described a fleet of five Linux hosts and two Macs. It was never an argument about Windows being
unsuitable; it was an argument about where the machines were.

**The development machine was reinstalled from Fedora to Windows.** So the platform with no
support is now the platform the work is done on, the gate is run on, and releases are cut from.
That is a different question from "should we grow a third platform for users", and it answers
itself.

### What is in scope, and what stays out

| | |
|---|---|
| **In** | building and running on Windows; `config`, `plan`, `run`, `snapshots`, `status`; a Task Scheduler backend for `schedule`/`unschedule`/`status` |
| **Out** | `crond` — still a non-goal, and nothing on this fleet uses it |
| **Out** | restic as a backend — unchanged, for the §3 reasons |
| **Out** | WSL as the answer. A backup tool that only protects a Linux filesystem inside the machine is not protecting the machine |

### The three things that are genuinely different, and how each is answered

**1. There is no `flock`.** The lock is a file opened with `share_mode(0)` — no sharing at all, so
the *open itself* is the exclusion and a second holder gets `ERROR_SHARING_VIOLATION`. That is
mandatory rather than advisory, needs no second syscall and no dependency, and is released by the
kernel when the handle closes for any reason. `LockFileEx` was rejected: it needs `windows-sys` and
locks byte ranges inside a file that is still openable, which is a weaker thing to hold.

**2. There are no signals.** And forwarding turns out to be unnecessary for the interactive case:
a child spawned without a new process group shares the console, and the console delivers `Ctrl+C`
to every process attached to it, so rustic receives the interrupt from the same keypress. What is
absent is the *record* — no handler, so `Outcome::interrupted` stays false and `Verdict::Interrupted`
is never reported there. Stated rather than papered over, in the same spirit as launchd reporting
no next-fire time.

The case that is **not** covered is a scheduled run, which has no console: stopping the task kills
only the top process and orphans rustic. Closing that needs a job object with
`KILL_ON_JOB_CLOSE`, and it belongs with the backend rather than with `exec` — a scheduled run is
the only place it can happen.

**3. `%COMPUTERNAME%` is a trap, not a source.** See §5.10. `GetComputerNameExW` is the reason
`windows-sys` is a dependency at all, and it is worth one: the alternative silently strands a
host's history in its own retention group.

### One deliberate consistency, and it is the same argument as `0.1.25`

**The XDG rules apply on Windows too** — `~/.config`, `~/.local/state`, not `%APPDATA%`. Not a
preference: `jobs.yaml` is byte-identical fleet-wide, so `${state_dir}` in one shared `log:` line
must not resolve to a third distinct place. There is a local precedent — chezmoi, which generates
this fleet's configuration, reads its own config from `~/.config` on Windows.

### The one thing left undecided, because it is a fleet decision and not a code one

The shipped `config --example jobs` sets `rustic-config-dir: "${env:HOME}/.config/rustic"`, and
`HOME` is normally unset on Windows. Two ways out, and they are not equivalent:

- **Rely on the default.** That key's default is already `$XDG_CONFIG_HOME/rustic`, which is now
  correct on all three platforms, so the line can simply go. Zero new surface, no version skew,
  and it removes a value that was only ever the default spelled less portably.
- **Add `${home}`**, resolved from `dirs::home_dir()`, which works everywhere. More expressive,
  and the honest fix for anyone who genuinely needs a home-relative path — but it triggers the
  `0.1.24` rule: a shared `jobs.yaml` using it fails to load on every host still running an older
  binary.

The first is right for the example. The second is worth having anyway, later, and separately —
and neither changes the fleet's own `jobs.yaml`, which is chezmoi-managed and has its own decision
to make about `${env:HOME}`.

## 7.10 SETTLED: a detached run retries a failed operation (2026-08-10)

**Decision: when — and only when — a run is detached (`run --background`), a failed operation is
attempted twice more at two-minute intervals before the job stops.** Written here before the
code, because it reverses a decision this project had already taken.

### What is being reversed, and why the old reasoning stopped holding

`WIP.md` §12, from the first five unattended runs on Linux, recorded the identical failure — a
21:12 run that died on a DNS lookup to `oauth2.googleapis.com` after a resume — and concluded:

> There is **no retry** after a transient failure — the next hourly run is the retry. For an
> hourly job that is proportional, but it means the unit sits in `failed` state for up to an
> hour, which is what a monitor would see.

**That reasoning is still correct on its own terms, and its premise is what failed.** It assumes a
failed run is an isolated event on a machine that is otherwise awake, so the next hour is a fresh
and healthy attempt. On a laptop in near-constant Modern Standby the premise inverts: the machine
is asleep at most calendar times, `StartWhenAvailable` converts each missed one into a run fired
seconds after a wake, and that is the moment the network is *least* likely to be up. Measured over
one day on the Windows host, **four of ten runs were resume-races and every one of them failed**,
while every run that started on a normally-scheduled awake trigger succeeded. The retry is not
compensating for an unreliable network; it is compensating for the scheduler choosing the worst
instant in the hour.

Note what is *not* claimed: no backup was lost. Each failure was followed by a successful run
(21:49 → 22:05, 07:27 → 08:03). What the failures cost is a skipped `forget` — retention not
running is the failure class that started this project — plus a `failure` verdict a monitor has to
learn to ignore, and up to an hour of latency on a machine that may sleep again first.

### Why it lives in the runner rather than in the task definition

§5.10 measured the alternative and it does not exist: `RestartOnFailure` retries a task that
failed to *launch*, not one whose action exited non-zero. `RunOnlyIfNetworkAvailable` was already
rejected in `schtasks.rs` for a reason that still holds — the repository may be local, and gating
on the OS's idea of connectivity adds a silent skip. So the only component that knows a run failed
is this one.

### The four constraints the shape follows from

**1. Detached runs only.** A hand-typed `run` must behave exactly as before — a person watching a
failure does not want the tool to sit for four minutes before telling them. `--background` is
emitted only by `schedule`, and on Windows only, so the change is confined to precisely the runs
that suffer the race. This is the same gate the console detachment uses, for the same reason.

**2. No new `jobs.yaml` key.** A shared config is only ever as new as the oldest binary reading it
(`NOTES.md` `0.1.24`), so adding a key stops every host still on an older build. Removing one is
safe; adding one is not. The policy is therefore not configurable, which is also this project's
default instinct — a closed set with no escape hatch until a data-integrity case demands one.

**3. `run_job`'s signature does not change.** The state is a process-global set once at startup,
in the idiom `INTERRUPTED` and `NO_CHILD_WINDOW` already establish. `0.2.6` set this precedent
explicitly: adding a parameter to a public function breaks an exhaustive caller and would force a
minor bump under §3 for a detail with one call site.

**4. Only a plain `Failure` is retried.** `Interrupted` is a decision, not a fault, and retrying it
would fight the operator. `Partial` already continues to `forget` by design and needs nothing.
A `--dry-run` never retries: a dry run exists to answer a question quickly.

### What this deliberately does not do

- **It does not distinguish transient from permanent.** rustic exits 1 for everything that is not
  a clean success (§5.3) and classification is by `--json` object count, so "cannot reach GCS" and
  "wrong password" are indistinguishable here. A genuinely broken configuration therefore fails
  three times instead of once, and the run takes four minutes longer to say so. That is the price,
  and it is paid only by scheduled runs, where nobody is waiting.
- **It does not replace §12's rule.** The next scheduled run is still the real backstop; the retry
  narrows a window, and a network that is down for an hour still produces a failed run.
- **It is not extended to systemd or launchd.** §12 shows Linux has the same race, so this will
  probably want to move; but a `Restart=` on a `Type=oneshot` service and launchd's deliberately
  absent `KeepAlive` are different mechanisms with different failure modes, and inventing a shared
  abstraction from one measured platform would be designing past the evidence.
- **It holds the per-job lock across the wait.** Correct rather than incidental: two runs of one
  job must not overlap, `MultipleInstancesPolicy` is `IgnoreNew`, and the lock is non-blocking, so
  an hourly trigger arriving mid-retry is refused rather than queued.

## 7.11 SETTLED: what `doctor` checks, and what it refuses to (2026-08-10)

**Written after building it, unlike §7.9 and §7.10** — because two of the four decisions here
were forced by *measuring* against the live repository, and could not have been reached from
reading. That is worth flagging: this section is a record of the specification being wrong.

### The four candidates, and their fate

| # | check | outcome |
|---|---|---|
| 1 | retention authority — labelled vs unlabelled | **built**, behind `--repository`, with a **different predicate** than §7.5 specified |
| 2 | lock authority — a live restic prune schedule | **built**, always, but **narrower** than §7.6 specified |
| 4 | the profile's credential files exist | **built**, always |
| 3 | a stale chezmoi checkout | **rejected** |

### §7.5's predicate is wrong, and the measurement is the argument

§7.5 asks to warn on *"a mix of labelled and unlabelled entries"* for one host. Against the live
repository:

```
host-a   labelled    84   oldest 2026-08-03T12:03:32   newest 2026-08-09T13:05:25
          unlabelled  38   oldest 2025-09-24T23:00:10   newest 2026-08-03T10:00:30
```

`host-a` is clean — those unlabelled snapshots are its own restic-era history, from before the
2026-08-03 cutover. Under `keep-yearly = 2` they survive about two years, so the specified check
warns on every migrated host continuously for two years. **A check that is always red is not a
check.**

**Shipped predicate: an unlabelled snapshot NEWER than the oldest labelled one.** After a clean
cutover every unlabelled snapshot precedes every labelled one; two live writers interleave. The
same data verifies it: `10:00:30 < 12:03:32`, clean, with the two-hour cutover gap legible.

Corollary kept deliberately: **a host with no labelled snapshots is not in conflict**, it is
un-migrated. `host-c` and `host-g` are exactly that, and the naive reading would have flagged the
control group forever.

### §7.6's check cannot be fleet-wide, and the narrowing is not a shortcut

§7.6 wants *"a repository written by rustic while a restic prune schedule still exists **anywhere
on the fleet**"*. **rusticprofile has no fleet inventory and no remote access, by design** — Part
1 settled that it is a *local, per-machine* scheduler with no central server, which is the entire
reason it exists rather than `rustic_scheduler`. Surveying six other machines means SSH, an
inventory, and credentials to reach them: a different tool.

**Shipped: "on this host", with `doctor` run per host covering the fleet between them.** Stated in
the module docs and the man page, because the gap is real — a host nobody runs it on is not
covered, so this cannot establish the property §7.6 wants.

**Rejected: warn on an installed-but-disabled predecessor unit.** The prune host deliberately
keeps `resticprofile-prune@profile-dot-files.timer` on disk and disabled (§3a invariant 3), and a
tool that calls the correct state a problem gets ignored. It is reported as `ok` **and still
listed**, because one `systemctl enable` re-arms the single measured-unsafe combination.

### Rejected: check 3, a stale chezmoi checkout

The risk is real and is recorded in `NOTES.md` — nothing guards *"a shared `jobs.yaml` is only as
current as the most stale checkout"*, the mirror of `0.1.24`'s binary-version rule. **It does not
belong here.**

- rusticprofile would shell out to `git` and `chezmoi`, to audit a **different tool's** state.
  `AGENTS.md` §2: it is explicitly *not* a config wrapper, and the delegation boundary is the
  thing that keeps this project small enough to be correct.
- It would no-op on any host not using chezmoi. That the whole fleet does is a **fleet** fact, not
  a rusticprofile fact, and building a check that only works because of a local deployment
  accident is how a tool acquires assumptions nobody wrote down.
- The natural home is whatever manages the dotfiles, which already knows how to ask.

**Left unguarded and openly so**, rather than guarded by the wrong component.

### Rejected: pack accounting, i.e. "did our own prune reclaim anything"

Nearly assumed to be covered by check 2, and it is not: check 2 detects a competing restic prune
*schedule*, which says nothing about whether **our** prune's deletion pass works. Verifying that
is a pack count against a stored baseline, and `doctor` has nowhere to keep a baseline — it is
stateless by construction, and giving it state to compare against is a larger design change than
the check is worth. Recorded in `NOTES.md` §4 as its own backlog item.

### Rejected: reporting secret file *permissions*

`0600` versus group-readable is a genuine question and is **not** what the recorded item asks
(*"does not exist or is unreadable"*). Scope was held to existence and openability. A mode check
also has no correct answer on Windows, where the equivalent is an ACL, and a check that means
something different per platform is worse than no check.

### The third severity is a design decision, not a detail

`ok`, `warn`, `unknown`. **A check that could not run must not report `ok`** — an unreachable
repository, a rustic that will not start, a service-manager state word we do not recognise. This
project's most frequently rediscovered failure is *a check that returns the expected answer for
the wrong reason*: the `ls`/`eza` empty listing, `$PIPESTATUS` in zsh, the `wsl.exe` variable that
came back blank, the man-page `sed` that quietly did nothing. `unknown` does not set the exit
code, because "could not look" is not evidence of a problem — it is an absence of evidence, and
the two must not be spelled the same way.

## 7.12 SETTLED: the retry extends to systemd, gated on `--background` (2026-08-11)

**Decision: `schedule` emits `--background` into the generated systemd service, which turns on
§7.10's existing retry for scheduled Linux runs. launchd is deliberately not included.**

> **Written alongside the code rather than before it, which is a deviation from §5.9/§7.9/§7.10
> and is recorded rather than glossed.** Those three sections each preceded their implementation,
> and §7.10 says so in as many words. Here the measurement (§5.11) came first and the design and
> the code were written together, by explicit decision, because §5.11 had already narrowed the
> choice to two candidates and priced both. The risk that ordering carries is that the design
> gets shaped to fit whatever was easy to write — so the two rejected options below are stated
> with what it would have cost to build them, not merely with why they lost.

### Why the retry rather than making the network ordering real

§5.11 measured both. The retry costs ~2 minutes of latency and a transient `failure` verdict; a
working wait-for-network would have cost ~11-12 seconds and produced no failure at all, which is
strictly better **where it applies**. It is not chosen, for two reasons that are about coverage
rather than effort:

1. **It only fixes a network cause.** The retry is indifferent to *why* the run failed, and §5.3
   means rusticprofile cannot tell why anyway — rustic exits 1 for everything. A resume can also
   break a run by way of a not-yet-mounted filesystem, a keyring that is still locked, or a VPN
   that has not re-established; a network gate helps with none of those and a retry helps with
   all of them.
2. **`RunOnlyIfNetworkAvailable`'s rejection transfers exactly.** `schtasks.rs` declined that
   setting because **the repository may be local** — a backup to an external disk needs no
   network, and gating on the OS's idea of connectivity adds a silent skip, which is this
   project's worst failure class. `Wants=network-online.target` on a *system* unit has the same
   property.

**And there is no dependable mechanism to build it out of.** `network-online.target` does not
exist in the user manager at all (§5.11), so the only implementations available are a
`podman`-shipped shim or one this project would write and install itself — a new unit in the
user's `~/.config/systemd/user`, owned by us, ordered ahead of every job. That is a larger and
more invasive change than the retry, for narrower coverage.

**The inert directives stay in the unit and their comment is corrected.** They are right for a
`permission: system` unit, where the target exists; they are inert for a user timer. A comment
asserting a protection that is not there is exactly what this project refuses, so the generated
unit now says which case is which, and a test asserts the caveat is present.

### The gate: an explicit flag, because the environment cannot be trusted to say

§7.10's constraint 1 is that a hand-typed `run` must fail immediately — nobody watching a failure
wants four minutes of silence. On Windows `--background` supplies that gate for free, because only
`schedule` emits it. **systemd needs the same gate and the obvious cheap version does not work.**

`INVOCATION_ID` and `JOURNAL_STREAM` both look like "systemd started this process", and **both are
set in an ordinary desktop terminal** — measured on this host, where the terminal lives in a
transient `app-*.scope`, so a hand-typed run inherits them. Sniffing either would have switched the
retry on for interactive runs, i.e. broken constraint 1 while appearing to implement it. That is
the `hostname(1)` / `fpath` / `%COMPUTERNAME%` family again: **a variable whose name describes what
you want and whose value answers a different question.**

So the gate is the flag, emitted by `schedule` — one mechanism on all platforms, testable as a pure
function of the generator, and with a test asserting `--background` appears for every interval and
priority combination, because dropping it silently removes the retry from every Linux host.

**`--background` now means two things and only one of them is universal.** On Windows it detaches
the console *and* marks the run unattended; on Unix it marks the run unattended and touches nothing
else. That was checked rather than assumed, and it is the property that makes this safe: the
console detachment and the `Stdio::null()` that follows it are behind `#[cfg(windows)]`, so **a
detached run on Linux still sends rustic's stderr to the journal.** Had it not, this change would
have silenced the only diagnostic channel a scheduled Linux run has — and §5.11's whole diagnosis
came out of that channel.

The flag's name is now slightly wider than its Windows origin. Renaming it was rejected: it is on a
published crate's CLI, `0.1.7` establishes that removing a flag is a loud breaking change to take
deliberately rather than incidentally, and "detached" is already how §7.10 and the code describe
the concept.

### What is NOT extended, and why that is not symmetry for its own sake

**launchd is unchanged.** The macOS race is plausible — `launchd.plist(5)` documents coalescing a
missed `StartCalendarInterval` into one run on wake, which is the same shape — but **nothing has
been measured there**, and `0.2.10` was written specifically to avoid drawing a shared abstraction
from one platform. Extending to systemd is justified by §5.11's measurement on systemd; extending
to launchd would be justified by nothing, and the agent is generated by different code with a
different missed-run mechanism.

**No new `jobs.yaml` key**, unchanged from §7.10 constraint 2: a shared config is only as new as
the oldest binary reading it, so the policy stays non-configurable.

### The consequence that has to be said out loud

**Every existing Linux host keeps running its old unit until `schedule` is re-run**, so this change
reaches nobody by upgrading the binary alone. That is the defect §5.11 documents in its own right —
nothing re-emits a unit when the binary changes — and this release makes its cost concrete: a host
that is not re-scheduled gets none of this. The README says so, and it is the strongest argument
yet for the `doctor` check that compares the installed unit against what the current binary would
generate.

# Part 8 — Related state elsewhere

- **`~/Sync/git/resticprofile/UPSTREAMING.md`** — the companion document for the Go fork: PR
  plan, the two config-templating traps, the retention bugs in full, the 7-host inventory, and
  verification recipes. Read it alongside this file.
- **The three upstream PRs are gone.** #670 (log-target error), #671 (eget sbom asset) and
  #672 (systemd test isolation) were opened from a fork that has since been deleted, which
  closes them. The work survives in `UPSTREAMING.md`; re-opening needs a fresh fork and
  **`ghpub`**, not `gh`.
- **The Go tool no longer runs backups on every host.** As of 2026-08-01 one host is cut over
  to rusticprofile — its predecessor timer disabled, not uninstalled, so it is one
  `systemctl --user enable --now` from coming back. The other six are unchanged and still
  authoritative there. Its config (`~/.config/resticprofile/profiles.yaml`) is
  chezmoi-managed, so edits go through `chezmoi add`; the cutover deliberately touched only
  systemd state and left that file alone.
- **Cutover order is not optional** — see §7.5. Disable the outgoing tool's retention before
  enabling the incoming tool's schedule, or both tools delete each other's snapshots.
- **The prune schedule on `host-d` is disabled** as of 2026-08-02, per §7.6. Its unit is still
  installed, so restoring it is one `systemctl --user enable --now` — but it must not be
  restored while any host backs up with rustic. M4 is the precondition, not a formality.
  `host-d`'s *backup* timer is untouched and remains safe: backups take a shared lock, and its
  retention is `host: true`, so it only ever forgets its own snapshots.
- **The prune schedule is hostname-gated in the predecessor's config**, which is worth knowing
  because it is a trap for exactly this kind of investigation: `at: '{{ if eq .Hostname "…" }}
  weekly{{ end }}'` renders to nothing on six hosts, so its absence on the machine in front of
  you proves nothing about the fleet. A claim that "nothing prunes this repository" was made
  on 2026-08-01 on exactly that evidence and was wrong.
- **Upstream maintainer's position on linger** (issue #331) is recorded in `UPSTREAMING.md` — it
  matters for what can be upstreamed, not for this project.
