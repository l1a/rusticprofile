# rusticprofile — plan and session handoff

**Status:** pre-code, **unblocked**. Nothing has been implemented. This document is the full design
plus the reasoning and discoveries behind it. The Part 7 decision — the one thing that blocked all
code — was settled on 2026-07-30 in favour of **Option B** (named snapshot sets, selected per host);
Part 7 records it along with the test results that shaped it.

**Why it reads like a narrative:** it was written as a handoff across Claude accounts, so a
fresh session has no prior context. Everything needed to continue is here or explicitly
pointed at. Written 2026-07-30 on host `host-f`.

**Once the repo is scaffolded**, the forward-looking parts belong in `NOTES.md` (the house
convention for living project state) and this file can shrink to the historical record.

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
  | host-e.local | 25 | 140d idle | macOS |
  | host-f | 4 | active | Linux (this machine, reinstalled 2026-07-22) |
  | host-g.local | 3 | 269d idle | macOS, home is `/Users/user-b` |

  `host-h` (2 snapshots) was forgotten by explicit ID on 2026-07-30.
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

- **M2 — systemd scheduling.** `schedule`/`unschedule`/`status`; `at:` subset → `OnCalendar=`;
  `permission` → user vs system unit dir; `priority` → `Nice=`/`IOSchedulingClass=` **in the unit
  file**, which means in-process priority/ionice code is never written. Covers 5 Linux hosts.
  `status` must surface the `enabled-on-hosts` gate so "host-d has a prune timer and nobody else
  does" is inspectable.
- **M3 — launchd.** `~/Library/LaunchAgents`, `StartCalendarInterval`, `launchctl bootstrap
  gui/$UID`. Covers host-e.local and host-g.local. After M2 because 5 hosts > 2.
- **M4 — lock coordination.** `LockBudget` implemented: wait budget, execution-time crediting,
  stale-lock handling. Only then is `prune` against the shared repo supported.
- **M5 — observability.** Log targets (`O_APPEND`), status file, `--json`.
- **M6 — polish and publish.** Man page via `mandown`, six-shell completions, crates.io via
  `just publish-check` → `just publish`.

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

**Non-goals for v1.** Reading resticprofile config; a `migrate` command; restic as a backend;
Windows; crond/schtasks; groups; **restore** (use rustic directly — say so in the README's first
screen); hooks (rustic has them); Prometheus/metrics (rustic has them); templating in any form,
including a "just one small conditional" escape hatch.

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

## Safety rules observed during this testing

Read-only against GCS (`repoinfo`, `snapshots`); every write test in a throwaway local repo under
`/tmp`, deleted afterwards; no `prune` against GCS; no snapshots deleted on any host.

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

# Part 8 — Related state elsewhere

- **`~/Sync/git/resticprofile/UPSTREAMING.md`** — the companion document for the Go fork: PR
  plan, the two config-templating traps, the retention bugs in full, the 7-host inventory, and
  verification recipes. Read it alongside this file.
- **Three upstream PRs are open** against `creativeprojects/resticprofile`, all awaiting review:
  [#670](https://github.com/creativeprojects/resticprofile/pull/670) (log-target error),
  [#671](https://github.com/creativeprojects/resticprofile/pull/671) (eget sbom asset),
  [#672](https://github.com/creativeprojects/resticprofile/pull/672) (systemd test isolation).
  Open further ones with **`ghpub`**, not `gh`.
- **The Go fork is still the tool running backups on all 7 hosts** and stays authoritative until
  at least M2. Its config (`~/.config/resticprofile/profiles.yaml`) is chezmoi-managed, so edits
  go through `chezmoi add`.
- **Upstream maintainer's position on linger** (issue #331) is recorded in `UPSTREAMING.md` — it
  matters for what can be upstreamed, not for this project.
