# NOTES.md

Living state for **rusticprofile**: architecture, development guidelines, the operating
invariants, current state, the backlog, and the lessons that cost something to learn.

**This file is not a changelog, and `git log` is.** It carried a full per-release log until
`0.2.44` — about 5,300 of its 6,080 lines, reaching back to `v0.0.1` — which buried the parts
that are load-bearing. That log is dropped rather than migrated. Its durable content is
distilled into §5, and the log itself is one command away:

```bash
git show 366a984:NOTES.md      # the last version carrying the per-release log, v0.0.1–v0.2.44
```

**Citations of the form `0.2.13`**, in `PLAN.md`, `AGENTS.md` and source comments, name that
release's entry in the log above. They are left as they are: the version is a stable key, and
the commit that holds the text is recorded here.

`PLAN.md` is the design record: how the design was reached, every rejected alternative with its
reason, and the measurements behind each decision. It does not get rewritten as the project
moves; this file does.

---

## 1. Project Overview

- **Name**: rusticprofile
- **Goal**: a local, per-machine scheduler and orchestrator for `rustic` backups — no central server
- **Key technologies**: clap, serde, rustic (as the delegated backend), systemd / launchd / Task Scheduler
- **License**: GPL-3.0-or-later
- **Repository**: https://github.com/l1a/rusticprofile

**The delegation boundary is the single most important thing to understand.** rustic owns
all backup configuration: repository, sources, excludes, retention policy, hooks,
environment, metrics. rusticprofile owns scheduling, per-host gating, operation
sequencing, exit classification and lock coordination. It builds no backup flags — a job
resolves to `rustic -P <profile> <operation> [--name <set>]...` plus the hostname flags
(`--host` / `--filter-host`, since `0.1.34`) and `--json` on `backup`, and nothing more.

There is exactly one class of exception, and it is read-only: rusticprofile parses
`rustic.toml` to enumerate `[[backup.snapshots]]` names, so it can reject a `--name` that does
not exist, and to refuse a `sources` entry rustic would not expand. Both are bought and paid for
in `PLAN.md` §7.2 and §5.9 — rustic fails silently in each case, so nobody else can catch it.

---

## 2. Codebase Architecture

```
src/
  lib.rs          documented module index — the map of where things live
  main.rs         thin bin: parse -> dispatch -> report -> exit
  cli.rs          clap derive; only what is implemented is declared
  config/         parse, host-gate, interpolate, validate jobs.yaml; annotated examples
  rustic/         build the rustic argv; classify its exit
  exec/           spawn, forward signals, mask secrets in logs
  run/            operation ordering, retry, status record, run log; LockBudget seam
  report/         owo-colors output; the one timestamp renderer
  doctor/         the checks a hermetic `--check` cannot make
  schedule/       systemd units, launchd plists, Task Scheduler definitions

tests/cli_tests.rs        one integration test file, driving the real binary
tests/golden/             the argv each job would run, one element per line
docs/rusticprofile.1.md   man page source (mandown); .1 is generated, never edited
scripts/                  vendored helpers (install_*, gate_conformance, text_check) and
                          this repo's own guards (copr_check, wip_check)
scripts/hooks/            real git hooks — the enforcement layer
```

Single crate, lib plus thin bin, **no workspace**. `retch` is a workspace only because it
has a separately publishable library; there is no second consumer here, and a workspace
costs real complexity in the publish recipes.

| Module | Does |
|---|---|
| `config/job.rs` | the `jobs.yaml` schema; every struct is `deny_unknown_fields` |
| `config/hosts.rs` | hostname resolution, short form, host matching |
| `config/interp.rs` | the closed `${…}` variable set |
| `config/schedule.rs` | schedule vocabulary |
| `config/rustic_toml.rs` | read-only: snapshot-set names, sources and the delegated profile out of `rustic.toml` |
| `config/example.rs` | the annotated starting-point configs behind `config --example` |
| `config/validate.rs` | batched rules, `Violation` / `ValidationErrors` |
| `config/paths.rs` | XDG locations for both config trees, on every platform |
| `rustic/invoke.rs` | build the argv; the secret-flag and flag-inventory assertions |
| `rustic/exit.rs` | the verdict: partial vs failed, from the `--json` object count |
| `rustic/retention.rs` | the `retention` view over `forget --dry-run --json` (§3a invariant 4) |
| `exec/` | spawn, stdio modes, signal forwarding (Unix), job object (Windows), redaction |
| `run/steps.rs` | operations in order; stop on failure, continue on partial; the detached-run retry |
| `run/lock.rs` | local per-job lock; the M4 repository-lock seam, deliberately `None` |
| `run/status.rs`, `run/log.rs` | the status record and the run log; one `STAMP_FORMAT` for both |

**The load pipeline runs in a fixed order: read → parse YAML → host gating → `${…}`
interpolation → batched validation.** Parsing before substitution is what makes two of the
predecessor's templating traps structurally impossible: a comment is gone before anything is
substituted, and no substitution can produce a document that then fails to parse. `${date:…}` is
validated at load time but resolved per run, so a generated unit can never freeze one day's date.

**What validation refuses**, each closing a way a config could quietly do less than it says:
unknown keys anywhere; an unknown or duplicate snapshot-set name; a set that does not exist in the
rustic profile (checked as *declared*, so a typo behind another host's gate is still caught); a
set name starting with `-`; sets on a job that does not back up; every set gated away on a host;
`enabled-on-hosts: []`; a relative log path; an unknown `${…}` variable; `${job}`/`${profile}`
inside `defaults`; an unset `${env:…}`; a malformed `${date:…}` format; a duplicate operation; a
non-filename-safe job or profile name; an empty job list; a `sources` entry containing `~` or `$`;
a `forget` with no scoping; misplaced `[forget]` filters; an implicit `group-by`; and, under
`hostname: rustic`, a `filter-hosts` that cannot match this host.

**Four decisions in `exec/` worth knowing before changing it.** stdout is captured (the `--json`
objects) and stderr inherited (rustic's progress), so an operator watches live while
classification still gets its input. An interrupt is forwarded and then *waited on* — orphaning a
running rustic would leave it writing to a repository shared by seven machines. The environment is
inherited unmodified; `exec/env.rs` only selects what is worth showing. And redaction is a
backstop, not the control: the control is that secrets never enter this process (`PLAN.md` §4.1).

---

## 3. Specific Development Guidelines

- **Man page**: do not edit `docs/rusticprofile.1` directly. It is generated from
  `docs/rusticprofile.1.md` by `just man` (mandown), with the version read out of
  `Cargo.toml`. Run `just man` after any version bump and commit the result — `just pr`
  fails if it is dirty. `just install` installs the committed page and does not need mandown.
- **Quality gate**: `just check` = the golden-staleness gate, `standard-check` (the vendored
  helpers' self-tests), `copr-check`, `text-check`, `wip-check`, `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings`. CI runs fmt, clippy and the golden gate itself; `pre-push` runs all of it.
- **Pre-PR gate**: never call `gh pr create` directly — run `just open-pr`, which runs
  `just pr` first and only proceeds if it passes. `gh` and `git` have no hook of their own
  for "a PR is about to open", so this recipe is the one call site that can gate it. It is
  a Justfile recipe rather than agent configuration precisely so it binds a human, Claude,
  Gemini or anything else identically.
- **Git hooks are the enforcement layer, not agent config.** `scripts/hooks/pre-push`,
  installed by `just install-hooks`, runs `just check` before every push regardless of what
  invoked it. Prefer this pattern over anything under `.claude/`, which only binds one
  vendor's tool and is invisible to everyone else.
- **Version bump on every PR, and it is a patch bump.** `0.0.x` ran until Milestone 1
  delivered a tool that could actually run a backup; `v0.1.0` was reserved for that and is
  now released. **From `v0.1.0` until `v1.0.0`, every PR is a patch bump** — `0.1.7`,
  `0.1.8`, and so on — regardless of how much it adds or changes. This supersedes the
  sibling repos' "minor for features" rule, which does not apply here.

  **A minor bump means the library API broke, or a milestone landed.** Not a new feature, and
  not a CLI change. The precedent is `0.1.7`: `schedule` flipped to arming the timer by
  default and `--enable` was *removed* from a crate already published to crates.io, and that
  is still a patch bump. Nothing links against rusticprofile, and a removed flag fails loudly
  at parse time rather than quietly changing what a command does — so it costs a user a clear
  error, not a silent surprise. The CLI is expected to move before 1.0; say so in the commit
  message rather than in the version number.

  **`0.2.0` is the precedent for the other side of that rule**, and it is worth recording
  because "a whole new platform" is *not* by itself the reason. Windows support added a variant
  to the public `schedule::Backend` enum, which breaks any exhaustive match downstream — that
  is the library API breaking, the documented trigger. It also reversed a declared v1 non-goal,
  which is milestone-shaped in a way `0.1.26`/`0.1.27` were not: those added a *backend* for a
  platform already in scope, and were correctly patch bumps. If a future change adds a platform
  without touching a public type, it is a patch. A new `cli::Command` variant is a patch too
  (`0.2.13`'s `doctor`, `0.2.27`'s `retention`).

  Tag `v$VERSION` from a clean `main`; never `cargo publish --allow-dirty`.
- **Deliberate absences.** No `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `rust-toolchain.toml`, MSRV declaration, `[lints]` table, `#![deny(...)]` or
  `CHANGELOG.md`. Their absence is the convention — do not add them. Three deliberate
  deviations from the `retch` template, so they are not mistaken for oversights: no
  `post-merge` hook (retch's uploads benchmarks to a dashboard; there is none here), no
  `benches/`/criterion yet (§4), and no `.cargo/audit.toml` (it arrives with the first advisory
  that needs a justified ignore).
- **Backup safety**: read-only operations against a production repository are fine; every
  write test goes to a throwaway repository under a temp dir, deleted afterwards; never
  **`restic` prune** against a shared repository any rustic client writes to (`PLAN.md`
  §7.6 — `rustic prune` is safe by design and is the only prune that may run); never delete
  snapshots without explicit per-step authorisation. See `AGENTS.md` Part 2 §3.
- **No live infrastructure identifiers in tracked files.** The repository is public, so this
  is now permanent rather than a pre-publication chore: no real hostnames, bucket names,
  project ids or home paths. Hosts are `host-a`…`host-h`; paths are `/home/user`. Sweep the
  diff, the commit message and the PR body before opening a PR, with word boundaries — a
  redacted hostname can hide inside an ordinary English word (§5.6).

---

## 3a. Operating invariants — the rules that bite

**These are the rules that can destroy data if broken, gathered in one place.** Every one was
found the hard way and measured; every one is silent when violated. They were promoted here
from `PLAN.md` Part 7 on 2026-08-04 — that file keeps the full finding and the measurements
behind each, under the same section number, but **this is where they are maintained.**

A single sentence connects all of them: *this project exists because a backup that quietly
does less than it says is worse than one that fails loudly.* Each invariant closes one route
to that outcome.

### 1. A job using named snapshot sets MUST group retention by label

`group-by = "host,label"` in the rustic profile. Not `"host"`, and not rustic's default
`"host,label,paths"`.

With `"host"` alone every named set lands in one group and competes for a single retention
slot, so only whichever finished last survives. Measured on the live repository: a dry run
kept a **0-byte** `nushell` snapshot and deleted the **6,256-file** `core` one, and reported
success. With `paths` in the key, a renamed source mints a fresh group with its own full
quota — that is how 2810 snapshots survived a policy capping ~49 per host.

**Label, not paths:** a set's label is stable by construction, its path list is not.
*Evidence: `PLAN.md` §7.3.*

### 2. Exactly ONE retention authority per (repository, host)

Two tools may *back up* the same host concurrently — backups are additive and, with prune
disabled, nothing is destroyed. **Two tools may not `forget` it.**

Invariant 1 protects the sets from each other; it does nothing about a second tool applying
its own retention to the same host, and a machine mid-migration has one by definition. The
predecessor's `group-by: host` with `path`/`tag` off swept our labelled snapshots into its
bucket and deleted a **395.591 MiB** `core` snapshot in favour of a 0-byte one written one
second later. It ran both ways: our correctly-grouped `forget` deleted one of *its* 397.9 MiB
snapshots by the mirror-image mechanism.

**Migration means moving that authority, not overlapping it.** The ordering is not optional:
**disable the outgoing tool's retention BEFORE enabling the incoming tool's schedule**, and
confirm from the repository rather than from either tool's own report.
*Evidence: `PLAN.md` §7.5.*

### 3. Exactly ONE lock protocol per repository

**Never run `restic prune` against a repository any rustic client writes to.** This is the
one measured-unsafe combination, and it is unsafe because restic deletes packs immediately —
safe only by virtue of an exclusive repository lock that rustic neither takes nor honours.
Measured: 14 packs (487.780 MiB) deleted from under an in-flight rustic backup, repository
then failing `restic check --read-data` with five data packs missing.

**`rustic prune` is safe and is the only prune that may run here.** rustic is lock-free *by
design*: prune marks packs and deletes them only after `--keep-delete`, 23 hours by default.
Verified — a default `rustic prune` left every pack on disk; only `--instant-delete` removed
them. The deletion half is proven on the live repository too (§5.3).

| combination | safe |
|---|---|
| `rustic prune` + rustic backup | **yes** — the 23-hour grace period |
| `restic prune` + restic backup | **yes** — restic's repository lock |
| **`restic prune` + rustic backup** | **NO — measured corruption** |
| `rustic prune` + restic backup | probably — *reasoned, never measured* |

**The fourth row has never been tested.** Finishing the migration is what retires it.

> **"Tool X lacks the mechanism I expected" is not "tool X is unsafe."** Reading rustic's own
> documentation before generalising from the measurement was the missing step, and skipping
> it cost a day of exposure plus a prune schedule disabled for no reason. M4 is defence in
> depth, not permission.

*Evidence: `PLAN.md` §7.6.*

### 4. The delegation boundary — what this tool may emit

A **job** invocation is `rustic -P <profile> <operation>`, plus `--json` on `backup`, plus one
`--name` per enabled snapshot set, plus `--host`/`--filter-host` unless `hostname: rustic`.
**Those are the only flags rusticprofile ever emits**, and a test in `rustic/invoke.rs` asserts
it against every built argv. That test carries the instruction: *if it needs changing, the
delegation boundary is moving and that belongs in `PLAN.md` first.*

**A passthrough is acceptable only where it is read-only and adds no flags.** `snapshots`
qualifies and exists; `check` would qualify. `forget` and `prune` do not — destructive, and
their scoping belongs in the rustic profile where a flag typed at a prompt cannot contradict
it. `restore` never does.

**A constructed read-only command is a third category, and `retention` is the only one**
(`0.2.27`). It emits `forget --dry-run --json`, so it is not a passthrough — it adds flags —
and `forget` is on the excluded list because it is destructive, which `--dry-run` is measured
not to be. What makes it permissible is that the dry run is **unexpressibly-absent** rather
than merely intended: `retention_argv` calls the same `build_argv` the scheduled `forget` goes
through, with `dry_run: true` hardcoded and no parameter that could switch it off, so the
preview provably differs from the real operation by exactly `--dry-run` and `--json`. Two tests
assert that, one of them at ladder rung 2 against the argv actually spawned. **The bar for the
next command of this shape is that same standard**: not "it is read-only in practice" but "the
destructive form cannot be constructed". *Evidence: `PLAN.md` §7.14, §5.12.*

Two deliberate exceptions, both **read-only**, both because nothing else in the chain can
catch a silent failure: rusticprofile parses `rustic.toml` to validate every `--name` it
emits (rustic ignores an unknown one whenever a valid one is also given, exit 0, no
diagnostic), and to refuse a `sources` entry containing `~` or `$` (rustic expands neither,
and the result is a successful 0-byte snapshot that then wins its retention slot under
invariant 1).
*Evidence: `PLAN.md` §7.2, §7.8, §5.9.*

### 5. The dangerous decisions live in rustic's config, so the shipped example carries them

The delegation boundary means rusticprofile owns almost nothing — so nearly everything that
can silently destroy data is in `rustic.toml`. `config --example rustic` ships that knowledge
annotated, and a test puts both examples through the real binary so they cannot drift from
the validator.

| the trap | |
|---|---|
| `opendal:gcs`, not restic's `gs:` — that scheme does not exist in rustic | §5.1 |
| scoping filters go in `[snapshot-filter]`; under `[forget]` rustic **accepts and ignores** them | §5.5 |
| `group-by = "host,label"` — invariant 1 | §7.3 |
| exclusion globs need a leading `!`; a bare pattern is an *include* filter | §7.2 |
| split sets by how reliably the path exists — rustic hard-fails a whole set on one missing source | §5.7 |
| `filter-hosts` matches the recorded name exactly; a mismatch matches zero snapshots and retention silently never runs | §5.9 |
| exclude the password file and cloud credentials, or the key goes inside the lock | §4.1 |

### 6. Corollaries worth stating once

- **`jobs.yaml` is byte-identical on every host; `rustic.toml` must be generated.** rustic
  expands neither `~` nor `$VAR` and has no env-var route to the host filter. A consequence
  with teeth: a shared `jobs.yaml` is only ever as new as the **oldest binary** reading it —
  so **removing** a key is safe fleet-wide and **adding** one is not — and only as current as
  the **most stale chezmoi checkout** reading it.
- **rusticprofile applies XDG rules on every platform, macOS and Windows included.** Not a
  preference; the requirement that one line of one file mean one thing across the fleet.
- **`doctor` catches invariants 2 and 3 from outside the config** (`0.2.13`). A host with
  restic-written snapshots **newer than its rustic cutover** has a second retention authority; a
  restic prune schedule armed on this host is a second lock protocol. Note the first is an
  *ordering* test, not "mixes labelled and unlabelled" — a migrated host legitimately holds years
  of unlabelled history, and the naive form warns for two years. The second is **per-host**;
  rusticprofile cannot see the fleet. `PLAN.md` §7.11.

---

## Current State (v0.2.45)

**Which version is released is deliberately not stated here.** The newest tag, the GitHub release
and crates.io's `max_version` are the record — and they are three answers, not one, as §5.5 records
for the AUR. This section once announced the newest release in prose and was wrong most of the time
it was read (four corrections in two weeks, each found by accident), while the gated header line
directly above it — `just pr` refuses unless it matches `Cargo.toml` — stayed correct throughout.
**A fact nothing checks is a fact that rots.**

**What the tool is today.** Milestones 1, 2, 3 and 5 are complete and M6 is effectively delivered;
**M4, repository lock coordination, is the only unbuilt milestone** and is deferred by decision
(§3a invariant 3). It schedules and runs real backups on **systemd, launchd and Task Scheduler**,
and is shipped as GitHub release binaries (Linux, macOS and Windows, x86_64 and arm64), on
crates.io, in the AUR and in Fedora COPR.

| command | what it is |
|---|---|
| `config --check` / `--show` / `--example` | hermetic: no rustic, no repository, no network. `--show` prints both halves of the effective configuration |
| `plan` | the exact argv a job would run — the whole contract with rustic |
| `run` | lock, ordered operations, classification, log, status record; `--background` for scheduled runs |
| `schedule` / `unschedule` / `status` | the three backends; `status --json` under `schema: 1` |
| `snapshots` | read-only passthrough |
| `retention` | the constructed read-only `forget --dry-run --json` view (§3a invariant 4) |
| `doctor` | the checks a hermetic `--check` cannot make; exit 3 on a warning |

**Behaviour a reader needs before changing anything:**

- **`run` stops on failure but continues on partial**, so a backup that partly worked still reaches
  retention. A partial is only claimed on at least one parsed `--json` object — the safe direction
  when uncertain is failure, because the opposite error runs `forget` after a backup that saved
  nothing.
- **A detached run (`--background`) retries a failed operation twice more, two minutes apart**
  (`PLAN.md` §7.10, §7.12). `schedule` emits the flag on systemd and Task Scheduler, not launchd. A
  hand-typed run fails immediately.
- **Every generated unit names rustic by absolute path**, and **a unit is generated once**: an
  upgraded binary changes no installed unit until `schedule` is re-run (§4).
- **rusticprofile owns the recorded hostname** (`defaults.hostname`: `short` by default, `full`, or
  `rustic` to defer) and prints it under `config --check` whenever it differs from the OS's.
- **Alert on `last_success`** from `status --json`. A timer can be armed, green and firing while
  every run fails, and a Mac at the login window runs nothing at all.

**The release-verification rule.** A publish is verified three ways, never from `cargo publish`'s
own report: the registry API, `cargo install --locked` into a throwaway root, and **running that
binary** — the step that caught `0.2.16`'s false help text and `0.2.20`'s time-zone conversion.
And a tag is pushed only after the post-merge `full-test` has gone green on that exact commit
(§5.5).

---

## 4. Backlog

**Live items only.** When one is finished, delete it rather than striking it through — a backlog
carrying finished items overstates the work remaining. If the work left a rule behind, the rule
goes in §5.

### 4.1 Product

- [ ] **Nothing re-emits a generated unit when the binary is upgraded, and nothing notices.**
      `0.2.16` found a host whose units predated `v0.1.10`, so its first run after every boot had
      failed for eight days while `status` said `active`. Squarely inside the delegation boundary —
      the unit is this tool's own artefact — so a `doctor` check comparing the installed unit with
      what the current binary would generate needs no other tool. The manual form already exists:
      `schedule -n <job> --write-only --unit-dir <tmp>` and diff. `PLAN.md` §7.11's reasoning for
      rejecting the stale-checkout check does **not** transfer, so this wants its own decision.
- [ ] **Nothing reports that the AUR has fallen behind**, and it is the only channel that drifts by
      construction: `.SRCINFO` needs podman, so the AUR step needs a Linux host, and it gets
      skipped by default rather than by decision. Not a `just check` item — that would put a
      third-party registry call inside an offline gate, which `0.2.21` refused. A `doctor`-style
      check or a release-procedure step is the likely shape.
- [ ] **Nothing re-measures `PLAN.md` §5 and §7 when rustic moves.** Those measurements are the
      evidence behind §3a, all taken against rustic 0.11.3. They were re-measured by hand against
      **0.11.4** on 2026-08-24 and every load-bearing one held — but that was a person deciding to
      look. **A measurement is only as true as the version it was taken against.**
- [ ] **`PS_COMP` points where PowerShell does not read on Windows.** `$PROFILE` there was measured
      under OneDrive folder redirection and does not source `~/.config/powershell`, so the file
      written there is dead. Same silent-no-op shape as the nushell path fixed in `0.2.14`, but not
      the same fix: there is no `%APPDATA%`-shaped variable to key off. Probably ask
      `pwsh -NoProfile -c 'Split-Path $PROFILE'` when one is on PATH, else fall back to XDG.
- [ ] **Windows: `permission: system` has never been registered and run.** Generated and
      unit-tested only, and it is the documented answer to the login caveat, so it is the one part
      of the platform resting on reasoning. Needs an elevated shell, and should be decided together
      with where a SYSTEM task finds its credentials.
- [ ] **Windows: no automated argv round-trip test.** `quote_argument` is unit-tested against the
      MSVCRT rules and the round trip was verified by hand in `0.2.0`; nothing re-checks it.
      `tests/cli_tests.rs` is where it belongs — `CARGO_BIN_EXE_rusticprofile` is a cooperating
      child (`src/exec/mod.rs` points here).
- [ ] **`rusticprofile status | more` can print a panic on Windows.** There is no `SIGPIPE` to
      restore. "The fix does not apply here" is not "the bug does not happen here".
- [ ] **The launchd resume race is unmeasured**, and launchd is deliberately outside the retry until
      it is. `launchd.plist(5)` documents coalescing missed runs on wake, so the race is plausible.
- [ ] **Two systemd timings are unexplained or unmeasured.** Boot catch-ups fired 185 s and 204 s
      after the timer started, against milliseconds on resume and in the probe; a pre-NTP clock step
      is disproved. And whether `NextElapseUSecRealtime` moves between queries has never been
      measured — it decides whether `status`'s `(±N min)` annotation extends beyond Task Scheduler,
      which is a one-line change at the call site.
- [ ] **First benchmark**, with `benches/`, criterion and `[[bench]]`. `just bench` says there is
      none rather than pretending. Config parsing is the likely first candidate.
- **Decided, recorded so they stay decisions:** `retention` has no `--json` and `status --json`
  has no `next_run_iso` — both are schema promises nobody has asked for (`0.1.23`, `0.2.22`); a
  stale chezmoi checkout is unguarded here on purpose, because auditing another tool's checkout is
  the dotfile tooling's job (`PLAN.md` §7.11); and the inert `network-online.target` directives on
  a user unit stay, with a comment saying so, because §7.12 chose the retry over a wait-for-network.

### 4.2 Tooling and process

- [ ] **A Pre-PR Checklist section in `AGENTS.md` Part 2, mirroring retch's §4.** Until it exists,
      `just pr`'s manual checklist is the checklist, and it is the one that binds.
- [ ] **`just check` is not portable to a default Windows PATH**, because `golden-is-current` is a
      bash shebang recipe and a `check` dependency. Everything else in the chain is plain.
- [ ] **`merge-pr` does not update `WIP.md`.** Both siblings rewrite a WIP state block on every
      merge (`retch`'s `update_wip.py`, `etr`'s `reset_wip.py`); here the recipe only prints a
      reminder. The guard half of their change landed here (`just wip-check`, `0.2.45`); the
      updater did not, and would need the same byte-preserving, exactly-one-match discipline.
- [ ] **`scripts/text_check.py` is vendored with three divergent bodies at `TEMPLATE_VERSION = 1`.**
      This copy's docstring also cites a `WIP.md` entry (2026-08-12) that the `0.2.45` prune
      removed. Patching one copy deepens the drift; it needs one coordinated bump across all three.
- [ ] **`just aur-verify` hides its own failure.** It sends `pacman` to `/dev/null`, so a broken
      container reports only `container verification failed` (`0.2.28`, §5.5). A visible failure
      path is a candidate change, deliberately not made inside a packaging-only release.
- [ ] **A docs-only CI path — an idea with three shapes, not a plan.** The `pull_request` `paths:`
      filter already exists and can never fire, because every PR bumps `Cargo.toml` and
      `Cargo.lock`; the `push` trigger has no filter, so a docs-only merge runs all seven
      `full-test` legs. Only a `changes` job that diffs against the base could shrink that. Whatever
      shape, it must not drop the golden gate or the smoke test from the release path (`0.2.1`).
      Also open in the same place: `build` runs `cargo build` then `cargo test`, compiling twice —
      deliberate log separation, or vestigial?
- [ ] **`main` has no branch protection.** Recommended ruleset: require a PR with 0 approvals;
      require `build (fedora-x64)`, `build (macos)`, `build (ubuntu-arm)`, `build (windows)` and
      `audit`; block force pushes and deletions; allow admin bypass. **Never require `full-test`,
      `build-release` or `release`** — they report SKIPPED on a PR and would block it forever. Web
      UI only: both tokens here get HTTP 403 from the protection API. Not authorised yet.

### 4.3 Fleet and operations

Not code, but the operating rules in §3a make them this project's business.

- [ ] **Nothing records where the repository password lives outside the fleet.** It is excluded
      from the backup by design, so it exists only on each host; if every machine were lost it is
      not written down how the repository would be opened. Cheapest item here and the only one
      addressing a single point of failure. Key rotation has no recorded story either.
- [ ] **The GCS credential still sits directly in `~/.config`**, excluded by a glob added first
      (`!**/.config/rustic/*.json`) plus a hard-coded filename. Remaining: move it into
      `~/.config/rustic/`, repoint `credential_path`, then drop the filename line once every host's
      old copy is gone.
- [ ] **The passphrase move to `~/.config/rustic/dot-files.pw.txt` is pending on `host-e`,
      `host-c` and `host-g.local`**; then collapse the templates' `stat` fallback to a plain path. Check
      with `rusticprofile snapshots`, not `config --check`, which reports `ok` on a configuration
      that cannot authenticate.
- [ ] **Rung 9 is a decision, not a task.** `host-c` and `host-g.local` are the control group; cutting
      them over spends the baseline. `host-c`'s restic backup timer is dormant, not disabled, so a
      `Persistent=true` catch-up could land inside the prune window — accepted knowingly
      2026-08-04; the cheap close is to disable it once.
- [ ] **Unverified since 2026-08-04:** that `host-e` records `host-e` rather than `host-e.local`
      (`0.1.34`), and whether the stranded `.local` snapshots have been purged — irreversible,
      per-step authorisation. `host-e`'s launchd plist was also generated once and has never been
      audited against the current generator.
- [ ] **`host-f`, the development machine, is Fedora again (reinstalled 2026-09-16, per `~/AGENTS.md`)**, so every
      Windows-era fact about it is dead, and it was removed from the `gnupg` set's
      `enabled-on-hosts` while Windows had no keyring. Re-check its schedule, its secrets and that
      gate before trusting any claim about it.

---

## 5. Hard-won lessons

Each of these cost something to learn. Most are one instance of a single idea, which is also
this project's reason to exist: **a check that returns the expected answer for the wrong reason.**
The version in brackets names the release entry with the full story (`git show 366a984:NOTES.md`).

### 5.1 Verification — the oracle that answers a different question

- **A check is only worth what its oracle is worth.** The recurring shape is asking something
  *near* the thing under test: a fixture shelling out to `hostname(1)` instead of the function the
  binary uses (`0.1.5`); a non-interactive `zsh -c` asked about an interactive `fpath` (`0.1.14`); a
  fixture needing a named time zone when it wanted an instant (`0.1.21`); a Windows check run in a
  shell whose PATH had already been repaired (`0.2.1`); a throwaway config naming rustic explicitly,
  so the bare-name `PATH` lookup a real user takes was never exercised (`0.2.5`); fixtures built
  with `backup --time`, which lands on a whole second, while every real snapshot carries
  nanoseconds (`0.2.27`). **Ask the thing that defines the semantics, and build fixtures in the
  shape production produces.**
- **Watch every guard fail before trusting it** (`0.2.17`), from a clean baseline asserted first
  (`0.2.38`), and **assert the sabotage applied**: a `str.replace` that matched nothing once made a
  "sabotaged" run pass and read as evidence (`0.2.39`). A guard that rendered its own input with the
  very constant it verified could not fail at all; the fix was deleting the duplication — one
  `STAMP_FORMAT` — not a better test (`0.2.20`).
- **Nothing is not green.** An empty CI rollup is not a pass (`0.2.1`); a check that could not run
  reports `unknown`, never `ok` (`0.2.13`'s third severity); a sweep or exclusion check reporting
  zero needs a positive control that proves it could have found something (`.ssh` present, `.pw.txt`
  absent — `0.1.29`, `0.2.22`). **An absence of evidence must not be spelled the same way as
  evidence of absence.**
- **Absence bounds a rate; it cannot establish a property.** "No suspends in ten days" became "the
  host never suspends", and the next suspend falsified it within the hour (`0.2.16`).
- **A defect that fails every candidate run can hide a second defect on the same runs.** Five loud
  `PATH` failures masked the resume race behind them; a failure count says nothing about how many
  causes it contains (`0.2.16`).
- **Use the tool; a green suite is not the evidence.** Running it against the real configuration
  found what no test could six times (`0.2.5`, `0.2.6`, `0.2.10`, `0.2.20`, `0.2.22`, `0.2.27`).
  "The window is gone" is not an oracle for "the backup still works" (`0.2.6`).
- **Verify from the repository, not from any tool's report** — snapshot counts by id, pack counts,
  other hosts' counts unchanged (§3a invariant 2). A falsifiable prediction written down before the
  event is what proved the prune (`0.2.30`).
- **Shell and pipe traps that each produced a confident wrong answer here:** `$?` after `| head`
  is head's (`0.2.23`); `curl … | sh` exits 0 on a failed download and `sh -e` does not fire
  (`0.2.19`); zsh `MULTIOS` makes `2>&1 >/dev/null` tee rather than discard (`0.2.27`); zsh does not
  word-split `for id in $IDS` (`~/AGENTS.md` §11); `grep -c ".cache"` matches `_cache` (`0.1.29`);
  a pattern written for compact JSON matches nothing in a pretty-printed file; a `grep` over a
  script's output swallowed the error it existed to show; `| head -40` truncated a scan that was
  then read as complete (`0.2.2`); `ConvertFrom-Json` re-renders RFC 3339 in the locale (`0.2.22`);
  PowerShell rewrites backslashes in native arguments, so measure through `just --evaluate`, not a
  `sh -c` probe (`0.2.14`); `grep -c $'\r'` from an agent shell counts lines (`~/AGENTS.md` §17).
- **Never let a `--json` run be the oracle for a message `--json` suppresses.** rustic moves its
  "successfully saved" confirmation to stdout under `--json`, so a stderr grep reported the
  reassuring "0.11.4 no longer saves the 0-byte snapshot" — false.
- **Do not make structural edits to safety-critical files by pattern.** A scripted edit matched
  `[forget]` inside a comment and deleted `[snapshot-filter]` (`PLAN.md` §7.4); a global regex put
  `@` into three shebang recipes (`0.2.2`). Delete by exact filename, never a glob, and run
  `config --check` after any edit to a rustic profile.
- **Audit a Justfile by running its recipes, not by reading it.** Reading it three ways gave three
  wrong answers (`0.2.2`); running every recipe found the one that destroyed a tracked file
  (`0.2.12`). `just --show <recipe>` is the oracle for what a recipe body is.

### 5.2 Testing conventions

- **Choke points, never per-test discipline.** Every integration test spawns through one
  `command()` that redirects `XDG_STATE_HOME`, `XDG_RUNTIME_DIR` and the temp variables — the suite
  once overwrote the live `dot-files` status record with a fabricated success (`0.1.28`) and
  contended with a live run for its machine-wide lock (`0.2.19`). `exec` tests go through one
  `run_locked`, whose guard is `#[must_use]` (so `let _ =` is a clippy error) and whose poisoning is
  recovered (`0.1.33`). **A guarantee every future author must remember is not one.**
- **When a test races, ask whether the test breaks a documented constraint.** `run` is documented
  non-reentrant and production honours it; the tests did not, so the lock went in the tests
  (`0.1.33`), and a mutex-lifetime test got a mutex nothing else touches (`0.2.0`).
- **Take the environment as parameters.** `std::env::set_var` is `unsafe` in edition 2024 and races
  the whole binary, so path and `PATHEXT` logic are pure functions of their inputs (`0.1.25`,
  `0.2.5`), and platform choices use a runtime `cfg!` where possible so both branches run on any
  host.
- **The rustic-backed integration tests (four) skip with a printed notice when rustic is absent**,
  so CI never runs them and a local run without rustic looks identical in the totals. Count them
  from `tests/cli_tests.rs`, not from prose. `aur-verify` installs rustic in its container for
  exactly this reason (`0.1.1`).
- **The argv is the whole contract.** `plan --format lines` byte-identical before and after is the
  check for any upgrade, deploy or config change: if the argv did not move, the new binary cannot
  be asking rustic for anything different. Goldens are checked by content hash, and one golden set
  serves every platform because the separator is normalised (`0.2.0`).
- **Throwaway verification jobs get their own name.** The per-job lock is machine-wide by design,
  so a probe named `dot-files` starves the real hourly run.
- **Guard sentences as well as behaviour** where a sentence is what a reader relies on: the
  `--background` help text (`0.2.17`) and `retention`'s "without changing anything" (`0.2.27`) each
  have a test. A test that the example config *parses* cannot notice a false comment in it (`0.1.27`).
- **Beware tests that can only pass**, such as asserting `is_ascii()` over ASCII-only fixtures
  (`0.2.27`).
- **`clippy --all-targets`, on every leg.** Test code went unlinted from the scaffold to `0.2.30`,
  and `#[cfg(windows)]`/`#[cfg(unix)]` test modules are only linted on the platform that compiles
  them, so each CI leg is the oracle for its own half.

### 5.3 rustic, as measured

Evidence in `PLAN.md` Parts 5 and 7. Measured against 0.11.3; the load-bearing ones re-measured
against 0.11.4 on 2026-08-24 and unchanged.

- **Everything that is not a clean success exits 1**, a partial backup included. Classify by
  counting `--json` snapshot objects on stdout — and they are **concatenated pretty-printed JSON**,
  not JSON-lines, so only a streaming parser counts them. `program_version` inside them does not
  report the binary.
- **An unknown `--name` beside a valid one is silently ignored**, exit 0; alone it fails.
- **`~` and `$VAR` are not expanded**, and because the result is a *relative* path it skips the
  missing-source hard failure: rustic warns, saves a **0-byte snapshot** and exits 0. An *absolute*
  missing source hard-fails the whole set instead.
- **Scoping filters under `[forget]` are accepted and ignored**; they work under `[snapshot-filter]`.
  `group-by` defaults to `host,label,paths`.
- **For `--host` and `--filter-host`, the CLI overrides the config file** — contradicting the
  "env > config > CLI" summary in `PLAN.md` Part 2.
- **`snapshots --json` is `[{group_key, snapshots}]`; `forget --json` is `[{group_key, items}]`.**
  One key apart; the wrong parser reads zero and reports a clean empty repository. `label` is
  omitted, not empty, when unset.
- **A dry-run `forget` leaves the repository byte-identical. A `forget` with no keep rule is
  refused, and `keep-delete` alone does not count** — it is prune's grace period.
- **Prune is two-phase**: it marks packs and deletes them after `--keep-delete` (23 h). Proven end
  to end on the live repository: 600 packs and 2.3 GiB gone, live packs within 2.5% of a prediction
  written down beforehand. **A repeating prune never has an empty marked set** — each run deletes
  one generation and marks the next — so predict the *live* pack count, never the total (`0.2.30`).
- **On Windows a drive letter parses as a backend name**: `C:/…/repo` fails with ``The backend
  type `C` is not supported``; write `local:C:/…`. A bare `-P <name>` resolves through rustic's own
  `%APPDATA%` search there, not XDG — take the resolved path from `plan --format lines`.

### 5.4 Schedulers and platforms

**systemd**

- **Under `linger` the user manager starts at boot with `PATH=/usr/local/bin:/usr/bin`**, so a
  bare `rustic` resolves only after a graphical login imports the session environment: the working
  runs are the accident. Every unit names rustic by absolute path, and `schedule` refuses rather
  than writing a unit that cannot run (`0.1.10`).
- **A `Persistent=true` catch-up fires within milliseconds of the timer starting, and
  `RandomizedDelaySec` does not delay it** — measured at 300 s and 3600 s. On a laptop that means
  the missed hour is spent in the same second as `PM: suspend exit`, ~11 s before the network is
  usable, three times out of three (`0.2.16`). Arming a timer with no stamp file triggers nothing.
- **There is no user-level `network-online.target`** (`LoadState=not-found`), so `After=`/`Wants=`
  on it are inert in a user unit and `Wants=` fails silently (`0.2.16`).
- **`INVOCATION_ID` and `JOURNAL_STREAM` are set in an ordinary desktop terminal**, so neither can
  tell a scheduled run from a typed one; the gate has to be an explicit flag (`0.2.16`). **A
  variable whose name describes what you want and whose value answers a different question** is the
  family: `hostname(1)`, `fpath`, `%COMPUTERNAME%`, these two.
- **A service with an `[Install]` section can be enabled on its own** and run at every login; ours
  has none, so it is `static` (`0.1.7`).
- `journalctl -u <unit>` does not show a child's inherited stderr; `$(date)` in a unit is never
  expanded; `systemctl list-units` cannot see a disabled unit — use `list-unit-files` (`0.2.13`).
- **Whether an upgrade needs a re-schedule is a diff, not a guess:**
  `schedule -n <job> --write-only --unit-dir "$(mktemp -d)"`, then compare with the installed unit.

**launchd**

- **An agent gets `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and `PWD=/`**, so the absolute-path rule
  carries over; `HOME`, `USER`, `TMPDIR` and `SSH_AUTH_SOCK` do arrive (`0.1.26`).
- **There is no `linger`**: a user agent runs only inside a login session, so a Mac at the login
  window takes no backups and fails nothing. **launchd reports no next fire time.**
- **`enable` before `bootstrap`** (a persistent `disable` survives bootout), **`bootout` before
  `bootstrap`**, and the `gui/<uid>` domain from `getuid()`, not `id -u` (`0.1.27`).
- **A low-resolution clock reduced modulo a small number is not arbitrary**: macOS nanoseconds are
  multiples of 1000, so a spread seed from them put every host on minute 0 (`0.1.27`).
- A plist is XML: escape every interpolated value, and prove the file with `plutil -lint`, not a
  substring. `RunAtLoad`, `KeepAlive` and `StandardOutPath` are absent on purpose.

**Task Scheduler and Windows**

- **A repeating trigger with a past boundary runs the moment it is registered**, so `schedule`
  once took a backup and ran `forget` as a side effect; hourly is 24 plain triggers (`0.2.0`).
- **Read a setting's behaviour off a registered task, never off its name.** `<Hidden>` hides the
  task in the UI, not its window; `StartWhenAvailable` was not the run-on-registration cause;
  `RestartOnFailure` retries a *launch* failure, never an action's exit code (`0.2.10`).
- **A user task runs only via `InteractiveToken`, in the desktop session, and gets a console**;
  `S4U` needs `SeBatchLogonRight`. The fix is `FreeConsole` plus `CREATE_NO_WINDOW` on children —
  and `FreeConsole` closes this process's standard handles, so leaving `Stdio::inherit()` fails the
  spawn with `os error 50`, which names neither (`0.2.6`).
- **Defaults that stop backups silently:** priority 7 (below normal), no runs on battery, a
  three-day time limit. The definition must be UTF-16LE with a BOM. `<Arguments>` is one string,
  so the MSVCRT quoting rules apply; a job object with `KILL_ON_JOB_CLOSE` stops an ended task
  orphaning rustic (`0.2.0`).
- **`%COMPUTERNAME%` is the upper-cased NetBIOS name**, which would silently fall out of
  `enabled-on-hosts` and split the retention groups; `GetComputerNameExW` is the hostname (`0.2.0`).
  **`PATH` holds `rustic.exe`**: honour `PATHEXT`, extensions before the bare name (`0.2.5`).
- **`schtasks` has no locale-free next-run field** in LIST, CSV or XML, and `8/12/2026` parses
  successfully as two different dates. `Get-ScheduledTaskInfo` gives a real `DateTime`; render it
  with `.ToString('yyyy-MM-ddTHH:mm:sszzz')` — `'o'` carries fractional seconds and fails the parse.
  **`RandomDelay` is re-rolled on every query** (`0.2.22`).
- **`schtasks /Delete /TN <folder>` deletes nothing and says nothing**; delete each task with
  `Unregister-ScheduledTask` (trailing backslash on `-TaskPath`), then the COM `DeleteFolder`
  (`0.2.10`). A `conhost.exe` count cannot tell you whether a window was visible.
- **Config traps that look like validator bugs:** `\U` in a TOML string or a double-quoted YAML
  scalar is an escape; `/var/log/x.log` is *relative* on Windows; `HOME` is normally unset (`0.2.0`).
- **Windows nushell reads only `%APPDATA%\nushell\autoload`** (`0.2.14`); the console codepage
  mangles non-ASCII in printed output (`0.2.23`); an ssh session into Windows lands in `cmd.exe`,
  so multi-line PowerShell needs `-EncodedCommand`.

**Everywhere**

- **Rust ignores `SIGPIPE`**, so `status | head` panicked until the default was restored (`0.0.19`).
- **XDG on every platform**: a relative `XDG_*` value is ignored, an empty one is unset (`0.1.25`).
- **`jiff`:** `TimeZone::try_system()`, never `system()`, which falls back to UTC silently
  (`0.2.20`); `"…T14:30:00".parse::<civil::Date>()` succeeds and drops the time; `Timestamp` renders
  `…Z`, which the rustic format rejects (`0.2.27`).

### 5.5 Release, packaging and CI

- **Tag only after the post-merge `full-test` is green on that exact commit.** `full-test` is the
  only job in the release-shaped containers and is skipped on PRs; `v0.1.20`'s tag was the first
  thing ever to run them and had to be deleted (`0.1.21`).
- **A squash merge is a new commit**: `git rev-parse <main>^{tree}` equal to the CI-tested branch
  tip's tree is what makes the PR's green CI a statement about `main`. `merge-pr`'s trailing
  `Already on 'main'` is cosmetic.
- **Release channels must move together.** A GitHub-only release left crates.io a version behind,
  and the documented Mac route installs from crates.io. `cargo publish` refuses an *untracked* file,
  such as a Syncthing conflict copy (`0.2.3`); `cargo login` is interactive, so a first publish from
  any new machine needs a human.
- **The AUR, in full:** `pkgver` tracks the released tag, not `main`, so it legitimately trails
  `Cargo.toml`; `.SRCINFO` needs podman, hence a Linux host. **After a push the RPC answers about the
  state before it** — five observations — and `resultcount: 0` is not absence; the authority is
  the git repository the AUR serves, and a fresh clone diffing byte-identical is the strong form
  (`0.2.15`–`0.2.31`). The maintenance probe must be **authenticated** (`ssh aur@aur.archlinux.org
  help`); a keyless probe and `Host key verification failed` say nothing either way (`0.2.8`). The
  container must have rustic or the integration tests skip; `namcap` cannot see a *spawned*
  dependency; `makepkg`'s `-debug` package breaks a `*.pkg.tar.zst` glob (`0.1.2`). Here an AUR bump
  needs a PR, because `packaging/aur` is tracked and gated; `aur-bump` takes the bare version and
  `aur-publish` needs `AUR_CONFIRM=y` without a terminal.
- **A redirect truncates its target before the command is looked up**: `podman … > .SRCINFO` on a
  podman-less host emptied the tracked file. Generate into a temp file **in the destination
  directory**, validate the *content*, then `mv` (`0.2.12`; `~/AGENTS.md` §12 for why `/tmp` is the
  wrong place on a Syncthing tree).
- **Rootless podman egress follows route priority**: a stale `eth0` default route with no egress
  broke every container while the host itself worked. It is the host's fix, not the recipe's
  (`0.2.28`).
- **CI shape and its traps:** PRs run `build`; merges, tags and the Monday schedule run `full-test`
  (`ref_type == 'branch'` stops a tag matching twice); only drop a leg that is the intersection of
  two things each already covered (`0.1.30`). Fetch installers to a file and assert a post-condition
  by absolute path — `$GITHUB_PATH` affects only later steps (`0.2.19`). Matrix `fail-fast` reports
  one environmental failure as several reds; a runner not acquired fails at exactly 15m02s; the bare
  Fedora container has no git, so checkout falls back to an HTTPS archive download. **Read the job
  list, never an exit code**: `gh pr checks --watch` exits 1 on an API 502 too, and `gh run list
  --commit` needs the full SHA or returns nothing.
- **A red or stuck run needs a human here**: neither token can re-run a workflow. A job stuck
  `in_progress` with every step ✓ is unstuck by pushing the next *real* commit, never a manufactured
  one. A PR based on a non-`main` branch gets no CI, and a stacked PR is **closed, unrecoverably,**
  when its base branch is deleted on merge.
- **Squash-merge bodies concatenate every branch commit** (`squash_merge_commit_message =
  COMMIT_MESSAGES`), so a trailer on each commit is a duplicate on `main` — trailer on the last
  commit only (`0.2.41`; `AGENTS.md` Part 1 §1).

### 5.6 Repository, tooling and the fleet

- **`~/git` and `~/Sync/git` are one directory, `.git` included, synced by Syncthing.** Never run
  git in this repo from two machines at once: Syncthing has written conflict copies into `.git`
  mid-merge, and a `refs/heads/main.sync-conflict-*` copy was read as a branch and deleted on
  `origin` by `merge-pr`'s prune. After any merge or odd ref, `find . .git -name '*sync-conflict*'`
  — both paths. Read the peer from the device short-id in the suffix; never infer it. A peer
  rejoining conflicts first on the generated `docs/rusticprofile.1`. `git fsck`'s real errors need
  `dangling` and `bad sha1 file` filtered out.
- **`WIP.md` has no arbiter.** It is gitignored, so git cannot say which conflict copy is right —
  and a handoff note has already been lost that way. Check for `WIP.sync-conflict-*` before trusting
  it. Anything durable written only there dies with the working copy (`0.2.13`).
- **Install fleet binaries from a tag, never from the synced tree.** `cargo install --path .` here
  builds from a directory other machines write into. `just install-tag vX.Y.Z` installs the binary
  from the tag, completions from that binary and the man page from `git show` of the tag
  (`0.2.23`); `cargo install` alone leaves the man page and completions stale — seventeen releases,
  on two hosts. `~/.cargo/config.toml` sets `target-cpu=native`, so a binary copied between hosts
  can `SIGILL` (`0.2.13`).
- **`install_completions.py` takes a command NAME, never a path.** `Path(dir) / "/abs"` discards
  the directory, so an absolute path once overwrote a live binary with a completion script; the
  helper now refuses it (`0.2.32`). Around any fleet step, assert a post-condition (binary > 1 MB,
  reports the expected version) before and after.
- **Justfile traps:** `just` takes the last contiguous comment block as a recipe's doc comment, so
  prose needs a blank line above the description (`0.1.2`); `@` is only meaningful in a plain
  recipe, and `gate_conformance.py` refuses it inside a shebang body (`0.2.40`); `set windows-shell
  := ["bash", …]` broke every backtick variable where bash is absent (`0.2.1`); `${APPDATA:-}` under
  just's `sh -u` (`0.2.14`); a doubled backslash may lose a layer, and a bracket expression
  `[\]fB` has none to lose (`0.2.9`); on Windows, shebang recipes need Git's `usr\bin` for `cygpath`.
  **Weigh late-session edits to the Justfile or CI**: `0.2.1` and `0.2.2` were both confident
  regressions.
- **Vendored helpers are checked by running their self-tests, not by diffing text** — a text diff
  also passes on a repo that never adopted the standard (`0.2.23`). "The repo with the fixes" and
  "the repo with the right mechanism" were different repos, and only reading the sibling's history
  first showed it; the majority is not the arbiter either (`0.2.39`). **A documented trap is not a
  guard** (`0.2.40`).
- **Identifier sweeps** use word boundaries (`git grep -nwE 'name1|name2' -- ':!WIP.md'`; one fleet
  name is a substring of "mechanism"), run *after* writing the prose, and cover the commit message
  and PR body. A release note describing a redaction is the likeliest place to retype the value, and
  a test must not hard-code a developer's home path (`0.1.29`, `0.2.7`). The real names to sweep for
  are in `~/AGENTS.md`'s fleet table, not here.
- **ssh:** `ssh` inside a `while read` loop eats the list — use `ssh -n`, but never `-n` with
  `< script`, which then runs an empty script; ssh config is first-match-wins, so a host block must
  sit above `Host *`. **Ask Syncthing (`/rest/system/connections`) before declaring a host offline**:
  netbird's `No route to host` was wrong about a host on the LAN.
- **`--as-host` simulates another host's name against THIS machine's files**, so from a stale
  chezmoi checkout it reports stale gating attributed to another host — a false alarm, and then a
  chezmoi break from "fixing" it. `chezmoi update` before trusting any cross-host inspection, and
  read a rejected push before reconciling it (the chezmoi skill has the recipe).

### 5.7 Documentation and duplicated state

- **Duplicated state goes stale one copy at a time, and the copy nobody re-reads is the one that
  survives.** `AGENTS.md` said "pre-code" for thirty-five releases after `PLAN.md`'s identical line
  was fixed (`0.2.4`); the `--background` help text contradicted a correct man page (`0.2.17`). **If a
  fact has an authority, point at the authority**, and write down only what nothing else can answer
  (`0.2.21`).
- **Adding an option costs every comparative claim already written about the set** — "the only
  route that…", "none of them…" — and none of those claims is near the diff (`0.2.26`). Changing a
  gate falsified present-tense claims in `README.md` and `PLAN.md` (`0.2.30`). They surface when
  `just pr`'s checklist is answered after checking, never before (`0.2.5`).
- **A false claim in a safety section is worse than an absent one** (`0.2.17`), and a backup tool's
  README is a safety surface: an aspirational feature list is a way to lose data (`0.1.0`).
- **`PLAN.md` corrects in place and keeps the superseded text**; its section numbers are permanent
  anchors. This file does the opposite: it is rewritten as the project moves, and history is
  `git log`.
