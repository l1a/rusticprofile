# NOTES.md

Living state for **rusticprofile**: architecture, development guidelines, current state,
backlog and release log. This project has no `CHANGELOG.md` — this file is it.

`PLAN.md` is the design record: how the design was reached, every rejected alternative
with its reason, and the measurements behind each decision. It does not get rewritten as
the project moves; this file does.

---

## 1. Project Overview

- **Name**: rusticprofile
- **Goal**: a local, per-machine scheduler and orchestrator for `rustic` backups — no central server
- **Key technologies**: clap, serde, rustic (as the delegated backend), systemd / launchd
- **License**: GPL-3.0-or-later
- **Repository**: https://github.com/l1a/rusticprofile

**The delegation boundary is the single most important thing to understand.** rustic owns
all backup configuration: repository, sources, excludes, retention policy, hooks,
environment, metrics. rusticprofile owns scheduling, per-host gating, operation
sequencing, exit classification and lock coordination. It builds no backup flags — a job
resolves to `rustic -P <profile> <operation> [--name <set>]...` and nothing more.

There is exactly one exception, and it is read-only: rusticprofile parses `rustic.toml` to
enumerate `[[backup.snapshots]]` names, so it can reject a `--name` that does not exist.
That exception is bought and paid for in `PLAN.md` §7.2 — rustic silently ignores an
unknown `--name` when any valid name is also present, so nobody else can catch it.

---

## 2. Codebase Architecture

```
src/
  lib.rs          documented module index — the map of where things live
  main.rs         thin bin: parse -> dispatch -> report -> exit
  cli.rs          clap derive; only what is implemented is declared
  config/         M1  parse, host-gate, interpolate, validate jobs.yaml; annotated examples
  rustic/         M1  build the rustic argv; classify its exit
  exec/           M1  spawn, forward signals, mask secrets in logs
  run/            M1  operation ordering; LockBudget seam
  report.rs       M1  owo-colors output
  schedule/       M2  systemd units; M3 launchd plists; 0.2.0 Task Scheduler tasks

tests/cli_tests.rs   one integration test file, driving the real binary
docs/rusticprofile.1.md   man page source (mandown); .1 is generated, never edited
scripts/hooks/       real git hooks — the enforcement layer
```

Single crate, lib plus thin bin, **no workspace**. `retch` is a workspace only because it
has a separately publishable library; there is no second consumer here, and a workspace
costs real complexity in the publish recipes.

---

## 3. Specific Development Guidelines

- **Man page**: do not edit `docs/rusticprofile.1` directly. It is generated from
  `docs/rusticprofile.1.md` by `just man` (mandown), with the version read out of
  `Cargo.toml`. Run `just man` after any version bump and commit the result — `just pr`
  fails if it is dirty.
- **Quality gate**: `just check` = `cargo fmt --check` + `cargo clippy -- -D warnings`.
  This is what CI runs and what `pre-push` runs.
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
  error, not a silent surprise. The CLI is expected to move before 1.0; say so in the release
  entry rather than in the version number.

  **`0.2.0` is the precedent for the other side of that rule**, and it is worth recording
  because "a whole new platform" is *not* by itself the reason. Windows support added a variant
  to the public `schedule::Backend` enum, which breaks any exhaustive match downstream — that
  is the library API breaking, the documented trigger. It also reversed a declared v1 non-goal,
  which is milestone-shaped in a way `0.1.26`/`0.1.27` were not: those added a *backend* for a
  platform already in scope, and were correctly patch bumps. If a future change adds a platform
  without touching a public type, it is a patch.

  Tag `v$VERSION` from a clean `main`; never `cargo publish --allow-dirty`.
- **Deliberate absences.** No `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `rust-toolchain.toml`, MSRV declaration, `[lints]` table, `#![deny(...)]` or
  `CHANGELOG.md`. Their absence is the convention — do not add them.
- **Backup safety**: read-only operations against a production repository are fine; every
  write test goes to a throwaway repository under a temp dir, deleted afterwards; never
  **`restic` prune** against a shared repository any rustic client writes to (`PLAN.md`
  §7.6 — `rustic prune` is safe by design and is the only prune that may run); never delete
  snapshots without explicit
  per-step authorisation. See `AGENTS.md` Part 2 §3.
- **No live infrastructure identifiers in tracked files.** The repository is public, so this
  is now permanent rather than a pre-publication chore: no real hostnames, bucket names,
  project ids or home paths. Hosts are `host-a`…`host-h`; paths are `/home/user`. Grep the
  diff before opening a PR, and beware substring false positives — a redacted hostname can
  hide inside an ordinary English word.

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
them.

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
`--name` per enabled snapshot set. **Those are the only flags rusticprofile ever emits**, and
a test in `rustic/invoke.rs` asserts it against every built argv. That test carries the
instruction: *if it needs changing, the delegation boundary is moving and that belongs in
`PLAN.md` first.*

**A passthrough is acceptable only where it is read-only and adds no flags.** `snapshots`
qualifies and exists; `check` would qualify. `forget` and `prune` do not — destructive, and
their scoping belongs in the rustic profile where a flag typed at a prompt cannot contradict
it. `restore` never does.

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
| `filter-hosts` matches the **fqdn** exactly; a short name matches zero snapshots and retention silently never runs | §5.9 |
| exclude the password file and cloud credentials, or the key goes inside the lock | §4.1 |

### 6. Corollaries worth stating once

- **`jobs.yaml` is byte-identical on every host; `rustic.toml` must be generated.** rustic
  expands neither `~` nor `$VAR` and has no env-var route to the host filter. A consequence
  with teeth: a shared `jobs.yaml` is only ever as new as the **oldest binary** reading it —
  and, added 2026-08-04, only as current as the **most stale chezmoi checkout** reading it.
- **rusticprofile applies XDG rules on every Unix, including macOS.** Not a preference; the
  requirement that one line of one file mean one thing across the fleet.
- **A `doctor` command would catch invariants 2 and 3 from inside the repository** — a host
  whose snapshots mix labelled and unlabelled entries has a second retention authority; a
  rustic-written repository with a live restic exclusive schedule has a second lock protocol.
  Not built. Backlog.

---

## Current State (v0.2.7)

**`v0.2.2` is released on GitHub *and* on crates.io** as of 2026-08-07 — the registry had been
stuck on `0.1.34` since 2026-08-04, which mattered more than usual because `0.2.2` is the first
crate that builds on Windows: until it landed, `cargo install rusticprofile` handed a Windows user
a version that could not compile. Verified three ways rather than from the publish command's own
report: the registry API (`max_version 0.2.2`, 179,984 bytes, not yanked), `cargo install --locked`
into a throwaway root **on Windows**, and running that binary.

**Windows is a fully supported platform as of `0.2.0`**, including scheduling: `schedule`,
`unschedule` and `status` drive **Task Scheduler**, the third backend. 344 tests green there (294
unit + 50 integration, including the three rustic-backed ones against real rustic 0.11.3), and the
whole loop is verified end to end on a real machine against a throwaway repository.

Windows was a declared v1 non-goal until 2026-08-06; `PLAN.md` §7.9 records the reversal and §5.10
the measurements. **`M4` (repository lock coordination) is still the only unbuilt milestone.**


**Milestone 1 is COMPLETE** — all seven steps, v0.0.1 through v0.0.7.

**Step 7 — the CLI surface. Complete.** `--rustic-binary` overrides the configured
executable, which is what makes rung 2 of the verification ladder possible: point it at a
recording shim that logs its argv and exits 0, and a whole job runs end to end without
rustic ever executing. The exit-code surface promised by `PLAN.md` is now asserted by
tests: `0` success or partial, `1` run failure, `2` config error, `130` interrupted.


The override is applied *after* loading, so it cannot mask a mistake in the configuration
it overrides. `config` does not take it — that command never invokes rustic.

**Step 6 — the runner. Complete.** (Steps 1–5: v0.0.1–v0.0.5.)
**rusticprofile can now run a backup**, which is the first version of which that is true.

175 passing tests (145 unit, 30 integration), clippy clean under `-D warnings`.

`run -n <job> [--dry-run]` takes a local lock, runs the job's operations in order, and
**stops on failure but continues on partial** — so a backup that partly worked still reaches
retention. That single rule is the structural fix for the bug that started the project.
Verified end to end against a throwaway local repository: a job whose second snapshot set
has a missing source reports `partial`, runs `forget` anyway, and exits 0.

Also in this version, the **`forget` scoping invariant**, whose config key names were
verified against rustic 0.11.3 rather than guessed — and the guess would have been wrong.
Filters live in `[snapshot-filter]`; under `[forget]` rustic accepts and ignores them, and
`[forget]` does not reject unknown keys either, so a config can look scoped and filter
nothing. All three checks (a scoping filter present, no misplaced filters, `group-by`
explicit) are refusals at load time.

**This is the step that matters most.** rustic exits `1` for everything that is not a clean
success — and a *partial* backup exits 1 too. Treating that as total failure aborts the job
before retention, which is how the fleet reached 2810 snapshots under a policy that should
have capped it near 49 per host. `rustic/exit.rs` classifies on the count of `--json`
snapshot objects on stdout, never on the exit code and never on matching log text.

Measured against rustic 0.11.3 (table in `PLAN.md` M1 step 5 and in the module docs): one
good `--name` → exit 0, 1 object; one good and one broken → **exit 1, 1 object**; only
broken → exit 1, 0 objects; wrong password → exit 1, 0 objects.

`Verdict::Partial` lets the job continue so `forget` still runs. Because of that,
**the safe direction when uncertain is failure, not partial** — claiming partial without
evidence would run retention after a backup that saved nothing, losing snapshots rather
than merely accumulating them. Partial is only claimed on at least one successfully parsed
object; unparseable, absent and uncaptured output all count as zero.

Three integration tests exercise this against **real rustic** in a throwaway local
repository — partial, clean and total-failure. They skip with a printed notice when rustic
is absent, so **they do not run in CI**; the local run is what covers them.

`exec/` can now spawn a child, forward signals to it and mask secrets in anything printed —
but **nothing calls it with a rustic argv yet.** Wiring planning to execution is the runner,
step 6. The binary still cannot run a backup.

Three decisions in `exec/` worth knowing before changing it:

- **stdout is captured, stderr is not.** rustic writes progress and diagnostics to stderr
  and `--json` snapshot objects to stdout (measured, `PLAN.md` §5.8), so this gives both at
  once: the operator watches progress live while step 5 still gets the machine-readable
  output it needs to tell a partial backup from a failed one.
- **An interrupt is forwarded to the child and then waited on.** Exiting immediately and
  orphaning a running rustic would leave a lock held on a repository shared by seven
  machines.
- **The environment is inherited unmodified** — nothing is set, unset or rewritten.
  `exec/env.rs` only *selects* the subset worth showing a human.

Redaction is a **backstop, not the primary control**: the primary control is that secrets
never enter this process (`PLAN.md` §4.1). It masks by name — `RUSTIC_PASSWORD` is hidden,
`RUSTIC_PASSWORD_FILE` is shown because a path is not a secret and hiding it removes the
detail most worth seeing. `_COMMAND` variants *are* masked, since a `password-command` can
embed the secret inline and nothing can tell that apart from a keyring lookup.
`GOOGLE_APPLICATION_CREDENTIALS` is explicitly allowlisted as path-valued — it is exactly
the variable people misconfigure. The marker is fixed rather than length-matched asterisks,
which would narrow a search.

Three commands work: `config --check`, `config --show -n <job>` and `plan -n <job>` (with
`--show-env` and `--show-secrets`), all with `--as-host` and `--config`. All are hermetic — **no rustic binary, no repository, no
network** — and integration tests prove it by running them with `PATH=/nonexistent`. There
is still nothing that can execute a backup.

`plan` prints the exact argv a job would run without running it: `--format lines` gives one
argv element per line (the golden format, and diffable), `--format human` a readable
summary. `rustic/invoke.rs` builds it as a **pure function of (config, job)** — no clock, no
environment, no hostname lookup — since everything host-dependent was already decided during
config loading.

The argv is the whole contract: `rustic -P <profile> <operation>` plus one `--name` per
snapshot set enabled here, plus `--json` on `backup`. Those are the *only* flags
rusticprofile ever emits, and a test asserts exactly that against every built argv.
`--json` is an output-format flag rather than a backup setting: without it, classification
would be reduced to matching English text in a log.

The load pipeline, in `src/config/mod.rs`, runs in a fixed order: **read → parse YAML → host
gating → `${…}` interpolation → batched validation.** Parsing before substitution is what
makes two of the predecessor's failure modes structurally impossible — comments are gone
before anything is substituted, and no substitution can produce a document that then fails to
parse.

Modules:

| Module | Does |
|---|---|
| `config/job.rs` | the `jobs.yaml` schema; every struct is `deny_unknown_fields` |
| `config/hosts.rs` | hostname resolution, short form, host matching |
| `config/interp.rs` | the closed `${…}` variable set |
| `config/schedule.rs` | schedule vocabulary — parsed and validated now, acted on in M2 |
| `config/rustic_toml.rs` | read-only: snapshot-set names out of `rustic.toml` |
| `config/example.rs` | the annotated starting-point configs behind `config --example` |
| `config/validate.rs` | batched rules, `Violation` / `ValidationErrors` |
| `config/paths.rs` | XDG locations for both config trees |
| `rustic/invoke.rs` | build the argv; the secret-flag and flag-inventory assertions |
| `exec/mod.rs` | spawn, stdio modes, signal forwarding |
| `exec/redact.rs` | masking secrets in printed env and argv |
| `exec/env.rs` | which variables affect a rustic run |
| `exec/outcome.rs` | what came back — descriptive, not a verdict |
| `rustic/exit.rs` | the verdict: partial vs failed, from the JSON object count |
| `run/steps.rs` | operations in order; stop on failure, continue on partial |
| `run/lock.rs` | local per-job flock; the M4 repository-lock seam |
| `report.rs` | the run summary, including what was skipped |

`config/rustic_toml.rs` is not in `PLAN.md`'s original module list — it exists because of the
Part 7 decision, which came after that list was written.

**What validation refuses**, each closing a way a config could quietly do less than it says:
unknown keys anywhere; an unknown or duplicate snapshot-set name; a snapshot set that does not
exist in the rustic profile (checked as *declared*, so a typo behind another host's gate is
still caught); snapshot sets on a job that does not back up; every set gated away on a host;
`enabled-on-hosts: []`; a relative log path; an unknown `${…}` variable; `${job}`/`${profile}`
inside `defaults`; an unset `${env:…}`; a malformed `${date:…}` format even when its resolution
is deferred; a duplicate operation; a job or profile name that is not filename-safe; and an
empty job list.

`${date:…}` is **deferred, not resolved, at load time** — it is re-emitted verbatim unless a
clock is supplied. This is what will keep M2's generated unit files correct: baking today's
date into a unit would freeze it at install time. The format string is still validated at load
time, so a malformed one fails now rather than at 03:00 during the backup.

In place from v0.0.1:

- `Cargo.toml` (edition 2024, `exclude` list, `[profile.profiling]`), `Justfile`, `LICENSE`
- man page source and generated page; six-shell completions via `just install-completions`
- CI: `rust.yml` (build/test matrix over Linux x64+arm, Fedora x64+arm, macOS; tag-triggered
  release) and `security.yml` (weekly `cargo audit`), SHA-pinned actions, dependabot
- `scripts/hooks/pre-push` + `just install-hooks`
- community health set: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, PR template,
  two issue templates

**Three deliberate deviations from the `retch` template**, so they are not mistaken for
oversights:

1. **No `post-merge` hook.** retch's uploads benchmark results to a gh-pages dashboard.
   There is no dashboard and no benchmark here, so the hook would have nothing to do.
2. **No `benches/`, no criterion, no `[[bench]]`.** Nothing exists yet that is worth
   measuring. `just bench` says so rather than pretending. Both arrive with the first real
   candidate — config parsing is the likely one.
3. **No `.cargo/audit.toml`.** retch's exists to carry justified ignores for advisories it
   actually has. An empty ignore list would be noise; the file arrives with the first
   advisory that needs justifying.

Also deferred on purpose: **no Windows in the CI matrix**, since Windows is a declared
non-goal for v1 and the scheduling backends are systemd and launchd.

**Dependencies are added by the milestone that needs them**, not up front. Present:
`clap` + derive, `clap_complete`, `clap_complete_nushell`, `anyhow`, `owo-colors`, `serde` +
derive, `serde_yaml_ng`, `toml`, `dirs`, `jiff`, `nix` (`hostname` feature only so far), and
`tempfile` as a dev-dependency. Still to come: `serde_json` (streaming parse of
`rustic --json` output, M1 step 5), the rest of `nix` (flock, SIGINT forwarding), `semver`
(rustic version checks) and `criterion`.

---

## 4. Backlog

Milestone 1, remaining steps (`PLAN.md` §4):

- [x] **Step 2 — config.** Done in v0.0.2.
- [x] **Step 3 — invocation planning.** Done in v0.0.3, with `tests/golden/`, `just golden`
      and a `just check` staleness gate.
- [x] **Step 4 — exec + redaction.** Done in v0.0.4.
- [x] **Step 5 — exit classification.** Done in v0.0.5.
- [x] **Step 6 — runner.** Done in v0.0.6.
- [x] **Step 7 — CLI.** Done in v0.0.7. **Milestone 1 is complete.**

Smaller items:

- [x] **The `forget` scoping invariant — implemented in v0.0.6.** The key names were verified
      against rustic 0.11.3 rather than guessed, and the guess would have been wrong: the
      filters live in `[snapshot-filter]`, not `[forget]`, where rustic accepts and then
      ignores them. `[forget]` does not reject unknown keys either, so a config can look
      scoped and filter nothing. All three checks — a scoping filter present, no misplaced
      filters, `group-by` explicit — are refusals at load time.
- [ ] First benchmark + `benches/`, `criterion`, `[[bench]]` (see deviation 2 above)
- [x] **Publishing decision — taken, and executed as recommended.** The recommendation was
      *publish at M2, when `schedule`/`unschedule`/`status` make the README's first paragraph
      true.* M2 landed in `0.0.19` and `v0.1.0` was tagged and published then, so it was
      followed rather than overtaken. `rusticprofile` was claimed; `rustic-profile` is still
      free and nothing depends on it. crates.io carries **`0.1.31`** as of 2026-08-04.
      **AUR remains outstanding** — the package is written and container-verified, blocked
      solely on their maintenance window (rechecked 2026-08-04, still down; check
      `ssh aur@aur.archlinux.org help`, **not** the web page, which returns 200 throughout).
- [x] **Decided in v0.0.5, as the design intended.** A partial backup is `Verdict::Partial`:
      loud in the report, the job continues, `forget` still runs, exit **0**. Partial is only
      claimed on at least one successfully parsed `--json` snapshot object — unparseable,
      absent and uncaptured output all count as zero — because the opposite error would run
      retention after a backup that saved nothing.
- [x] **Redact infrastructure identifiers and make the repository public.** Done in v0.0.9.
- [x] **Why one snapshot set backs up 0 B — answered 2026-08-01, and it is not a defect.**
      The source directory is empty: 0 files on disk. Both rusticprofile and the predecessor
      report 0 B for it because 0 B is correct. Worth keeping as a set — it will contain data
      on other hosts — but it is a reminder that an empty snapshot still competes for a
      retention slot, which is how it came to displace a 395 MiB one (`PLAN.md` §7.5).
- [ ] **A `doctor` command.** Two checks now, one per authority a shared repository has.
      `PLAN.md` §7.5: warn when one host's snapshots carry a mix of labelled and unlabelled
      entries, which is what a second *retention* authority looks like from inside the
      repository. `PLAN.md` §7.6: warn when the repository is written by rustic while any
      restic schedule still runs an exclusive operation, which is a second *lock* authority
      and the more dangerous of the two. Not scheduled; recorded so the idea is not lost.
- [x] **"M4 blocks space reclamation" was WRONG, and is superseded.** Corrected in `0.1.3`
      and acted on in `0.1.11`: rustic is lock-free by design, so `rustic prune` is safe and
      is the only prune that may run here. Prune returned to the designated host on
      2026-08-03 as a `rustic prune`; it first fires **Mon 2026-08-10**. M4 is defence in
      depth, not permission — invariant 3 in §3a.
- [ ] Add a Pre-PR Checklist section to `AGENTS.md` Part 2, mirroring retch's §4
- [x] **A Task Scheduler backend — done in `0.2.0`**, verified end to end on a real machine.
      Found and fixed a §7.5 violation no unit test could have caught: a repeating trigger with a
      past boundary runs on registration, so `schedule` took a backup and ran `forget` as a side
      effect. Hourly is 24 plain triggers instead.
- [x] **A job object with `KILL_ON_JOB_CLOSE` — done in `0.2.0`**, verified by
      `TerminateProcess`-ing the parent and confirming the child tree died with it.
- [x] **`set windows-shell` — done in `0.2.0`.** The shebang recipes need Git's `usr\bin` on
      PATH, which the Justfile now documents at the top.
- [x] **The `${env:HOME}` question — decided and done in `0.2.0`**, the way `PLAN.md` §7.9
      recommended: the key is commented out in the example, since its default is already that
      path on all three platforms.
- [ ] **Windows, remaining work after `0.2.0`.** Neither is a gap in the platform's function:
      1. **Assert byte-for-byte argv delivery in `tests/cli_tests.rs`**, where
         `CARGO_BIN_EXE_rusticprofile` gives a cooperating child. The unit test is Unix-only
         because Windows has no argv; `PLAN.md` §5.10 explains why the guarantee weakens. What
         remains is only the *automated* form: `quote_argument` is unit-tested against the MSVCRT
         rules, and the round trip through a real process was verified by hand in `0.2.0` — a
         config path containing a space registered, ran under Task Scheduler, and was found. The
         gap is that nothing re-checks it.
      2. **`permission: system` has never been registered and run.** The SYSTEM principal is
         generated and unit-tested, and it is the answer to the login caveat — so it is the one
         part of the platform still resting on reasoning rather than measurement. It needs an
         elevated shell.
- [x] **Done 2026-08-07 — the fleet's own `jobs.yaml` no longer uses `${env:HOME}`.**
      `defaults.rustic-config-dir` was dropped from the chezmoi-managed file, taking the option
      `PLAN.md` §7.9 recommended: that key's default is already `$XDG_CONFIG_HOME/rustic` on all
      three platforms, so the line only restated the default in a form that is unset on Windows.
      **`HOME` was deliberately not set on the Windows host** — that would have fixed one machine
      and left the next to rediscover it. Removing a key is safe for a fleet-shared file; adding
      one is not, which is why §7.9's `${home}` variable stays unbuilt.
- [x] **Fixed in `0.2.5` — `schedule` could not find rustic on Windows.** `PATH` there holds
      `rustic.exe` and the resolver joined only the bare name, so it refused on every Windows
      host with a default config. `PATHEXT` is now honoured. The `0.2.0` end-to-end verification
      missed it because its throwaway config named rustic explicitly.
- [ ] **`just check` does not lint test code** — `cargo clippy` runs without `--all-targets`, so
      every `#[cfg(test)]` module has been unlinted since the scaffold. Adding it found one real
      lint (`0.2.0`). Not changed in that release because it may surface more across the
      codebase, and a gate change deserves its own PR.
- [x] **Decided and done in `0.1.32`.** The operating rules — `PLAN.md` §7.3, §7.5, §7.6,
      §7.7, §7.8 — are promoted to **§3a** above, which is now where they are maintained.
      `PLAN.md` keeps every section number, its full text and its measurements, and gains a
      pointer at the top of each; nothing was renumbered or deleted, because seventeen of its
      anchors are cited across `NOTES.md`, `AGENTS.md`, `WIP.md`, the shipped
      `config --example` and the source. `PLAN.md` is now explicitly the historical design
      record: Parts 1–3 (why the design is shaped this way) and Parts 5/7 (the measurements).

---

## 5. Release Log

Versioning rule in §3. **`v0.1.0` and `v0.1.9` are released** — tagged, on GitHub, and on
crates.io. Everything numbered `0.0.x` predates the first release and never left this
repository; the `0.1.x` entries between the two releases shipped together in `v0.1.9`.

*The first two entries were briefly numbered `0.1.0` and `0.2.0` before that policy was set,
and were renumbered in place. No tags existed, so nothing had to be unwound — if you find an
external reference to a rusticprofile `0.1.0` or `0.2.0` from July 2026, it predates the
renumbering and means the versions below.*

### v0.2.7 — a real hostname in two source comments

**Comments only; no behaviour changes.** Two comments describing the same measurement named a real
fleet machine instead of a placeholder, against §3's rule that this repository — which is public —
carries no live infrastructure identifiers in tracked files:

```
src/config/rustic_toml.rs:154   `pinned-name` on a machine called `<real-hostname>`, and a …
src/config/validate.rs:1073     called `<real-hostname>`, and `filter-hosts = ["pinned-name"]` …
```

**The quotation above is redacted, and writing this entry is how that lesson arrived a fourth
time.** The first draft pasted both lines verbatim — so the release note announcing the fix
carried the very identifier it was removing, into the same tracked, public file. Caught by
re-running the sweep *after* editing rather than only before. A release entry describing a
redaction is the one place the value is guaranteed to be typed out again, and quoting the
before-state is exactly where it hides.

Both lines now say `host-a`, which is already the source tree's ordinary placeholder — 52 uses against
21 of `host-a.local` and 13 of `host-b`. The measurements and the reasoning are untouched; only
the machine's name changes.

**A hostname is not a credential, and that is exactly why this kind of line survives.** It reads
as harmless in isolation, and each one individually is. What makes it worth a release entry is
that the same file already disagreed with itself: five lines below the `validate.rs` instance, the
test it introduces gets it right (*"the machine is `host-e.local`"*). So this was an inconsistency
rather than an unsettled question — the convention was known, applied a few lines away, and missed
here.

**It is also a repeat, which is the part worth recording.** `0.1.29` removed real identifiers from
illustrative command output in this same file; `0.2.5`'s work caught another in an example error
message and in a commit message, amended before pushing. Three instances now, all the same shape:
**a real value written while documenting a real measurement**, where it reads as decoration rather
than as data. The measurement is genuine and the name is incidental to it, which is precisely why
nobody notices.

The scan that finds them needs word boundaries, as §3 warns: at least one of the fleet's names is a
substring of a common English word, so an unbounded search returns a page of matches inside
ordinary prose — which trains the reader to skim, and skimming is how a real one survives:

```bash
git grep -nwE 'name1|name2|…' -- ':!WIP.md'
```

A full sweep for the other classes §3 names — bucket names, project ids, real home paths — found
**none**. The only other match is the AUR `PKGBUILD` maintainer line, which is authorship metadata
the format requires, in the same class as the SPDX copyright headers rather than an infrastructure
identifier.

### v0.2.6 — every scheduled run opened a terminal window

**Reported by Ken watching his own machine, not by a test:** each hourly backup popped a terminal
window that appeared and vanished. It had done so since `0.2.0` declared Windows supported.

#### Why it happens, and why it is not a setting

Task Scheduler can run a task as the logged-on user only through `<LogonType>InteractiveToken</LogonType>`,
which starts it **inside that desktop session** — and Windows gives a console-subsystem program a
console there. Measured on Windows 11 by enumerating top-level windows during a real scheduled run:

```
class=CASCADIA_HOSTING_WINDOW_CLASS  proc=WindowsTerminal  title='C:\Users\user\.cargo\bin\rusticprofile.exe'
```

`<Hidden>` is the setting that *sounds* responsible and is not: it hides the task in the Task
Scheduler UI, not its window.

**The logon types that avoid the interactive session all need rights an ordinary account does not
hold.** `S4U` and a stored password both require `SeBatchLogonRight`, granted by default to
Administrators and Backup Operators; `LocalSystem` needs elevation to register. Measured on a
standard account whose only groups are `Users` and `Authenticated Users`: registering an otherwise
identical task with `<LogonType>S4U</LogonType>` fails with `ERROR: Access is denied.` So this
cannot be fixed in the task definition, and a tool that only worked for administrators would be
the wrong answer.

#### The fix: `run --background`, emitted by `schedule`

`FreeConsole` at startup, plus `CREATE_NO_WINDOW` on every child. **Both halves are required and
neither works alone** — a child console program started from a process that has no console gets a
*new* one, so detaching by itself would move the window to rustic rather than remove it, later and
harder to attribute.

**`FreeConsole`, not `ShowWindow(GetConsoleWindow(), SW_HIDE)`.** Where the default terminal is
Windows Terminal the console is a pseudoconsole, and `GetConsoleWindow` returns the pseudoconsole's
own window rather than the visible terminal window — so hiding it hides the wrong window. Detaching
does not depend on which terminal is hosting.

**The flag is on the argv rather than in the definition** because it is the process that must
detach; the definition cannot express it. It is unconditional for every job, schedule and priority,
and a test asserts that across the matrix — one job generated without it is one job with an hourly
window.

#### The regression this introduced, caught by running it

The first version detached and left `Stdio::inherit()` in place. **`FreeConsole` closes this
process's standard handles**, so `CreateProcess` was handed invalid ones and refused the whole
spawn:

```
backup   failure  could not run `rustic.EXE`: The request is not supported. (os error 50)
forget   skipped  did not run, because an earlier operation stopped the job
```

That message names neither the console nor the handles and reads like a broken rustic install. A
detached run now uses `Stdio::null()` for stdin and stderr; `Stdout::Capture` is untouched, because
`backup` needs the `--json` objects regardless of who is watching (§5.8). Nothing is lost that was
being read: in a scheduled run stderr already went to a console nobody saw, and the run's record
goes to the `log:` file.

**It was caught only because the run was performed and its outcome checked** — the window had gone,
the tests were green, and the backup was failing. `status` reporting `failure` with `forget`
skipped is what surfaced it. *A check is only worth what its oracle is worth*, and "the window is
gone" is not an oracle for "the backup still works".

#### What this does not do

**A sub-50 ms flash remains.** The console is created by the OS before any of this crate's code
runs, so the window exists between process start and `FreeConsole`. Measured by sampling every
50 ms across a run: visible in **1 sample of ~900**, against every sample for the whole ~7 s run
before the change. Removing it entirely needs the task to launch something that is not a console
program — a GUI-subsystem launcher binary — which is a second artefact through build, release and
packaging, and is not obviously worth it for one frame.

`exec::run`'s signature is deliberately unchanged: the suppression state is a process-global in the
existing `INTERRUPTED` idiom, because adding a parameter to a public function would break an
exhaustive caller and force a minor bump under §3 for a platform detail with one call site.

### v0.2.5 — `schedule` could never find rustic on Windows

**`schedule` refused on every Windows host with a default configuration, and had done since
`0.2.0` declared the platform supported.** Found by running it on the real fleet config rather
than by a test.

```
error: could not find `rustic` on PATH, and a unit cannot search PATH for it: with `linger`
       the systemd user manager starts at boot with a minimal environment that will not
       include `~/.cargo/bin`.
```

Every clause of that message is wrong on Windows, and the diagnosis it invites — "rustic is not
installed" — was wrong too: `rustic.exe` was on `PATH`, at `C:\Users\user\.cargo\bin\rustic.exe`.

#### The defect

`resolve_rustic_binary` searched `PATH` with `dir.join(name)` and nothing else. **Windows `PATH`
holds `rustic.exe`; there is no file called `rustic`,** so the join tested a path that cannot
exist and the search always came back empty. `PATHEXT` — the mechanism that makes a bare name
resolvable at all on Windows — was never consulted.

Candidates are now built by `path_candidates`, a **pure function taking `PATH`, `PATHEXT` and a
`windows` flag as arguments**. That shape is the `0.1.25` precedent: `std::env::set_var` is
`unsafe` in edition 2024 and races every other test in the binary, and taking `windows` as a
parameter means both platforms' behaviour is testable on whichever one runs the suite — the same
reason `schedule` uses a runtime `cfg!` rather than `#[cfg]`.

**Extensions are tried before the bare name**, because an extensionless file is not something
`CreateProcess` will launch; the bare name is still tried last, so Unix resolution is unchanged
and a deliberately extensionless binary is still found. A name that already carries a recognised
extension is not suffixed again, so `rustic.exe` is not sought as `rustic.exe.exe`. The default
`PATHEXT` used when the variable is absent is deliberately narrower than the Windows default —
`.COM;.EXE;.BAT;.CMD`, omitting `.VBS`/`.JS`, which `CreateProcess` will not launch directly, so
a match on one would bake a path into a task that then fails at run time.

#### Why no test caught it

`0.2.0` verified Task Scheduler end to end on a real machine — register, run, status, unschedule
— and that verification was genuine. It used a **throwaway config that named rustic explicitly**,
so the one path a real user takes, a bare `rustic` resolved from `PATH`, was the one path never
exercised. That is `v0.2.1`'s lesson again in a new costume: *a check is only worth what its
oracle is worth*, and here the oracle was a config written to make the test convenient.

**The failure was loud, which is the part that worked.** `schedule` refused and wrote nothing,
exactly as `0.1.10` intended — no task registered against a binary that could not be found, no
red run at 03:00. The bug cost a person five minutes, not a backup.

#### The error message was systemd-only, on all three platforms

Split into `no_path_fallback_reason()`, which names *this* platform's scheduler: a stored
absolute command on Windows, `PATH=/usr/bin:/bin:/usr/sbin:/sbin` under launchd, `linger` under
systemd. A diagnostic that names the wrong subsystem sends the reader to look for a problem that
does not exist — `~/AGENTS.md` records the same shape as the `bfs`/`find` and `eza`/`ls` traps.

Verified on Windows by generating a real task definition: `Arguments` carries
`--rustic-binary C:\Users\user\.cargo\bin\rustic.EXE`, with the 24 plain triggers `0.2.0`
established. 302 unit tests, up from 297.

#### Also here: the `${env:HOME}` backlog item is closed

Not a code change — the fleet's chezmoi-managed `jobs.yaml` dropped `defaults.rustic-config-dir`
on 2026-08-07, taking the option `PLAN.md` §7.9 recommended, since that key's default is already
`$XDG_CONFIG_HOME/rustic` on all three platforms. The item is struck below rather than left to go
stale, which is the `0.2.4` lesson applied to the file `0.2.4` was about.

### v0.2.4 — `AGENTS.md` said this project was pre-code, for thirty-five releases

**Documentation only. The same defect `0.1.32` fixed in `PLAN.md`, in the other file, found
because only one of the two copies was ever corrected.**

`AGENTS.md` Part 2 §1 "Current state" opened with:

> **Pre-code.** Nothing is implemented. The repository contains this file, `CLAUDE.md`,
> `.gitignore` and `PLAN.md`. Scaffolding is step 1 of Milestone 1.

Written 2026-07-30, before the first commit. It survived to `0.2.3` — through five milestones,
three scheduling backends, two platforms added and eight releases — **in the first section of the
file Part 1 §STEP ONE orders every agent to read in full before touching anything.** So for
almost the whole life of the project, the first factual claim any new session read was false.

#### The finding is not "a file went stale", it is *why this one did*

`0.1.32` corrected the identical sentence in `PLAN.md`'s header and wrote the rule that produced
this file's split. It did not occur to anyone to check whether the same claim existed elsewhere,
and it did — one file was fixed, its duplicate was not.

**Duplicated state goes stale one copy at a time, and the copy that survives is the one nobody
re-reads.** A header is re-read least of all: it is what you scroll past on the way to the part
you came for. So the fix is not just a corrected sentence — §1 no longer *holds* the current
state at all. It states the milestone shape, which changes rarely, and points at `NOTES.md` for
everything that moves.

#### Two more expired clauses in the same file, same cause

- **§0 said "read `PLAN.md` in full before anything else"** and never mentioned `NOTES.md` —
  even though `PLAN.md`'s own header has said since `0.1.32` that it is *not* where you look for
  what is true, and even though §4 promised `NOTES.md` would "become required reading" once
  scaffolding existed. Scaffolding existed at `v0.0.1`. **Sending every session to the historical
  record and not to the living one is precisely how §1 stayed wrong without being noticed.** §0 now
  requires both and carries a two-row table saying which answers which question.
- **§4 still carried the `0.0.x` versioning rule** — "patch bumps only until Milestone 1 ships a
  tool that can run a backup, since `v0.1.0` is reserved for that". M1 shipped at `v0.0.7`. Left
  in place it gives the wrong answer today, so the current rule is now stated with `0.2.0` and
  `0.1.26`/`0.1.27` as the precedents on either side, and the authority pointed at `NOTES.md` §3.

Both superseded passages are kept inline rather than deleted, per the `0.1.16` / `0.1.30` /
`0.1.32` precedent: the mistake is the useful part.

**The Pre-PR Checklist §4 promises is still not written** — it stays a backlog item rather than
being invented here, because smuggling unreviewed process into a staleness fix is its own version
of the problem.

### v0.2.3 — ignore Syncthing conflict copies

**One line of `.gitignore`, and it blocked a release.**

`.gitignore` has carried `.syncthing.*` since PR #49 committed one by accident. Syncthing writes a
**second**, differently-shaped artefact that the pattern does not catch:
`<name>.sync-conflict-<date>-<time>-<deviceid>.<ext>`. Now ignored as `*.sync-conflict-*` — no
slash, so git applies it at every depth, verified by creating probe files at the root and under
`docs/` and confirming neither reaches `git status`.

**The two artefacts are not the same risk, which is why the comment says so.** An in-progress
transfer file is byte-identical to what it shadows, so committing one is noise. A conflict copy is
*by definition a different version* of the file. The one found on 2026-08-07 was a generated
`docs/rusticprofile.1` built from **0.1.31** sitting beside the real 0.2.2 one — commit that and
the tree carries a stale man page under a name that reads as authoritative.

**It also cost a release.** `cargo publish` refuses on an unclean working directory and an
*untracked* file counts, so the `0.2.2` publish failed on a file nobody had written, in a directory
nobody had edited, for a reason the error does not connect to Syncthing. Deleting it by hand
unblocked the upload; this stops the next one arriving.

Checked before adding: no tracked path in the repository contains `sync-conflict`, so the pattern
shadows nothing that exists.

### v0.2.2 — `@` leaked into three shebang recipes

**Tooling only. A regression from `0.2.1`, found by the recipe it broke.**

`0.2.1` converted `install-completions` to a plain recipe and silenced its comment lines with `@#`.
Both edits were applied with a global regex:

```
perl -0pi -e 's{^    #$}{    \@#}gm'
sed  -i    's/^    echo ""$/    @echo ""/'
```

Each matched **every** such line in the file, not only the ones inside the recipe being edited. In
a *plain* recipe `@` means "do not echo this line"; in a **shebang** recipe the body is a script,
so bash receives `@#` and `@echo` as commands:

```
merge-pr: line 413: @#: command not found
merge-pr: line 462: @echo: command not found
```

Landed in `merge-pr`, `pr` and `aur-publish` — the recipes that open and merge pull requests.

**This is `PLAN.md` §7.4's rule** — *do not make structural edits to safety-critical files by
string matching* — applied to the file that holds the gates rather than to a rustic profile. It is
the same shape as the scripted edit that once deleted `[snapshot-filter]` by matching `[forget]`
inside a comment.

#### What is worth keeping is how badly the *checking* went

Three consecutive attempts to find the strays were themselves unreliable, and each failed
differently:

1. A scan piped through `head -40` **truncated the evidence**, so six of seven were fixed and one
   at line 595 was missed entirely. A bounded search reporting a partial answer as if it were
   complete is the emptiness-check failure `~/AGENTS.md` records for `ls <glob>`.
2. An `awk` state machine tracking "am I inside a shebang recipe" reported **zero** while a known
   breakage sat at line 462 — blank lines inside a recipe body reset its state.
3. A regex for recipe headers missed `flamegraph *ARGS="":`, so two *correct* `@` lines in a plain
   recipe were briefly counted as broken.

The check that finally worked does not parse the file at all: it asks **just** for each recipe body
(`just --show`), decides whether it is a shebang recipe from that body, and greps only then. **The
tool that defines the semantics is the only reliable oracle for them** — the same lesson as
`v0.1.5` (asking `hostname(1)` instead of the function the binary uses) and `v0.1.14` (asking a
non-interactive shell about an interactive `fpath`), arriving for a third time in a new costume.

Verified after the fix by *running* the recipes that broke: `just merge-pr` and `just pr` on `main`
now refuse with their own messages and exit 1, rather than dying at exit 127 on a missing command.

### v0.2.1 — a merge now runs the suite it ships from, and two gates that could not fail

**CI configuration and tooling only; no product code changed.** Both halves are the same defect in
different clothes: **a check that cannot fail is not a check.**

#### First: `0.2.0` broke `just` on Windows, and this reverts it

**`set windows-shell := ["bash", "-cu"]` is removed.** It was added in `0.2.0` on the reasoning
that the recipes are POSIX and should not depend on whichever `sh` is first on PATH. That was
wrong in the worst available direction: **`bash` is not on a default Windows PATH, while `sh`
often is** — so it turned a working Justfile into one where every backtick variable failed to
evaluate, taking `just install` and everything else that reads them down with it:

```
error: backtick could not be run because just could not find the shell: program not found
  ——▶ Justfile:29:15
   │ BASH_COMP  := `echo "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"`
```

just's own default is already `sh -cu`, which resolves. **The setting bought nothing and cost the
whole file.**

**Why it shipped is the part worth keeping.** Every Windows check during `0.2.0` was run in a shell
that had already been fixed up with `…/git/current/usr/bin` on PATH — because that is what the
shebang recipes need. So the Justfile was only ever exercised in *the environment the fix creates*,
never the one a user has. `just --list` also does not evaluate backticks, so the smoke check that
looked like it covered this could not have failed. Same family as `0.1.14` asking a
non-interactive shell about an interactive `fpath`: **a check is only worth what its oracle is
worth**, and here the oracle was a pre-repaired PATH.

Verified after the revert with a genuinely default PATH — no `bash`, no Git `usr\bin` — that all
four backtick variables evaluate. The shebang recipes still need that PATH entry; that part was
always true and is documented at the top of the Justfile rather than enforced by a setting that
breaks the file for everyone else.

**That documentation now names `cygpath`,** because it is the requirement with the least helpful
failure. On Windows just translates a shebang recipe's temporary script path with `cygpath` — for
*any* interpreter — so without it every shebang recipe dies before running a line:

```
error: could not find `cygpath` executable to translate recipe `install-completions`
shebang interpreter path: program not found
```

That message names neither PATH nor Git, so it reads as a just bug rather than a missing tool. It
is the same one directory (`usr\bin`) that supplies `bash` and the coreutils, so the fix is
unchanged — but "add this to PATH" is only actionable if the error can be connected to it.
`just install-completions` is now listed alongside `just check` and `just man` as verified working
once it is set.

#### A merge to `main` now runs `full-test`

Before this, a merge ran `build` and nothing else — four host-runner legs. `full-test` ran on tags,
on the weekly schedule and on dispatch, but never on the branch releases are cut from, so between
one release and the next `main` was never checked in the release-shaped containers.

That is the `v0.1.21` gap from a third angle. `0.1.30` narrowed the pull-request matrix and named
the risk; `0.1.31` added the weekly schedule and called the gap closed. It was not: a merge on
Monday afternoon had six days before anything ran the containers over it.

`full-test` now also triggers on a push to a branch. **`ref_type == 'branch'` is load-bearing** on
that clause, because a tag push is *also* `event_name == 'push'` and would otherwise match twice.

**`build` is skipped on push in exchange**, so a merge runs seven jobs rather than eleven. The
workflow already stated that rule for the `schedule` event — "without the second clause this job
would run alongside it and test the same commit twice" — and this applies it to the new case. The
resulting matrix, verified by evaluating both expressions over every event shape this repo
produces:

| event | `build` | `full-test` |
|---|---|---|
| pull request | ✅ | — |
| **merge to `main`** | — | **✅** |
| version tag | — | ✅ |
| weekly schedule | — | ✅ |
| manual dispatch | ✅ | ✅ |

Dispatch deliberately runs both: it is how someone asks for everything.

#### Two checks were about to be lost, and one had never run where it mattered

`build` carries the **golden-staleness gate** and the **binary smoke test**; `full-test` did not.
Skipping `build` on merges would have dropped both from the release path — and they were *already*
absent from tags, since `build` has always been skipped there. So a stale golden could have reached
a release, and nothing would have run the binary at all.

Both now run in `full-test` too. The duplication is seconds; losing them on the path that ships
would have been the expensive half of that trade.

#### `just merge-pr` reported "CI is green" when nothing had run

The gate refuses on `FAILURE`/`TIMED_OUT`/`CANCELLED`/`ACTION_REQUIRED` and on a still-running
check. An **empty** rollup matches none of those, so it printed *"CI is green."* and merged.

**Hit for real on 2026-08-06**, which is how it was found rather than reasoned about: GitHub was
not creating workflow runs for pushed commits — a close/reopen of the PR did not trigger one
either — so the head sat with an empty rollup while the branch looked perfectly mergeable. That is
`v0.1.5` one layer up: that release added this gate because a red check had been merged over, and
"nothing ran" is a state the gate could not distinguish from "everything passed".

It now refuses on an empty rollup and names the recovery (`gh workflow run rust.yml --ref <branch>`).
The check is a string comparison rather than `jq -e length`: `gh --jq` is gh's *built-in* jq, but an
external `jq` is not on a default Windows PATH, and a gate that silently degrades where its
dependency is missing is the thing being fixed, not a way to fix it. Exercised against all four
rollup shapes — empty, whitespace-only, all-success, and one still running.

### v0.2.0 — Windows is a supported platform

**A minor bump, and the first since `v0.1.0`.** The trigger is §3's documented one: the public
`schedule::Backend` enum gained a `TaskScheduler` variant, which breaks an exhaustive match
downstream. The scale of the change is the secondary argument, not the primary one — `0.1.26` and
`0.1.27` added the whole launchd backend as patch bumps, correctly, because they touched no public
type and macOS was already in scope. `status --json`'s `backend` field also gains a third value;
that alone would not bump the schema, since a consumer ignoring unknown values keeps working.

**A declared v1 non-goal is reversed.** `PLAN.md` §7.9 carries the decision and §5.10 the
measurements; both were written before the code, per the §5.9 precedent. The short reason: the
development machine was reinstalled from Fedora to Windows, so the platform with no support became
the platform the work is done on and releases are cut from. `crond` and restic-as-a-backend remain
non-goals.

`nix` moves under `[target.'cfg(unix)'.dependencies]`; `windows-sys` is added for exactly one call.

#### Task Scheduler is the third backend

`schedule`, `unschedule` and `status` work on Windows. `Backend` gains `TaskScheduler`, and
`schedule/schtasks.rs` generates one task definition per job as a **pure function**, exactly as
`systemd.rs` and `launchd.rs` do — nothing written, `schtasks.exe` never consulted, so a definition
can be read before it exists anywhere.

**It maps onto systemd better than launchd did**, which was a pleasant surprise: `StartWhenAvailable`
*is* `Persistent=true`, `RandomDelay` *is* `RandomizedDelaySec=`, and unlike launchd **a real next
fire time is reported** — so `status` shows a genuine `next run` on Windows and `status --json` a
non-null `next_run`.

**Verified end to end on a real machine**, not asserted from unit tests: a throwaway local rustic
repository, a real task registered under `\rusticprofile\win-verify`, `schtasks /Run`, one snapshot
in the repository, `status` reading `last success`, then `unschedule` removing both the registration
and the definition and a repeat being a no-op. Nothing touched the shared repository.

##### The bug that end-to-end verification caught, and unit tests could not

**`schedule` took a backup — and ran `forget` — as a side effect of scheduling.** The obvious way to
express "every hour" is a daily trigger with `<Repetition><Interval>PT1H</Interval></Repetition>`,
and with a `StartBoundary` in the past Task Scheduler treats that as *currently due* and runs it
immediately. That is exactly what launchd's absent `RunAtLoad` exists to prevent (§7.5: do not add a
writer to a shared repository as a side effect), and no assertion about the generated XML would ever
have noticed — the file was correct, the platform's reading of it was not.

`StartWhenAvailable` was the first suspect and is **not** the cause: with it `false`, the task still
ran on registration. Worth recording, because it is the setting that sounds responsible, and
disabling it would have cost the `Persistent=true` equivalent for nothing.

**Hourly is now 24 plain daily triggers**, one per hour, sharing the fixed boundary date — the only
construction measured to get all three properties at once: no run at registration, a correct next
fire time *within the hour* (a future boundary pushes it to tomorrow), and an unchanging boundary so
generation stays pure and re-scheduling is byte-identical. Measurements in `PLAN.md` §5.10.

Re-verified afterwards: `schedule` three times in a row against a fresh repository leaves **zero**
snapshots and reports `installed`, `unchanged`, `unchanged`.

##### Three deliberate departures from the other two backends

- **Both priorities are emitted, breaking the "Standard emits nothing" convention.** On systemd and
  launchd, omitting `Nice=`/`ProcessType` leaves a neutral default alone. **Task Scheduler's default
  priority is 7 — already below normal** — so silence would not mean "normal", it would mean
  "de-prioritised", and `Priority::Standard` would quietly stop meaning what it means elsewhere. So
  `Standard` is 5 and `Background` is 7.
- **Two defaults are overridden because they would stop backups silently.**
  `DisallowStartIfOnBatteries` and `StopIfGoingOnBatteries` both default to **true**, so out of the
  box a laptop on battery takes no backups and unplugging mid-run kills one — nothing failing,
  nothing reported. Both are set `false`, and `ExecutionTimeLimit` is `PT0S` because the default of
  three days would terminate a long first backup rather than report it.
- **The definition file is UTF-16LE with a BOM.** `schtasks /Create /XML` rejects UTF-8 with a
  generic *"The task XML is malformed"*, which points at the XML rather than its encoding.

##### The one place this crate composes a command line

`<Arguments>` is a single string, because Windows has no argv — so `schtasks.rs` has the only
argument-quoting code in the project, implementing the MSVCRT rules (`2n+1` backslashes before an
embedded quote, `2n` at the end). Two things make that safe rather than merely tolerable: **the
child is our own binary**, so the parser on the other side is known, and `<Exec>` is `CreateProcess`
rather than `cmd /c`, so no shell expands anything. A trailing-backslash bug here would silently
merge two arguments — for `--config` that means a scheduled run reading a different file than the one
`schedule` validated.

**Verified through a real task, not only by unit test**: a job whose config lived under a directory
with a space in its name registered with `--config "C:\…\rp space test\jobs.yaml"` quoted and
`--rustic-binary` left bare, then ran and found the file. Wrong quoting would have truncated the
path at the space and the run would have failed to load its configuration.

##### A job object closes the last unfinished mechanism

Windows has no signal to forward, and the interactive case needs none — the console delivers
`Ctrl+C` to every process on it. The case that needed closing is a **scheduled** run: Task
Scheduler's "End" terminates only the process it started, so stopping a job would leave rustic
running against the repository with nothing supervising it.

The child is now put in a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so the *kernel*
terminates it when the last handle closes — on a normal return, on a panic, and on this process
being killed outright, which no in-process handler could ever cover.

**Verified the harshest way available:** `Stop-Process -Force` (i.e. `TerminateProcess`) on
`rusticprofile.exe` while a stand-in rustic slept, and both the shim and its grandchild were gone
afterwards. A failure to create or assign the job warns on stderr rather than failing the run —
the same rule a failed log write follows — because the guarantee's absence is otherwise invisible
until the day something is stopped mid-backup.

##### The shipped example no longer depends on `HOME`

`config --example jobs` set `rustic-config-dir: "${env:HOME}/.config/rustic"`, and `HOME` is
normally unset on Windows — so **the shipped example itself failed to load there**. The key is now
commented out, because its default is already that exact path on all three platforms; it was the
same path spelled less portably. `${env:…}` is for values only the environment knows.

Verified with `HOME` genuinely unset: `config --check` against the emitted example exits 0. The
`${env:HOME}` text that remains in the file is inside a *comment*, which is harmless precisely
because of the load order — YAML is parsed before anything is interpolated, so a comment cannot
contribute a variable. That order was chosen to make the predecessor's two templating traps
impossible, and this is it paying off in a third way.

##### `just` runs on Windows

`set windows-shell := ["bash", "-cu"]`, so non-shebang recipes stop depending on which `sh` is
first on PATH. The shebang recipes need one setup step rather than a change — Git for Windows
ships `bash`, `sha256sum`, `date`, `install`, `grep`, `sed`, `cut` and a real `find` in `usr\bin`,
and prepending that directory makes the whole gate work. The Justfile now says so at the top,
including that a default-PATH `find` is `C:\Windows\system32\find.exe`, a text-search tool.

##### A test raced again, and again the test was the defect

`exec::tests::a_dropped_guard_would_not_serialise_anything` — the guard-lifetime assertion added
in `0.1.33` — **failed on the Windows CI runner while passing locally**. It probes `CHILD_PID_LOCK`
with `try_lock` and demands a specific answer, which only holds while nothing else is using that
mutex. The two Windows tests added here legitimately take it, so the assertion became racy.

Same shape as `0.1.33`, same resolution, and worth stating as the pattern rather than the incident:
**when a test races, ask whether the test is the thing breaking a constraint.** The property under
test is a property of `MutexGuard`, not of which mutex it guards, so it now uses a dedicated mutex
that nothing else touches. Production is untouched — no new lock, no `cfg`, no atomic.

Verified the way `0.1.33` verified its fix: **0 failures in 30 stressed runs** at
`--test-threads=16`, against a reproducible CI failure before.

##### Also here

`login_caveat(backend)` replaces the pair of `if backend == Launchd` checks: **two of the three
backends cannot run a user's job with nobody logged on**, and a third backend arriving is exactly
when a caller forgets one. systemd's `linger` remains the only answer to it. On Windows the way out
is worse than on macOS and says so — `permission: system` runs as SYSTEM, and running as yourself
without a login session needs a stored password or S4U, which relocates the credential problem
rather than solving it.

#### The finding that mattered: `%COMPUTERNAME%` is not the hostname

| source | value here |
|---|---|
| `%COMPUTERNAME%` (NetBIOS) | **`HOST-A`** |
| `hostname` / `GetComputerNameExW` | **`host-a`** |

Since `0.1.34` rusticprofile *records* this name, so the upper-cased form would have done two
silent things: `host_matches` is plain equality, so `enabled-on-hosts` would stop selecting the
machine and its gated snapshot sets would not run; and under `group-by = "host,label"` the host's
existing history becomes a **different retention group** — §3a invariant 1 breached by a value that
reads as cosmetic. Lower-casing the variable was rejected: right here, silently wrong for a host
genuinely named `Web1`. That single call is why `windows-sys` is a dependency.

#### Three platform mechanisms, each answered rather than stubbed

- **No `flock`.** The lock is a file opened with `share_mode(0)`: the open *is* the exclusion, a
  second holder gets `ERROR_SHARING_VIOLATION`, and the kernel releases it when the handle closes
  for any reason. No dependency, no second syscall. `LockFileEx` locks ranges inside a file that is
  still openable, which is weaker.
- **No signals — and forwarding is not needed interactively.** A child sharing the console receives
  `Ctrl+C` from the same keypress. What is absent is the *record*: no handler, so
  `Outcome::interrupted` stays false and `Verdict::Interrupted` is not reported there. Stated, not
  papered over. A *scheduled* run has no console and would orphan rustic; that needs a job object
  with `KILL_ON_JOB_CLOSE` and belongs with the backend.
- **XDG paths on Windows too** — `~/.config`, `~/.local/state`, not `%APPDATA%`. The `0.1.25`
  argument unchanged: `jobs.yaml` is byte-identical fleet-wide, so `${state_dir}` must not mean a
  third place. chezmoi already reads its own config from `~/.config` on Windows.

#### `status` now reports the record with no backend, and that was a real gap

`show_status` returned early on a backendless platform after printing only the host — so `last run`
and `last success` were invisible, while `status --json` treated the backend as an `Option` and had
always emitted them. **Two views of one record disagreeing by output format.** The record does not
come from a service manager, and `last success` is the field to alert on precisely because a run
that never happens is silent — as true for a job driven by hand as for one driven by a timer. The
printer is extracted so both paths share it.

#### Four traps found by porting the tests, all of which bite real configs

Each was a test failure whose cause is a genuine Windows behaviour, so the fixes are documented
where the next person meets them rather than buried in a fixture:

| trap | consequence |
|---|---|
| rustic splits the repository string on `:`, and a **drive letter is that shape** — `C:/…/repo` gives ``The backend type `C` is not supported`` | a local or removable-drive repository needs `local:C:/…`. `opendal:gcs` is unaffected, so production never sees it |
| `\` in a **TOML** string | `TOML parse error` — `\U` is not a valid escape |
| `\` in a **double-quoted YAML** scalar | `did not find expected hexadecimal number` — `\U` opens a Unicode escape |
| `Path::is_absolute()` needs a drive prefix | `/var/log/x.log` is *relative* on Windows and `check_log_is_absolute` refuses it — correctly. A shared `jobs.yaml` with a rooted-but-driveless `log:` loads on Unix and is refused here |

The first three all surface as rusticprofile refusing a config, which reads as a validator defect
and is not one. The `\` → `/` fix is the same idiom the fleet's chezmoi template already applies.

Two test-oracle corrections in the same family the project keeps finding: a golden expectation
comparing a literal `"/cfg/rustic/p.toml"` was asserting a Unix separator rather than asking
`paths::profile_toml` — the `v0.1.5` defect — and the golden harness now normalises the separator
after the placeholder so **one golden set serves every platform**, instead of `just check`
reporting "the argv rusticprofile would run has changed" when only a separator did.

#### Two gaps left open on purpose, both named rather than fixed quietly

- **The shipped `config --example jobs` still uses `${env:HOME}`, and `HOME` is normally unset on
  Windows.** A user copying it verbatim gets an unset-variable error. Not fixed here because the
  choice is a small design decision with a fleet echo — `PLAN.md` §7.9 records both options and
  recommends simply dropping the line, since the key's default is already the same path.
- **`rusticprofile status | more` can still print a panic on Windows.** There is no `SIGPIPE` to
  restore; the print macros panic on a closed pipe there exactly as on Unix. "The fix does not
  apply here" and "the bug does not happen here" are different claims, and only the first is true.

#### The workflow gate needs a PATH entry on Windows, and nothing more

With the **default** PATH it does not run at all: `bash`, `sha256sum`, `date` and `install` are
absent, and `find` resolves to `C:\Windows\system32\find.exe` — a text-search tool, the same
shadowing trap `~/AGENTS.md` records for `bfs`/`find` and `eza`/`ls`. Every shebang recipe fails,
including `golden-is-current`, which `check` depends on.

**Git for Windows ships every one of them in `usr\bin`, and that is the whole fix.** With that
directory prepended, `just check` (including the golden staleness gate) and `just man` were both
run green on this machine — so the Justfile needs no rewrite and the recipes are not the problem.
`set windows-shell` is still worth adding so the non-shebang recipes do not depend on which `sh`
happens to be first on PATH; that lands with the Task Scheduler increment.

#### CI builds and tests on Windows, at pull-request time

**`windows-latest` is in all three matrices** — `build`, `full-test` and `build-release` — and
**`windows-11-arm` joins `full-test` and `build-release`**, so a release ships both
`rusticprofile-windows-x86_64.exe` and `rusticprofile-windows-arm64.exe` alongside the other three
binaries. Windows now has the same two-architecture coverage Linux does.

`windows-arm` is tag-time only, following the `fedora-arm` precedent: it is the second
architecture of a platform already covered at pull-request time, and there is no
architecture-dependent code in this crate, so what it uniquely exercises is the toolchain on
aarch64. It earns a *release artefact* on a different argument from its CI leg — an arm64 Windows
laptop is precisely the machine least likely to have a Rust toolchain on it, so "supported"
without a binary would mean supported for nobody who needs it.

This was very nearly deferred to "the increment after", and deferring it would have been the
`v0.1.21` failure restated: Windows support verified on exactly one developer's machine, green
everywhere CI actually ran, and never run where it ships. The Windows leg is also the only one that
can reach the `share_mode(0)` lock, `GetComputerNameExW`, and the `cfg(not(unix))` branches of
`exec` and `main` — no Unix runner exercises a line of it.

The stale comment that stood at the top of the `build` job — *"Windows is absent from every matrix
here on purpose … adding a Windows runner would only prove the crate compiles somewhere it is not
meant to run"* — is replaced rather than deleted, since it read as a settled decision and would
have been quoted as one.

One step needed `shell: bash` to be honest on Windows: the smoke test's `> /dev/null` would, under
the runner's default PowerShell, create a *file* named `dev\null` rather than discard output — so
it would either fail on the missing directory or quietly leave a file behind, and in neither case
be the smoke test it claims to be.

*Stated this way deliberately: the first draft of this entry said the gate was "unavailable on this
machine", which was true of the default environment and false of the machine. A tool that is one
PATH entry away from working is a setup step, not a missing capability, and the difference decides
whether the next person tries.*

*Platform-independent finding from the same pass: `just check` runs `cargo clippy` **without**
`--all-targets`, so test code has never been linted. Adding it found one real lint.*

323 tests, up from 313.

### v0.1.35 — support non-interactive open-pr

**`just open-pr` now supports non-interactive execution and environment options.**

Before this change, `just open-pr` ended with a bare `gh pr create`, which failed non-interactively with *"must provide --title and --body when not attached to a tty"* after printing "Gate passed".

- In non-interactive contexts (`! [ -t 0 ]`), if no arguments or title/body options are given, `open-pr` defaults to `--fill` so `gh pr create` completes cleanly using commit messages.
- `open-pr` also parses `PR_TITLE`, `PR_BODY`, `PR_BODY_FILE`, and `PR_FILL` environment variables when invoked without explicit positional arguments.

### v0.1.34 — rusticprofile owns the hostname

**Behaviour change, and the widest one this project has made.** rusticprofile now passes
`--host` on `backup` and `--filter-host` on `forget`/`prune`. The delegation boundary moved
deliberately; `PLAN.md` §5.9 records the reversal and the reasoning, and the flag-inventory
test was updated only after that, per its own instruction.

#### The problem was the default, not the configuration

Left to itself rustic asks the OS for a hostname. **Linux answers `foo`; macOS answers
`foo.local`.** One fleet therefore carries two naming conventions in one repository's
history forever, and every filter, query and census has to know which hosts are which. It
caused `0.1.4`'s `.chezmoi.hostname`/`.chezmoi.fqdnHostname` bug and a false alarm on
2026-08-04, and a user without chezmoi had to hand-write the right name into two places with
nothing telling them `.local` would bite.

An earlier attempt shipped as a *pin* the user could set in `rustic.toml`. That was rejected
before merge for the obvious reason: **it was still opt-in, so a new macOS user got `.local`
anyway.** Making the default correct is the whole requirement.

**What makes it possible, measured against rustic 0.11.3: for these flags the CLI overrides
the config file.** `--host from-cli` beat `host = "from-config"`, and `--filter-host` beat
`[snapshot-filter] filter-hosts`. So the answer no longer depends on what any file says.
(Part 2 of `PLAN.md` summarises rustic's precedence as "env > config > CLI", which is wrong
at least here.)

#### `defaults.hostname`

```yaml
defaults:
  hostname: short    # short | full | rustic
```

- **`short`** (default) — the OS hostname up to the first dot. Identical to the OS name on
  Linux; drops `.local` on macOS.
- **`full`** — exactly as the OS reports it.
- **`rustic`** — emit neither flag and defer entirely. The pre-`0.1.34` behaviour.

**The escape hatch exists for two data-integrity cases, not for taste** — which is why this
project, whose instinct is closed sets with no escape hatches, has one here:

1. **Changing the recorded name splits an existing repository.** Stored snapshots keep the
   old name; under `group-by = "host,label"` old and new are different groups, so the old one
   stops being selected and is never retained down again — it accumulates, silently. This
   project's own failure class, which is exactly why it gets a documented way out rather than
   a note. `hostname: rustic` is the migration path.
2. **Short names collide across domains.** `web1.prod` and `web1.staging` both shorten to
   `web1`, putting two machines in one retention group where they forget each other's
   snapshots — the §3a invariant 2 rule broken by default rather than by misconfiguration.
   `hostname: full`.

#### What stops it being silent

**`config --check` and `config --show` print the name that will be recorded whenever it
differs from what the OS reports**, and say which mode produced it:

```
  host              mac-host.local
  recorded as       mac-host  (the OS reports `mac-host.local`; `hostname: short`)
```

Nothing is printed when the two agree, so Linux hosts gain no noise. A behaviour change that
can strand snapshots has to be visible without reading the source.

#### Two validation rules narrow, on purpose

- **`check_forget_is_scoped` no longer requires `filter-hosts` in the profile** under `short`
  or `full` — rusticprofile supplies the scope, so a profile without one is *correct*, and
  demanding it would refuse a right configuration. Under `rustic` it still refuses,
  unchanged, because there the profile is the only scope there is.
- **`check_filter_hosts_can_match` only applies under `rustic`.** Elsewhere our flag wins, so
  a stale `filter-hosts` is harmless leftover rather than the silent-retention bug.

Both are covered by tests asserting the *pair* of behaviours, not just the new one.

#### Not changed: the `snapshots` passthrough

It still adds nothing but `-P` and the operation (§7.8). `--filter-host` is **repeatable and
unions** rather than overriding, so a flag we injected would silently widen a caller's own
filter instead of yielding to it. The honest cost: a profile that drops `filter-hosts` gets a
fleet-wide `rusticprofile snapshots`, and a per-host view needs `-- --filter-host <name>`.

The `[backup] host` reader from the superseded attempt stays — under `hostname: rustic` a
user may legitimately pin it, and the validator has to judge `filter-hosts` against that
rather than the OS name.

Goldens regenerated; they use `--as-host host-a` placeholders, so no real hostname enters a
tracked file. 324 tests, up from 313.

### v0.1.33 — two tests were signalling each other's child

**Test-only change. `run` is untouched, and that is the point.**

The post-merge CI run for `0.1.32` went red on `build (ubuntu-arm)` with two failures, on a
commit that had passed the same leg eighty minutes earlier and changed no source at all:

```
exec::tests::a_failing_child_reports_its_code_rather_than_erroring   left: None  right: Some(1)
exec::tests::a_forwarded_signal_reaches_the_child                    left: None  right: Some(15)
```

**Reproduced locally at 2 failures in 40 runs**, so a race rather than a runner artefact.

#### The mechanism

`CHILD_PID` is a process-global — it has to be, because a signal handler takes no arguments.
`run` stores its child's pid there and clears it after the wait; the signal test stores its
own child's pid and calls `signal_child`. **Cargo runs unit tests as threads in one process**,
so all eight shared that one global.

When `run(["false"])` overwrote `CHILD_PID` inside the window where the signal test fired its
`SIGTERM`, the signal landed on the wrong child: `false` died of a signal, so `code()` was
`None` instead of `Some(1)`; the `sleep` it was aimed at exited normally, so `signal()` was
`None` instead of `Some(SIGTERM)`. **They always failed as a pair** — the fingerprint of one
signal delivered to the wrong process.

#### The tests were breaking the contract, not the code

`run`'s own documentation has always said it is **not reentrant — one child at a time** — and
production honours it: two call sites, `run/steps.rs`'s sequential operation loop and the
`snapshots` passthrough, neither ever with two children alive at once. The suite was doing the
one thing production never does.

**So the lock went in the test module, not in `run`.** Making production reentrant to satisfy
a test that violates its documented contract would be fixing the wrong thing — the same call
`v0.0.7` made, where two tests contending on the run lock were fixed by giving each its own
job name rather than weakening the lock. Production carries no mutex, no new atomic and no
`cfg` branch.

Three details decide whether the fix actually holds:

- **A choke point, not a `lock()` per test.** All spawning goes through one `run_locked`
  helper, so a test added later cannot forget something it would have to go out of its way to
  avoid. Same reasoning as `0.1.28`, where a `--state-dir` flag was written and then removed
  in favour of one harness helper: a guarantee every future author must remember is not one.
- **Poisoning is recovered, deliberately.** A panic holding the lock would otherwise make
  every later test fail with `PoisonError` instead of its own assertion — one real failure
  becoming seven, with the true one hidden. The guarded data is `()`; no invariant can have
  been corrupted.
- **The guard's whole effect is its lifetime**, and `let _ = exclusive();` drops it instantly
  while looking correct — silently restoring the race. `#[must_use]` turns that into a clippy
  error under `-D warnings`, and `a_dropped_guard_would_not_serialise_anything` asserts the
  lock is genuinely exclusive and genuinely released.

The signal test now holds the guard across the whole store/signal/wait sequence rather than
just the store, since the bug was a concurrent `run` landing *between* them.

#### Why a flake was worth a release

`v0.1.5` added the CI gate to `just merge-pr` because a red check had been merged over, and
the argument was that a check nobody believes trains you to merge anyway. **A test that fails
~5% of the time erodes that from the other side.** It also made tagging risky: `full-test`
runs five platforms per tag, so each release had several independent chances to trip it, and
`v0.1.21` is the precedent for a bad tag having to be deleted.

313 tests, up from 312 — the new one is the guard-lifetime assertion.

### v0.1.32 — split `PLAN.md` and `NOTES.md` along the line they always intended

**Documentation only; no code changed, no test changed.** `PLAN.md`'s own header has said
since 2026-07-30 that *"once the repo is scaffolded, the forward-looking parts belong in
`NOTES.md` and this file can shrink to the historical record."* That never happened, and
thirty-one releases later the two files had drifted into three tangled kinds of content.

**The symptom that forced it.** `PLAN.md` opened with **"Status: pre-code. Nothing has been
implemented."** — while five milestones were complete and `v0.1.31` was on crates.io. That is
the first thing `AGENTS.md` Part 2 §0 sends every new session to read. A document whose first
screen is five milestones out of date is the same silent-staleness failure this project exists
to catch, turned inward, and it had survived every review because nobody re-reads a header.

**The split, stated as a rule so the next addition has one to follow:**

| kind | lives in |
|---|---|
| why the design is shaped this way; what was rejected and why | `PLAN.md` Parts 1–3 |
| measurements against rustic 0.11.3 | `PLAN.md` Parts 5, 7 |
| what is built, released, next | `NOTES.md` Current State + release log |
| **rules that can destroy data if broken** | **`NOTES.md` §3a** |

**`NOTES.md` §3a, "Operating invariants", is the substantive addition** — five rules plus
their corollaries, each of which is silent when violated: group named sets by label; exactly
one retention authority per (repository, host); exactly one lock protocol per repository; the
delegation boundary and what a passthrough may be; and why the dangerous decisions live in
rustic's config so `config --example` has to carry them.

**Nothing was moved out of `PLAN.md` and nothing was renumbered.** Seventeen section anchors
are cited from `NOTES.md`, `AGENTS.md`, `WIP.md`, the shipped example and the source — §7.6
alone is referenced ten times — so each promoted section keeps its number and full text and
gains a pointer saying where it is now maintained. **The authority moved; the evidence did
not.** Verified by asserting all seventeen anchors still resolve after the edit.

**Two stale claims in `PLAN.md` Part 4 are corrected in place**, per the `0.1.3`/`0.1.16`
precedent of correcting rather than rewriting: M4's "nothing may prune until lock coordination
lands", superseded by `0.1.11`; and the milestone list's future tense for M2 and M5, both long
complete. M6 is recorded as effectively delivered — man page, completions and crates.io all
shipped — with the AUR package as its one outstanding piece.

**Four backlog items closed as already-done**, which is its own small finding: the publishing
decision (taken at M2, executed at `v0.1.0`), what a partial backup should do (settled in
`0.0.5`), "M4 blocks space reclamation" (wrong, superseded by `0.1.11`), and this migration
itself. A backlog carrying four items that were finished versions ago overstates the work
remaining, which is the mirror of the problem above.

### v0.1.31 — `full-test` runs weekly, so a release is not its first run

**CI configuration only; no code changed.** `full-test` now runs on a weekly schedule
(Mondays 06:37 UTC) as well as on tags and manual dispatch.

**This closes the gap named in `0.1.30` rather than living with it.** `full-test` is the only job
that runs the release-shaped environments — all five platforms, with the Ubuntu and Fedora legs
inside **bare containers** rather than on host runners — and it ran only on tags. That is exactly
how `v0.1.21`'s time-zone-database failure passed every pull request and then broke the release:
the first thing ever to exercise those containers was the tag itself, and the bad tag had to be
deleted. **A suite that is green on every PR and red at tag time has not been run in the
environment it ships from.**

Three details, each a decision rather than a default:

- **Mondays 06:37 UTC.** Off the hour, because GitHub queues scheduled workflows submitted at
  popular times, and a different day from `security.yml`'s Sunday 00:00 audit so a red week
  points at one workflow rather than two.
- **`build` is skipped on schedule events.** On a schedule, `github.ref_type` is `'branch'`, so
  without an explicit `github.event_name != 'schedule'` the pull-request matrix would run
  alongside `full-test` and test the same commit twice.
- **`build-release` and `release` stay tag-only**, so a weekly run tests everything and publishes
  nothing. Scheduled runs also only ever execute on the default branch, which is what a release
  is cut from anyway.

A failed scheduled run notifies the repository owner. That is the point — nobody watches a
schedule they have to remember to check.

#### Correcting `0.1.30`'s stated benefit

That entry said dropping `ubuntu-x64` bought "runner minutes, not wall clock". **Actions minutes
are free on a public repository**, and this one has been public since `v0.0.9` — so the framing
overstated it. What is actually saved is one redundant job per pull request: less queue
contention, one less line in the checks list, less energy. The *reasoning* for the removal is
unaffected, because it never rested on cost: the leg was the intersection of two things each
already covered. Corrected in the workflow comment too, where the next person weighing a matrix
change will read it.

Kept as a correction rather than an edit, per the `v0.1.16` precedent: the mistake was quoting a
benefit without checking whether it applied to this repository.

### v0.1.30 — drop `ubuntu-x64` from pull-request CI

**CI configuration only; no code changed.** The pull-request matrix goes from four legs to
three: `ubuntu-arm`, `fedora-x64`, `macos`.

**The rule, stated so the next removal has one to follow: drop a leg only when it is the
intersection of two things each already covered.** `ubuntu-x64` uniquely covered "Ubuntu host
runner + x86_64", while `fedora-x64` covers x86_64 Linux and `ubuntu-arm` covers the Ubuntu
userland on a host runner. Same argument that removed `fedora-arm` in `v0.0.15`, minus that
one's timing complaint.

**Said precisely, because "redundant" is too loose.** What is genuinely no longer exercised at
pull-request time is `dtolnay/rust-toolchain@stable` **on x86_64** — the remaining x86_64 leg
installs rustup inside a container instead. That action still runs here on `ubuntu-arm` and
`macos`, and on x86_64 in `full-test`. The only platform-specific code in this crate is a
handful of `nix` syscalls plus the systemd/launchd split, none of which can distinguish an
x86_64 host runner from an x86_64 container.

**Nothing loses tag-time coverage.** `full-test` still runs all five platforms including
`ubuntu-x64`, and `build-release` still produces every artefact — so nothing ships untested,
which was also the closing argument for the `fedora-arm` removal.

**What it buys, honestly: runner minutes, not wall clock.** Measured on the `#44` run,
`ubuntu-x64` took 37s while `fedora-x64` at 66s set the critical path either way. A pull request
will not feel faster. `WIP.md` §1a had recorded that caveat, along with the counter-argument that
`ubuntu-x64` is the most standard environment and therefore the easiest failure to reproduce
locally — which is real, and is outweighed by the fact that its coverage exists twice over.

**The cost that is not zero, and is worth naming rather than burying.** Trimming the
pull-request matrix widens the gap that let the `v0.1.21` time-zone-database failure pass every
PR and then break the release: `full-test` runs only on tags, in leaner containers. That gap is
not created here, but it is made slightly wider. **If it bites again the fix is to run
`full-test` on a schedule, or before a release rather than during it — not to restore a leg
whose coverage is already duplicated.** Recorded in the workflow itself, where someone weighing
a red release will read it.

**Checked before removing, rather than assumed:** no job `needs:` the `build` job (`full-test`
→ `build-release` → `release` is the tag chain), so nothing is stranded. Branch protection could
not be read with the available token (HTTP 403), but `v0.1.5` records PR #19 merging with
`build (fedora-x64)` red — which is only possible with no required status checks, so there is no
required `build (ubuntu-x64)` left waiting for a leg that will never report. The `macos` leg was
re-confirmed as **not** removable by the same reasoning: it is the only leg that exercises
launchd at all.

### v0.1.29 — the second host is cut over, and a redaction failure in this file

**Documentation only; no behaviour changed.** Two things, and the second is a defect in this
file rather than in the code.

#### `host-e.local` is cut over — the first macOS host to take its own backups

Ladder rung 9 for a second machine, and the first on macOS. **It left the control group to do
it**, deliberately and with authorisation, so `AGENTS.md` §3 is corrected: the control group is
now `host-c` and `host-g.local`, and `host-g.local` is the only remaining evidence of what an
un-migrated Mac looks like.

Measured from the repository rather than from either tool's report:

| | before | after | |
|---|---|---|---|
| `host-e.local` snapshots | 25 | **23** | +3 backed up, −5 forgotten |
| repository total | 689 | **687** | same net |
| packs | 784 | **788** | new data; prune is off, so nothing was reclaimed |
| **other hosts** | 664 | **664** | **unchanged** — `[snapshot-filter]` held under a real irreversible operation |

The 5 removals were authorised individually from a `forget --dry-run` beforehand: four same-day
near-duplicates from 2026-03-09 and one from 2026-02-04, with 20 kept including every
monthly and yearly anchor back to 2025-08-21. Each was confirmed gone afterwards by ID.

**The `group-by` in the profile was verified to take effect rather than assumed.** The
config-driven dry run and an explicit `--group-by host,label` both said 5; rustic's default
`host,label,paths` said 4. A differential test, because "the key is present" is not "the key is
doing something" — §7.7 records that rustic accepts and ignores these keys in the wrong
section.

**One rollout step earned its place, and it is the reason that list exists.** `WIP.md`'s
procedure says to confirm `enabled-on-hosts` coverage *before* arming anything. On this host
both gated-off sets' sources were present — `~/.gnupg` holding a **real keyring**
(`trustdb.gpg`, `pubring.kbx`) — while the host was in neither set's list. Arming first would
have backed it up hourly, reported `success` every time, and never once saved those keys.
Nothing would have failed; the sets simply would not have run. The host was added to both
lists first, and the run then reported `backup saved 3 of 3 snapshot sets`.

**Retention authority moved before the schedule arrived**, per §7.5's non-optional ordering: the
predecessor's LaunchAgent was durably disabled with `launchctl disable` — the launchd analogue
of `systemctl --user disable` — with its plist left installed, so the reversal is one command.
Exactly one retention authority on the host, checked rather than assumed.

Exclusions were verified against the **stored** snapshot with positive controls, as on the
first cutover: password file, cloud credentials, `.cache`, `node_modules` and `.DS_Store` all
absent; `.ssh` 48 files and `chezmoi` 422 present, so the check could actually fail. **A first
attempt at that check was wrong in a way worth recording:** `grep -c ".cache"` reported 11
matches, because an unescaped `.` matches any character — `_cache` and friends. `grep -Fc
'/.cache'` reports 0. A verification step whose oracle is a sloppy pattern can report a
failure that is not there, which is the same class of error as the ones that report success
that is not there.

#### This file was leaking live hostnames, and had been since 0.1.4

§3 of this document says: *"No live infrastructure identifiers in tracked files. The repository
is public… Grep the diff before opening a PR, and beware substring false positives."*

Doing exactly that grep found **11 real hostnames in this file's own release log**, in the
`0.1.4`, `0.1.12` and `0.1.24` entries — quoted inside example error messages and command
output, where they read as illustration and slipped past review three times. Now redacted to
the `host-x` vocabulary.

The substring warning proved its worth in the same pass: a naive search for one of the names
matches **"mechanism"**, which occurs 17 times across these documents. Most of the hits were
that word.

**A `#[cfg(test)]` guard was itself part of the leak.** The test asserting the shipped examples
carry no real home directory did it by checking for one hard-coded developer path — a live
identifier in a public file, inside the test written to prevent live identifiers in public
files, and one that could only ever catch a leak on the single machine it named. It now asks
`dirs::home_dir()` for *this* machine's home.

**Redacting the file does not purge the history.** The earlier commits remain readable, exactly
as recorded for the fork incident in `WIP.md` §8: the current tree is clean, and past objects
are reachable to anyone who looks for them. Accepted rather than escalated, on the same
reasoning — these are hostnames on a private fleet, not credentials.

### v0.1.28 — `cargo test` was destroying the real status record

**The test suite was overwriting the very field the tool exists to protect.** Found while
cleaning up after the M3 verification, and it had been true since M5 landed the status file.

`run` records each job at `$XDG_STATE_HOME/rusticprofile/status/<job>.json`. The integration
fixtures use the job name **`dot-files`** — which is also the fleet's live hourly job. So a
plain `cargo test` on such a host wrote this into the real state directory:

```json
{ "job": "dot-files", "host": "host-a",
  "last_run": "2026-08-03T21:42:25-07:00", "last_verdict": "success",
  "last_success": "2026-08-03T21:42:25-07:00", "skipped": [] }
```

`"host": "host-a"` is a **fixture's** hostname. Two consequences, and the second is worse:

1. It destroys the `last_success` history — the one field `v0.1.18` added because *"a run that
   fails is loud, a run that never happens is silent"*, and the field `WIP.md` says to alert
   on.
2. It replaces it with a **fabricated success**. A monitor reads a run that never happened, and
   if the next real run fails, that fake success is carried forward by the very
   carry-forward rule that makes the field useful.

It demonstrated itself before being fixed: `rusticprofile status` on the macOS host reported
`dot-files` last succeeding at 21:47:29, on a machine where that job has never once run.

**Fixed in the harness, with no product surface added.** Every child the integration tests spawn
now gets `XDG_STATE_HOME` pointing at a scratch directory. That variable is already the
documented contract — the man page's FILES section says so, and `0.1.25` made it authoritative
on macOS too — so nothing new had to be invented.

**A choke point, not a per-test flag.** All eleven spawn sites go through one `command()`
helper, so a test added later cannot forget to be hermetic. A `--state-dir` flag was written
first and then removed: it would have had to be remembered in every future test, which is a
weaker guarantee for more surface.

**Verified by deleting the real state tree and running the full suite**: it was not recreated.
That is the assertion that matters, because a count of passing tests says nothing about what
they wrote.

**One real simplification came out of it.** `main.rs` resolved `paths::user_state_dir()` in
**three** separate places — the runner, `status`, and `status --json` — so three code paths
independently decided where state lived, and the reader could disagree with the writer about
which file is the record. It is now resolved once in `config::load` and carried as
`Config.state_dir`. A new integration test asserts the round trip: `run` writes under
`XDG_STATE_HOME`, and `status` reading the same tree finds it.

Also corrected: `schedule/mod.rs` still opened with "macOS launchd is M3", written when it was
future work and left behind by `0.1.27`.

313 tests, up from 312.

### v0.1.27 — M3 complete: macOS schedules itself

**`schedule`, `unschedule` and `status` work on macOS.** `backend_is_available()` becomes
`current_backend() -> Option<Backend>`, and the four commands that touch the world dispatch on
it. Everything above that match is shared: the same `at:` vocabulary, the same `UnitContext`,
the same spread window, the same host gating, the same validation.

**Verified end to end on the macOS host, not asserted from unit tests.** A throwaway local
rustic repository under a temp dir, a real agent installed into the real
`~/Library/LaunchAgents`, then `launchctl kickstart`:

```
2026-08-03T21:42:48-07:00 success m3-verify on host-e.local
  backup   success  backup saved 1 of 1 snapshot sets (0.764s)
    -> /opt/homebrew/bin/rustic -P …/p.toml backup --json --name core
  forget   success  forget succeeded (0.630s)
```

launchd reported `runs = 1`, `last exit code = 0`, `nice = 19`, `spawn type = background (5)`;
the snapshot was in the repository; the status file recorded `last_success`; `status` agreed;
`unschedule` booted the agent out and removed the plist, and repeating it was a no-op. Nothing
touched the shared repository or that host's snapshot baseline.

#### A bug found by using it, not by testing it

**The fleet spread silently did nothing.** `schedule` printed `runs hourly at 0 past`, which
looked like luck until the seed was checked: the first implementation took
`Timestamp::now().subsec_nanosecond()` and reduced it modulo the 5-minute window. **macOS's
clock has microsecond resolution**, so that field is always a multiple of 1000 — and
`1000 % 5 == 0`. Measured 10 samples out of 10 at minute 0. Every host in the fleet would have
landed on the same instant while `schedule` reported a spread.

That is this project's own failure class, in the feature written to prevent contention. The
seed is now `RandomState::new().hash_one(…)` — OS-seeded, no dependency added, no clock
involved — and a regression test folds 200 seeds and fails if they all land on one minute.
Re-verified by installing three times: minutes 3, 1, 1.

**The lesson generalises past this bug: a low-resolution clock is a bad source of
arbitrariness whenever the consumer reduces it modulo a small number**, and the failure is
invisible unless you look at the value. `Offset::within` is left as plain modulo on purpose —
the reuse path needs a minute read back out of a plist to come back unchanged — so the
robustness has to live in the seed.

#### Idempotence, which took deliberate work here

A random offset and a byte-comparison idempotence check pull against each other: a fresh choice
on every run would rewrite an unchanged agent, move the host's slot and report `installed`
forever. So `write_agent` **reads the installed plist and reuses the offset it finds**, falling
back to a new one only for a first install or a plist hand-edited past recognition. Confirmed
through the binary rather than the generator: a second `schedule` reports `unchanged` and the
files are byte-identical.

#### Three things launchd needs that systemd does not

**`enable` before `bootstrap`, and it earns its place.** A persistent `launchctl disable`
survives bootout/bootstrap, so without it `schedule` on a job somebody once disabled would
report success and schedule nothing. Verified by disabling an installed agent behind the tool's
back: `status` correctly read `installed, not enabled`, and the next `schedule` cleared the
override and returned it to `active`.

**`bootout` before `bootstrap`.** `bootstrap` fails outright when the service is already
loaded, so re-arming needs the old registration gone. Its failure on a first install is the
normal case and is ignored.

**`gui/<uid>` as the domain**, with the uid from `getuid()` rather than `id -u`. Asking a
different oracle than the code uses is the v0.1.5 defect, where a fixture shelled out to
`hostname(1)` and disagreed with the binary on every containerised runner.

#### What `status` now says, and one new JSON field

```
host: host-e.local
backend: launchd
  m3-verify              active
    declared             hourly (user, background)
    next run             not reported by launchd; the agent's StartCalendarInterval has the schedule
    last run             2026-08-03T21:42:48-07:00 (success)
    last success         2026-08-03T21:42:48-07:00

  note: a user LaunchAgent runs only while you are logged in — launchd has no equivalent of
        systemd's `linger`, so a Mac sitting at the login window takes no backups. …
```

**The `next run` line is printed even though there is nothing to print**, the same way `never
recorded` is: launchd reports the calendar descriptor and no next firing, and a blank line
there would read as "nothing is scheduled", which is a different claim.

**The login caveat is stated by `schedule` *and* `status`.** It is the one thing a macOS
schedule cannot promise, it cannot be fixed by more code, and a Mac at the login window fails
nothing at all — the absence-shaped failure `last_success` exists for. `permission: system`
installs a LaunchDaemon that runs regardless, as root.

**`status --json` gains `backend`** (`"systemd"`, `"launchd"` or `null`), **without a schema
bump**, which the schema's own contract allows — fields may be added and a consumer ignoring
unknown ones keeps working. It earns its place by explaining an absence: `next_run` is always
null under launchd, and a monitor otherwise cannot tell that from a timer it failed to read.

#### Also corrected here

**The shipped `config --example jobs` claimed something the validator rejects.** Its `at:`
comment read `hourly | daily | weekly | monthly, or an OnCalendar expression`. There is no
OnCalendar escape hatch — `At` is a closed enum of four values, deliberately (`PLAN.md` lists
templating "in any form" as a non-goal, and an arbitrary calendar expression is the same
bargain in a different costume). The example is tested through the real binary, but a test can
only prove the config *parses*; it cannot notice a comment that is false. Now corrected, and
the launchd differences are documented there too.

**`config --show` said a declared schedule was "not installed until M2".** M2 landed nineteen
versions ago.

263 unit and 49 integration tests, up from 249 and 45.

### v0.1.26 — M3 begins: launchd agent generation

`schedule/launchd.rs`, and the same split M2 used: **pure functions here, nothing written,
`launchctl` never consulted**, so an agent can be read before it exists anywhere. `schedule`
still refuses on macOS — deliberately, since flipping the guard before anything can install
would write plists nothing bootstraps, which is the silent success the guard exists to stop.
Wiring is the next version.

**`permission` and `priority` already meant the same things, so this is a second backend
rather than a second design.** Four differences are real, and each was measured on macOS 26.6
rather than reasoned about:

**One agent, not two units.** systemd cannot run a command from a timer, so a job there is a
`.service` plus a `.timer`. launchd puts the schedule and the program in one job, so a job
here is one plist. The two-file shape was never a design choice, and its absence is not a gap.

**No `RandomizedDelaySec`.** `StartCalendarInterval` names an instant with no tolerance, so
the fleet spread cannot be a separate directive — it has to be part of the calendar
specification, which puts the offset in the plist itself. `calendar::Offset` is bounded by
the **same window** systemd gets, derived from `randomized_delay` rather than written out
again: two numbers that must agree are two numbers that can drift, and drift here would make
`at: hourly` mean something different on macOS than on Linux from the same line of the same
byte-identical `jobs.yaml`. Base instants match too — hourly `:00`, daily `00:00`, weekly
Monday, monthly the 1st.

**Missed runs: half of `Persistent=true` comes free, half does not, and they are different
cases.** For sleep, `launchd.plist(5)` is explicit — *"Unlike cron which skips job
invocations when the computer is asleep, launchd will start the job the next time the computer
wakes up. If multiple intervals transpire before the computer is woken, those events will be
coalesced into one event upon wake"* — which is what `Persistent=true` exists for, including
the one-catch-up-run behaviour measured on Linux (`WIP.md` §12: one run on resume, not
eleven). But a calendar time that passes while the agent is **not loaded** is *not* caught up:
measured by bootstrapping an agent whose minute had already gone by, which reported
`runs = 0`. So the first run after `schedule` is at the next occurrence, never immediately.

**No `linger` equivalent, and this one is a real limitation.** A systemd user manager can be
told to run with nobody logged in; launchd cannot. A `gui/$UID` agent runs while the user is
logged in, so **a Mac sitting at the login window does not back up.** `permission: system`
installs a LaunchDaemon instead, which runs regardless and carries the same trade a systemd
system unit does — it runs as root, which needs its own answer for credentials. Stated in the
module docs now and surfaced by `status` in the next version, because a schedule that only
works while someone is logged in is exactly the kind of thing that looks fine for months.

**Two measurements that confirmed the design rather than changing it**, both from a throwaway
agent bootstrapped into `gui/501` and then torn down:

| measured | consequence |
|---|---|
| `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `PWD=/` | the v0.1.10 absolute-path rule carries over exactly — a Homebrew `/opt/homebrew/bin/rustic` is invisible to a launchd agent, and a relative path resolves against `/` |
| `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `SSH_AUTH_SOCK` all arrive | no `EnvironmentVariables` block is needed; the environment stays inherited unmodified, as everywhere else in this tool. The predecessor's plist sets `HOME` and a full `PATH`, and neither is necessary |

`UnitContext` moved from `systemd.rs` to `schedule/mod.rs`, since both backends need the same
three absolute paths for the same reason — one doc comment now carries both platforms'
evidence instead of one platform's.

**Four things are deliberately absent from the plist**, each recorded where someone changing
it will read them: **`RunAtLoad`**, which would take a backup the instant `schedule` installed
the agent — adding a writer to a shared repository as a side effect, which is what §7.5
forbids; **`KeepAlive`**, which would restart a one-shot backup on exit; **`WorkingDirectory`**,
unnecessary once every path is absolute; and **`StandardOutPath`/`StandardErrorPath`**, which
looks like the questionable one. launchd discards both streams and macOS has no journald, so
nothing captures them — but a fixed path would be an unrotated file growing forever, which is
what left 904 KB behind in the predecessor's setup. The per-run record is the `log:` file and
the status file, `launchctl print` reports `runs` and `last exit code`, and `last_success` is
the field that answers whether a job still works.

**A plist is XML, so a value from configuration must not be able to change its structure.**
Every interpolated value is escaped: a home directory containing `&` would otherwise produce
a file launchd refuses to parse, and an agent that fails to load is a schedule that silently
does not exist. Same reasoning as refusing a snapshot-set name that starts with `-`.

**Tested through `plutil`, not only by substring.** Substring assertions prove the content;
only a parser proves the file. `plutil -lint` is run over all eight interval/priority
combinations, skipping with a printed notice off macOS — the convention the rustic-backed
tests already use. Plus the invariants inherited from the systemd side: no agent contains a
date, every path in an agent is absolute, generation is pure, `Standard` priority emits
nothing rather than `Nice=0`, and Monday is `Weekday = 1` (`launchd.plist(5)`: "0 and 7 are
Sunday") so a weekly backup cannot quietly move a day.

249 unit tests, up from 228.

### v0.1.25 — on macOS, the tool could not find its own configuration

**Every command exited 2 on a Mac**, and the reason was one line of platform courtesy:

```
$ rusticprofile config --check
error: 1 configuration error:
  /Users/u/Library/Application Support/rusticprofile/jobs.yaml: could not be read:
    No such file or directory (os error 2)
```

`dirs::config_dir()` returns each platform's own convention, which on macOS is
`~/Library/Application Support`. Meanwhile the man page's FILES section, the README and the
shipped `config --example` all documented `$XDG_CONFIG_HOME/rusticprofile/jobs.yaml`, and
chezmoi — which generates this fleet's configuration — writes it to `~/.config`. **The
documented contract was already the XDG one; only the code disagreed.**

`user_config_dir`, `default_rustic_config_dir` and `user_state_dir` now apply the XDG rules
on macOS as well as Linux. **Nothing changes on Linux**: `dirs` implements exactly these
rules there, so the resolved paths are identical.

**The reason is not tidiness, and not "XDG is better".** `jobs.yaml` is designed to be
byte-identical across a fleet — that is why it needs no templating while `rustic.toml` must
be generated (§5.9). A config location that varies by operating system makes one line of one
file resolve to two different places, and `${state_dir}` in the shipped `log:` would have
pointed under `~/.local/state` on five hosts and `~/Library/Application Support` on two. A
variable that means something different depending on which machine reads it is the
fleet-wide version of the failure this project keeps finding.

**This is a behaviour change on macOS, stated rather than hidden:** a Mac that had a config
under `~/Library/Application Support/rusticprofile` must move it to `~/.config/rusticprofile`.
No fallback probe was added — a location that depends on which directory happens to exist is
how the same command comes to read different files on two machines, and the error already
names the exact path it tried.

Two details from the specification are honoured and tested, because both bite here
specifically: **a relative `XDG_CONFIG_HOME` is ignored**, since a scheduled run's working
directory is whatever launchd or systemd chose and honouring it would make the config a job
loads depend on that; and an **empty** variable is the same as unset. The rules are a pure
function taking the variable and `$HOME` as arguments — `std::env::set_var` is `unsafe` in
edition 2024 and races every other test in the binary, so testing them through the process
environment was not an option.

Verified against the fleet's real generated config on the macOS host rather than a fixture:
`config --check` passes, `plan --format lines` emits the expected argv, and 228 unit plus 45
integration tests pass — **including the three rustic-backed integration tests**, which ran
for the first time on macOS because rustic 0.11.3 is now installed there.

### v0.1.24 — name the version skew, and refresh a stale README

**Two loose ends from the host-d incident.**

**The error now names the running version.** Unknown keys and variables stay hard errors —
that rule caught a config block in the predecessor which had silently never taken effect
(§2.2). But there is a second cause the message could not distinguish: `jobs.yaml` is
designed to be byte-identical across a fleet, which makes it **only ever as new as the
oldest binary reading it**. Pushing a config using a newer key stops backups on every host
still on an older build, at its next scheduled run, with nothing to say why.

That happened on 2026-08-03: `${state_dir}` reached a host three versions behind, and its
hourly backup failed from then on. The failure was correct; the **diagnosis** was missing.

```
unknown variable `${state_dir}`; valid names are … If this configuration is shared between
machines, it may use a variable added after rusticprofile 0.1.23 — compare
`rusticprofile --version` across hosts
```

Applied to both shapes a skew takes: an unknown `${variable}`, and serde's `unknown field`
for a key. No check is weakened — one line is added to an error that already fired.

**The README had gone stale**, and the audit was done by comparing it against the binary
rather than against memory:

- Status said "Milestones 1 and 2 complete". **M5 is complete too**, and observability —
  the whole of it — appeared nowhere.
- *What it does* listed nothing about the run log, the status file, or `--json`.
- The configuration sketch predated `default-job` and still showed no `log:` at all.
- Nothing warned that a shared `jobs.yaml` is as new as the oldest binary — now an
  `[!IMPORTANT]` block, since that is the failure most likely to bite someone else.
- `just install` promised "completions for six shells" without mentioning that **zsh
  usually will not load them** without an `fpath` entry.

Every command in the README was checked against `--help` output; all seven exist and the
block now shows `run --json` and the `default-job` fallback.

### v0.1.23 — M5 complete: `--json`

`run --json` and `status --json`. **Milestone 5 is done.**

Anything automated reading the human summary would be matching English —
`backup  ok  backup saved 3 of 3 snapshot sets` — which is exactly what `rustic/exit.rs`
refuses to do to *rustic's* output, and for the same reason: a summary line is a message to
a person, and it changes when the wording improves.

```
$ rusticprofile status --json 2>/dev/null
{
  "schema": 1,
  "host": "host-a",
  "jobs": [
    {
      "job": "dot-files",
      "scheduled": true, "units_present": true, "enabled": true, "active": true,
      "next_run": "Mon 2026-08-03 16:04:53 PDT",
      "last_run": "2026-08-03T15:31:55-07:00",
      "last_verdict": "success",
      "last_success": "2026-08-03T15:31:55-07:00",
      "skipped_last_run": []
    }
  ],
  "not_on_this_host": [ { "job": "dot-files-prune", "enabled_on_hosts": ["host-d"] } ]
}
```

Four decisions worth keeping:

- **Shaped separately from the internal types.** `RunJson` is not `#[derive(Serialize)]` on
  `JobReport`. An internal type may be refactored freely; an emitted schema is something a
  monitor depends on, and coupling them makes every rename a silent breaking change for
  somebody's alerting. The mapping is written out once, where a change to it shows in a diff.
- **`schema` is the promise.** Fields may be added without changing it; anything removed or
  redefined bumps it. A consumer ignoring unknown fields keeps working.
- **`null` is not `false`.** `enabled`, `active` and `last_success` stay null for "could not
  tell" or "never". Collapsing those to `false` would make a monitor confident about
  something nobody measured — the same distinction `TimerStatus` already draws.
- **`skipped` is emitted even when empty.** A consumer asking "did retention run?" should not
  have to tell an absent key from an empty list.

The log and status file are written whichever format was asked for: they are the record, not
the report, and the caller's choice of output says nothing about whether a run should be
remembered.

rustic's progress goes to stderr, so `run --json 2>/dev/null` is parseable as-is.

### v0.1.22 — AUR package tracks 0.1.21

`packaging/aur/` was pinned to `0.1.9` and superseded four times over while the AUR sat in a
maintenance window. Re-pointed at `0.1.21` once rather than bumped repeatedly through
versions nobody could install.

Rebuilt and linted in `archlinux:base-devel`: `makepkg` completes, `check()` runs **217 unit
and 43 integration tests against real rustic 0.11.3**, the payload is the binary, three shell
completions, the gzipped man page, README and LICENSE, and `namcap PKGBUILD` is clean.

Still not pushed — the AUR has been in maintenance for the whole of this work. `just
aur-publish` completes it, and refuses on its own if the window has not lifted.

### v0.1.21 — tests must not need a time zone database

Tagging `v0.1.20` failed the release build. Nine tests in `run::log` and `run::status`
panicked on `full-test (ubuntu-arm)`:

```
called `Result::unwrap()` on an `Err` value: failed to find time zone
`America/Los_Angeles` since there is no time zone database configured
```

The fixtures parsed `"…[America/Los_Angeles]"`, which needs a tzdb the release container
does not have. The assertions never cared about the *zone* — only about a fixed instant —
so the dependency was incidental and is now gone: a `Timestamp` plus
`TimeZone::fixed(Offset::constant(-7))` needs nothing from the machine.

**It passed every pull request.** `full-test` runs only on tags, in a leaner container than
the PR-time `build` matrix, so the first thing to exercise it was the release itself. That
is the gap worth noting rather than the parse error: a test suite that is green on every PR
and red at tag time is one that has not actually been run in the environment it ships from.

Verified by reproducing the condition rather than reasoning about it — `rm -rf
/usr/share/zoneinfo` in a container, then running the affected tests, which now pass.

The bad tag was deleted before any artefact was published, so `v0.1.20` never existed
outside this repository.

**Third instance of one pattern in this release series**, and worth naming: `0.1.5` asked
`hostname(1)` instead of the function the binary uses; `0.1.14` asked a non-interactive
shell about an interactive `fpath`; this asked for a named time zone when it wanted an
instant. Each time the test looked reasonable and quietly depended on the machine it was
written on.

### v0.1.20 — `defaults.default-job`

```yaml
defaults:
  default-job: dot-files
```

`run`, `plan`, `snapshots` and `config --show` use it when `-n` is omitted. An explicit
`-n` always wins.

**Two commands deliberately ignore it, and that is the part worth reading:**

- **`unschedule`** still requires an explicit name. Removing a schedule because a
  configuration file named a default, rather than because someone typed it, is the one
  action that should never happen by default. Its own doc comment already said "removal is
  always named explicitly"; this keeps that true.
- **`schedule`** already treats a missing `-n` as *"every job that declares a schedule"* —
  useful in its own right, and a default would silently replace it with something narrower.

Both exclusions are stated in the man page, the shipped `config --example jobs`, and the
CLI help, because a fallback that applies to four commands out of six is exactly the kind of
thing a reader will otherwise assume is uniform.

**Validated against the *declared* jobs**, not the ones surviving host gating — the same
reasoning as snapshot-set names. A default gated off on this machine is legitimate and
reports the gate when used; a default that is a *typo* is wrong on every machine, and
catching it only where that job happens to run would be catching it in the wrong place:

```
defaults.default-job: `dot-filez` is not a job in this configuration, so every command
  that falls back to it would fail. Defined jobs are dot-files, other
```

**The "no job and no default" error names the config file** and shows the two lines to add.
That check moved out of clap to make this possible — clap cannot mention a file it has never
read, and "the following required arguments were not provided: --name <JOB>" tells a reader
nothing about where a default would go.

### v0.1.19 — `snapshots`, a read-only passthrough

The predecessor answers `resticprofile @dot-files` with a snapshot listing, because its
config sets `default-command: "snapshots"`. Migrating removed a daily habit and replaced it
with `rustic -P ~/.config/rustic/dot-files.toml snapshots` — a path the user must remember
and **rusticprofile already resolves and validates**.

```bash
rusticprofile snapshots -n dot-files
rusticprofile snapshots -n dot-files -- --filter-label core
```

**The value being added is profile resolution, not a new capability**, and the command adds
exactly that:

- It emits `rustic -P <resolved> snapshots` plus whatever the caller appended after `--`.
  Every flag beyond `-P` is the caller's; rusticprofile constructs none.
- rustic's **exit code passes straight through**. Inventing a verdict here would be a second
  opinion nobody asked for.
- stdout is **inherited, not captured** — this exists to be read by a person, and rustic's
  own table beats anything reprinted from a parse.

**It is not an `Operation`.** That enum stays `backup`, `forget`, `prune`, because it is what
a *job* may schedule and a query is not schedulable work. Letting it into `jobs.yaml` would
be a real boundary move rather than an ergonomic one.

**The line is now written down** (`PLAN.md` §7.8), so the next request of this kind has an
answer rather than a precedent: **a passthrough is acceptable only where it is read-only and
adds no flags.** `check` would qualify. `forget` and `prune` do not — destructive, and their
scoping belongs in the rustic profile where a flag typed at a prompt cannot contradict it.
`restore` never does; the non-goals settled that already.

The flag-inventory test in `rustic/invoke.rs` carries the instruction *"if this test needs
changing, the delegation boundary is moving and that belongs in `PLAN.md` first"*. It did, so
§7.8 was written first. The test itself is **unchanged** — it guards *job* invocations, and
this is not one. `query_argv` is separate and has its own test asserting rusticprofile
contributes only `-P` and the operation word.

### v0.1.18 — M5: a status file, and `status` answers the real question

**A run that fails is loud. A run that never happens is silent** — a disabled timer, a
laptop asleep for a week, a job gated away after a hostname changed. Nothing fails, so
nothing reports, and the only evidence is an absence. Absence is what nobody notices, and it
is the failure class this project exists to prevent.

`$XDG_STATE_HOME/rusticprofile/status/<job>.json` now records it:

```json
{
  "job": "dot-files",
  "host": "host-a",
  "last_run": "2026-08-03T14:43:55-07:00",
  "last_verdict": "failure",
  "last_success": "2026-08-03T14:43:37-07:00",
  "skipped": ["forget"]
}
```

**`last_success` survives a failed run.** A file recording only the latest attempt answers
"did the last run work?" — useful, but not the question. The one worth asking is *when did
this last actually work?*, so a failure carries the previous success forward instead of
overwriting it. That single field is what makes "hasn't succeeded in three days" a check
rather than a discovery.

**`status` now shows it**, which is where the question gets asked:

```
  dot-files              active
    declared             hourly (user, background)
    next run             Mon 2026-08-03 15:00:05 PDT
    last run             2026-08-03T14:43:55-07:00 (failure)
    last success         2026-08-03T14:43:37-07:00
    skipped last run     forget
```

A timer can be armed, green and firing while **every run fails** — which happened on a fleet
host earlier the same day, for hours, surfacing only because someone happened to look. The
schedule cannot answer that; the record can. A job with no record at all reads
`never recorded` rather than being silently omitted.

Three more decisions, each tested:

- **Partial counts as success.** A partial backup saved data and, by the rule in
  `run/steps.rs`, retention still ran. Recording it as "never succeeded" would make a
  monitor cry wolf on a job doing its job.
- **The write is atomic** — temp file plus `rename(2)`. A monitor polling this must never
  read a half-written record and conclude something false about a backup.
- **A corrupt record reads as absent, not as an error.** Losing history is bad; losing the
  *current* run because the previous one is damaged is worse.

One file per job, so two jobs running at once cannot contend on a read-modify-write and lose
the loser's record. As with the log, a write failure never changes the exit code.

Still to come in M5: `--json`.

### v0.1.17 — M5 begins: `log:` finally writes something

**`log:` was the tool's own failure mode.** It was parsed, interpolated and validated — a
relative path rejected, a malformed `${date:…}` rejected — then *displayed by*
`config --show` as though it were in use, while nothing ever opened it. `cli.rs` opens by
declaring that "a flag that parses and then does nothing is the same silent no-op this
project is built to avoid". This was one, in the configuration surface, for fifteen versions.

`run` now appends a record per run. Verified end to end against real rustic:

```
2026-08-03T14:34:41-07:00 partial j on host-a
  backup   partial  backup saved 1 of 2 snapshot sets and then failed (exit 1); continuing
                    so retention still runs (0.308s)
    -> rustic -P …/p.toml backup --json --name good --name broken
  forget   success  forget succeeded (0.300s)
    -> rustic -P …/p.toml forget
```

and, on a backup that saved nothing:

```
  backup   failure  backup saved nothing (exit 1) (0.302s)
  forget   skipped  did not run, because an earlier operation stopped the job
```

**That second block is the point.** "Retention did not run" is the single most important
thing to be able to find afterwards, and an absence in a list does not say it. The log
states it, the same way the terminal report does.

Four decisions, each with a test:

- **Append, never truncate** (`O_APPEND`), so two runs racing on one file interleave whole
  writes. Rotation is `${date:…}` in the path, which is why it resolves per run.
- **`run` supplies a clock; nothing else does.** One `Zoned::now()` for the whole run, so
  the file `${date:…}` selects and the timestamp inside it cannot disagree across midnight.
  Inspection commands still leave `${date:…}` unresolved — baking today's date into a
  generated unit would freeze it at install time.
- **Plain text, no ANSI.** `report.rs` writes for a terminal; escape sequences are noise to
  `grep`. Rendered separately rather than stripped back out.
- **A failed write never changes the exit code.** The backup already happened; failing it
  over a log line would be a lie in the more dangerous direction, and a systemd unit would
  act on it. The warning goes to stderr, where the journal catches it.

The parent directory is created on demand, since `$XDG_STATE_HOME/rusticprofile` does not
exist on a fresh machine and failing a first scheduled run over a directory nobody was told
to create is a poor introduction.

Still to come in M5: a status file and `--json`.

### v0.1.16 — correct the log-path claim

`v0.1.15` moved logs to `$XDG_STATE_HOME` and justified it with a hazard stated in the past
tense: that the job "appended to a directory it was in the middle of backing up". **It never
did.**

`rusticprofile` writes no log file. `job.log` is parsed, validated and *displayed* by
`config --show`; nothing opens it. Log targets are **M5**, unimplemented. The proof is that
`~/.config/rusticprofile/logs` does not exist and never did.

The exclusion cited as evidence —

```toml
"!**/.config/rusticprofile/logs",
"!**/.config/resticprofile/logs",
```

— is real, but the line that was *earning its keep* is the **predecessor's**. resticprofile
does write logs, 904 KB of them. The rusticprofile line was pre-emptive.

**The change itself was right and stays.** The declared path was in the wrong XDG category
and would have written into a live backup source the moment M5 landed. Fixing it before any
log exists cost nothing and left nothing to migrate — which is the useful part of the error,
not a defence of it.

Corrected here rather than edited away, per the `v0.0.12` precedent: the mistake was reading
a config file's *intent* as evidence of behaviour, without checking whether the code that
would produce that behaviour had been written.

### v0.1.15 — logs are state, not configuration

**`${state_dir}` is new, and the shipped example's `log:` now uses it.**

Logs were written to `${config_dir}/logs/`, i.e. `$XDG_CONFIG_HOME/rusticprofile/logs`. The
XDG Base Directory specification names logs as the example of what **`XDG_STATE_HOME`** is
for — "data that should persist between restarts but is not important enough to be in
`XDG_DATA_HOME`". Config is for configuration.

**Part of this entry was wrong; see `v0.1.16`.** The hazard below was *prospective*, not
observed: rusticprofile writes no log file at all yet (that is M5), so it had never appended
to anything. The exclusion quoted is the **predecessor's** doing — resticprofile does write
logs, 904 KB of them, into `~/.config/resticprofile/logs`.

The path was still in the wrong XDG category and would have bitten the moment M5 landed, so
the change stands. Only the claim that it had already happened was untrue.

**Why it would have mattered.** On this fleet `~/.config` is a backup source, so a job
logging there appends to a directory it is in the middle of backing up — and the rustic
profile carries an exclusion for exactly that reason:

```toml
# Scheduled-run logs are written into .config while .config is being backed up.
"!**/.config/rusticprofile/logs",
```

That exclusion existed only because of the wrong location. `~/.local/state` is not a backup
source, so with the default log path the hazard does not arise and the exclusion is
unnecessary. It stays in the shipped rustic example as an illustration of the trap rather
than as a requirement.

`paths::user_state_dir()` resolves `$XDG_STATE_HOME/rusticprofile`, falling back to the local
data directory on platforms with no state directory — `dirs::state_dir()` is `None` on macOS
and Windows, which have no equivalent concept.

**Existing configurations are unaffected.** `${config_dir}` still resolves, and a `log:`
already pointing there keeps working; this changes what the *example* recommends and adds
the variable that makes the correct choice expressible.

### v0.1.14 — the completion check was interrogating the wrong shell

`0.1.13` replaced a recipe that *claimed* zsh completions were loaded with one that checks
`fpath`. The check used `zsh -c`, which is **non-interactive** and therefore sources neither
`.zshrc` nor anything it includes — so `fpath` there is the built-in default.

Result: it reported `NOT ACTIVE` on a machine where completion was working perfectly,
telling the user to add an `fpath+=` line they already had. **The mirror image of the bug it
replaced** — one claimed success without checking, the other claimed failure while checking
the wrong thing. Both told the reader something untrue with equal confidence.

`zsh -i -c` now. Measured on the same machine, same moment: the non-interactive `fpath` had
2 entries matching `site-functions` (the two system directories), the interactive one had 3
(plus the user's), and `whence -w _rusticprofile` reported `function`.

The lesson is the one this project keeps relearning in new costumes: **a check is only worth
what its oracle is worth.** `0.1.5` asked `hostname(1)` instead of the function the binary
uses; this asked a shell that never reads the config being tested.

### v0.1.13 — help where help belongs, and completions that actually load

Three complaints, three different causes.

**A bare `rusticprofile` printed a two-line error instead of help** — and the error had gone
false: *"rusticprofile cannot run jobs yet"*, written during M1 and still there long after
it could. `arg_required_else_help` now prints help at parse time, like every other CLI.
It still exits **2**, not 0: a silent success from a bare invocation is exactly what a
systemd unit or wrapper script would believe.

**`config` with no mode printed grammar rather than guidance:**

```
error: the following required arguments were not provided:
  <--check|--show|--example <WHAT>>
```

That names the *shape of the constraint*. It now prints the subcommand's own help, which
answers the question actually being asked — "what can `config` do?" The `ArgGroup` is kept,
so `--check --show` together is still rejected.

**Tab completion never worked in zsh, for a reason that had nothing to do with the
completion.** `just install-completions` wrote `_rusticprofile` into
`~/.local/share/zsh/site-functions` and printed *"zsh auto-loaded from …"*. **zsh reads no
user directory unless `fpath` names it**, and that one is not on `fpath` by default on any
distribution. The file was written, never read, and the recipe claimed success — so
`rusticprofile config --<tab>` produced nothing, with no indication why.

zsh is the odd one out: fish auto-loads `~/.config/fish/completions` and bash-completion 2.x
auto-loads `~/.local/share/bash-completion/completions`. zsh has **no standard user path**,
only conventions — `site-functions`, `~/.zfunc`, `~/.zsh/completions` — none of them
privileged.

`install-completions` now **checks `fpath` and says so** when the directory is not on it,
printing the exact `fpath+=(…)` line needed. It also notes that shell aliases do not inherit
completions, with the one-liner per shell (`compdef rp=rusticprofile` and friends) — the
`rp` alias was the reported symptom, though not the cause.

Two tests pin the new behaviour at parse time rather than by scraping output: a bare
invocation and a bare `config` both produce
`ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand`.

### v0.1.12 — `--as-host` stops reporting a defect it cannot see

`0.1.4` added a refusal for a `filter-hosts` that cannot match the host which will run the
job. Correct on the real host, and a **false alarm on every other one**: the check reads
*this* machine's `rustic.toml`, and §5.9 requires that file to differ per host. Under
`--as-host` it therefore compares a profile from one disk against a hostname from another,
and they disagree precisely when the simulation is doing its job.

```
$ rusticprofile config --check --as-host host-d      # before
error: 2 configuration errors:
  jobs.dot-files.profile: …/dot-files.toml scopes `forget` to `host-f`, which does not
  include this host (`host-d`) …
```

Nothing was wrong: on that host, chezmoi renders its own profile naming its own hostname.
But the noise made `--as-host` useless for the per-host gate inspection it exists for.

The check is now skipped while simulating — **and the skip is stated**, because passing
silently would report a clean bill of health for a machine whose rustic profile this process
has never seen:

```
$ rusticprofile config --check --as-host host-d      # after
ok: …/jobs.yaml
  host              host-d
  note:         simulated — `filter-hosts` was NOT checked
                    that check reads this machine's rustic profile, and `host-d` has its own
```

Two tests hold the line: one that simulating another host exits 0 *and* says the check was
skipped, and one that on the real host a non-matching `filter-hosts` is still an error. The
skip is scoped to simulation; the silent-retention bug it guards against has not come back.

Also removed from the live fleet config, though not from this repository: the
`dot-files-backup` and `dot-files-forget` jobs. They existed so ladder rungs 7 and 8 could
be taken separately, both are done, and a job named `dot-files-backup` that silently skips
retention is the exact failure this tool was written to prevent.

### v0.1.11 — prune returns to the prune host

**A standing safety rule is superseded, and it is worth saying why rather than quietly
editing it.**

`AGENTS.md` and §3 both said: *never `prune` against the shared repository until M4 lands*.
That was written before rustic's own documentation was read, on the inference that a tool
taking no repository lock must be unsafe under concurrent access. §7.6 corrected the
inference; the rule never followed.

The corrected pair of rules:

- **Never run `restic prune` against a repository any rustic client writes to.** restic
  deletes packs immediately, safe only because it holds an exclusive lock — and rustic
  neither takes nor honours that lock. Measured: 14 packs deleted mid-backup, repository
  then failing `restic check`.
- **`rustic prune` is safe, and is the only prune that may run here.** rustic is lock-free by
  design: prune *marks* packs and deletes them only after `--keep-delete`, 23 h by default.
  Verified — a default `rustic prune` left every pack on disk; only `--instant-delete`
  removed them.

**The cost of the wrong rule was real.** The fleet's prune schedule was disabled on
2026-08-02 and nothing has reclaimed space since, on reasoning that did not hold. M4 is
defence in depth, not permission.

So a prune job goes back on the designated host — as a **`rustic prune`**, never by
re-enabling the predecessor's `restic prune` timer, which remains correctly disabled. The
shipped `config --example jobs` already carried the right explanation; the live config
carried the old one and is corrected alongside this.

No code changed.

### v0.1.10 — units name rustic by absolute path

**A scheduled backup could not find `rustic` at all on a host where the shell finds it
fine.** Every run failed with `could not run rustic: No such file or directory`, on a unit
**byte-identical** to one working elsewhere.

The unit invokes rusticprofile by absolute path — deliberately, and the code says why:
*"systemd requires `ExecStart` to be absolute; a bare name is not resolved against `PATH`."*
That reasoning was never extended to the binary doing the actual work. rusticprofile then
spawned `rustic` by bare name, resolved against the **service manager's** `PATH`, which is
not the shell's.

**With `linger` enabled — and it must be, or backups only run while someone is logged in —
the user manager starts at boot** with the system default `PATH=/usr/local/bin:/usr/bin`.
No login shell, no `~/.zprofile`, no `~/.cargo/bin`. A cargo-installed rustic is invisible
to it.

Measured across two hosts, identical configs and identical units, neither containing a
`PATH` or `Environment` line:

| | rustic | service manager `PATH` | result |
|---|---|---|---|
| host with manager restarted from a login session | `~/.cargo/bin/rustic` | includes `~/.cargo/bin` | worked |
| host with manager started at boot | `~/.cargo/bin/rustic` | `/usr/local/bin:/usr/bin` | **every run failed** |

**The working host was the accident.** A reboot would have taken it too, with no change by
anyone — the kind of latent failure that looks like a regression months later.

`schedule` now resolves the configured rustic to an absolute path and writes it into
`ExecStart` as `--rustic-binary`. An absolute path in the config is taken as given but
checked for existence; a bare name is resolved against the interactive `PATH`, which is the
only place there is to look, and **baking in what the shell finds now is the entire point.**

**Consequence worth knowing: `schedule` now requires rustic to be installed.** Provisioning
in the order rusticprofile → config → schedule → rustic no longer works; schedule that host
after installing rustic. That is the right trade — there is no correct unit to write without
it, and the alternative is a unit that fails at 03:00 instead of an error now. CI has no
rustic, so the integration fixture names `/bin/sh`, which exists on every runner and is
never executed; weakening the resolver for tests would have removed the guarantee the test
exists to protect.

If it cannot be resolved, `schedule` **refuses and writes nothing**, naming `linger` as the
reason — moving the failure to a moment when someone is watching, instead of 03:00 on a red
unit nobody reads. A test asserts every `/`-containing token in `ExecStart` is rooted, so
the third path cannot quietly join the other two in depending on an environment the unit
does not control.

### v0.1.9 — second release: everything since v0.1.0

**Tagged, released and published.** `v0.1.0` was the only prior release, so this carries
eight versions of work. Entries `0.1.1`–`0.1.8` below are the detail.

#### Upgrading from 0.1.0 — one breaking change

**`rusticprofile schedule --enable` no longer exists.** `schedule` now arms the timer by
default, and `--write-only` is the opt-out:

```bash
rusticprofile schedule -n dot-files                # was: schedule -n dot-files --enable
rusticprofile schedule -n dot-files --write-only   # was: schedule -n dot-files
```

A script still passing `--enable` gets clap's `unexpected argument` and stops. That is
deliberate — the alternative was accepting a flag that silently does nothing, which is the
failure class this tool exists to prevent — but it is the one thing here that can break an
existing setup, so it belongs at the top rather than eight entries down. Full reasoning in
`0.1.7`.

Everything else is additive or internal.

#### The rest, briefly

- **A new load-time refusal:** a `filter-hosts` that cannot match the host it will run on
  (`0.1.4`). rustic matches that field exactly, so a filter naming anything else selects
  nothing and retention silently never runs. Found because a generated config had exactly
  that defect — `.chezmoi.hostname` is the name up to the first `.`, which matches no
  snapshot on a `*.local` host.
- **`config --example jobs|rustic`** — annotated starting-point configurations, tested
  through the real binary so they cannot drift from the validator (`0.1.1`).
- **The service unit is now `static`**, so it cannot be enabled independently and run a
  backup at every login (`0.1.7`).
- **`schedule` refuses on platforms without systemd** rather than writing units nothing will
  run — verified on a real macOS CI runner (`v0.1.0`, restated here because it is what makes
  the macOS binary honest).
- **AUR packaging** under `packaging/aur/`, with `just aur-verify` building and linting it in
  a container (`0.1.1`, `0.1.2`, `0.1.6`).
- **Documentation corrections that change operational advice**: `PLAN.md` §7.6 was
  overstated and is corrected in `0.1.3` — rustic's lock-free design is sound, and the hazard
  is specifically `restic prune` against a rustic writer.

### v0.1.8 — the automatic Claude review is off

`claude-code-review.yml` no longer runs on pull requests. It is `workflow_dispatch` only:

```bash
gh workflow run claude-code-review.yml -f pr=<number>
```

**Why.** The token behind it stopped working mid-session — **19 consecutive green runs,
then every run failing in ~490 ms on turn 1 at $0.00**, posting no findings. A rejection
before any tokens were billed is a credential or quota problem, not a verdict on any code;
re-running it produced an identical failure, so it was not transient.

Left as-is that is a **red check on every future PR for a reason unrelated to the PR**,
which trains everyone to merge over a failing check. That is worse than having no automated
review, and this repository has already spent effort on the opposite problem — see v0.0.11
and v0.0.14, where the same job went *green* without reviewing. A check that cannot be
believed in either direction is not a check.

**The `pull_request:` trigger is kept, commented, directly beneath the replacement**, with
the diagnostic path in the header comment: `gh secret list` for when
`CLAUDE_CODE_OAUTH_TOKEN` was last set, `claude setup-token` to mint a new one. Restoring
automatic review is uncommenting two lines.

**`claude.yml` is untouched but shares the secret**, so `@claude` mentions fail the same way
until the token is replaced. That is noted in the workflow header rather than left to be
rediscovered.

The `prompt:` now reads `inputs.pr` instead of `github.event.pull_request.number`, which
does not exist on a dispatch event and would have silently produced a review of
`repository/pull/` — the empty-value failure this project keeps running into.

### v0.1.7 — `schedule` is one step, and the service unit is `static`

**`--enable` is gone and `schedule` now arms the timer by default.** Both changes came from
a question about why rusticprofile needs a timer *and* a service.

**It does not — and neither does the predecessor.** systemd has no way for a `.timer` to run
a command; a timer exists only to activate a `.service`. Every tool that schedules with
systemd installs two units per job, resticprofile included — `resticprofile-backup@profile-*.service`
and `.timer` sit side by side on the same machines. That part was a misconception, not a
defect.

But asking the question surfaced two real things.

**Our service unit was wrong, and the predecessor's was right.** Ours carried
`[Install] WantedBy=default.target`, so systemd reported it as `disabled` rather than
`static` — which reads as an invitation. `systemctl --user enable rusticprofile-<job>.service`
would have run a backup at every login, forever, with no timer involved and no schedule to
explain it. The `[Install]` section is gone; only the timer has one now, and systemd reports
the service as `static`: activatable by its timer and by nothing else.

**`schedule` was two steps while `unschedule` was one.** `unschedule` already disables,
removes and reloads in a single command. `schedule` wrote inert units unless given
`--enable`. That asymmetry was a migration guard — "don't quietly add a second writer to a
shared repository" — and the reasoning does not survive: running `schedule` *is* deliberate,
it is not a side effect, and **a command that reports success while scheduling nothing is the
silent no-op this project exists to prevent.**

So `schedule` arms the timer, and `--write-only` keeps the inspect-first path. `--enable` is
removed rather than accepted-and-ignored: clap's `unexpected argument` is loud, and a flag
that silently does nothing is exactly the failure mode being fixed. Writing to a custom
`--unit-dir` still never arms anything — that is an inspection target, not a place systemd
reads.

Stays in the `0.1.x` chain, and **§3's versioning rule is rewritten to say so.** It claimed
the sibling repos' "minor for features" convention resumed after `v0.1.0`, which would have
made this `0.2.0`. It does not apply here: every PR from `v0.1.0` to `v1.0.0` is a patch
bump, and a minor bump is reserved for a broken library API or a landed milestone. This
release is the precedent — a flag *removed* from a published crate, still a patch — because
nothing links against rusticprofile and a removed flag fails loudly at parse time rather
than quietly changing what a command does.

202 tests, up from 198.

### v0.1.6 — the AUR recipes were breaking Syncthing

**Tooling only; no code changed. The bug was outside the repository, which is why nothing
here could have caught it.**

`aur-verify` and `aur-srcinfo` mounted `packaging/aur` with podman's **`:Z`**. Uppercase `Z`
assigns a *fresh private MCS category pair per container run* and relabels the mount in
place, so after `just aur-verify` the directory and its three files were left as
`container_file_t:s0:c238,c656` — categories no other container holds.

This repository lives under a Syncthing folder. Syncthing's own container runs as
`container_t:s0:c337,c880`, so it could no longer read that directory: `scan: open
…/packaging/aur: permission denied`, one permanently unsyncable subtree, re-created on every
run of the recipe. Unix permissions were untouched and looked perfectly normal
(`drwxr-xr-x`), so only `ls -Z` showed anything.

**The way it presented is the part worth keeping.** The folder reported `state: idle` and
`needBytes: 0` — indistinguishable from healthy. The failure appeared only as
`"errors": 1, "pullErrors": 1` in `/rest/db/status`, and the *reason* only at
`/rest/folder/errors`. A tool built around the idea that silent degradation is the failure
class that matters had shipped one in its own packaging recipes.

**Fixed to lowercase `:z`** — the shared `container_file_t:s0` label, no categories.
Lowercase rather than dropping the flag: this repository is public, and on a machine without
an fcontext rule pinning the tree to `container_file_t`, `$HOME` is `user_home_t`, which
`container_t` cannot read at all. `z` is correct everywhere; no flag is correct only here.

### v0.1.5 — fix a fixture that lied on CI, and stop `merge-pr` merging red

**Two failures, and the second one is why the first reached `main`.**

**The fixture.** v0.1.4 made the integration fixture substitute the real hostname into
`filter-hosts`, because `schedule` and `run` resolve the host themselves and have no
`--as-host`. It did that by shelling out to `hostname(1)` with a `"localhost"` fallback.
The Fedora CI container **has no `hostname` binary**, so the fallback was used while the
binary under test resolved the container id — and the fixture then disagreed with the
program it was testing on every containerised runner. It now calls
`config::hosts::current_hostname()`, the same function the binary uses. Asking a different
oracle than the code under test is the whole defect; the test only passed locally because
both happened to agree there.

**`merge-pr` merged over a red check.** `gh pr merge` will merge a failing PR when the
repository has no branch protection, and #19 went in with `build (fedora-x64)` red. The
proximate cause was mine — "wait for the checks to settle" is not "wait for them to pass",
and I checked the result after merging rather than before. But nothing in the tooling
stopped it, and `merge-pr` is the one recipe that merges, so that is where the gate belongs
— the same argument that puts the pre-PR gate in `open-pr`.

`just merge-pr` now refuses on any `FAILURE`/`TIMED_OUT`/`CANCELLED`/`ACTION_REQUIRED`, and
refuses while checks are still running rather than racing them. It names the failing legs.
Merging a red PR deliberately still works — via `gh` directly, which is the right amount of
friction for something that should be a conscious act.

### v0.1.4 — refuse a `filter-hosts` that cannot match this host

**Found by asking whether the fleet rollout was ready. It was not, and the reason was a bug
in the chezmoi template written the same day.**

`.chezmoi.hostname` is the hostname *up to the first `.`*. Templated into `filter-hosts` it
renders `["foo"]` on a machine whose snapshots rustic records as `foo.local` — and rustic
matches that field **exactly**, so the filter selects nothing, `forget` deletes nothing, and
retention silently never runs while every command reports success. That is bug #1 from
`PLAN.md` §2.1, reintroduced.

It was invisible on the host it was written on: `host-f` has no domain suffix, so
`.chezmoi.hostname` and `.chezmoi.fqdnHostname` are identical there. Two of the seven hosts
are `*.local`, and neither is rolled out yet — the template would have failed only on the
machines nobody was looking at.

**The template is fixed** (`.chezmoi.fqdnHostname`, with a comment saying why). **More
importantly, so is the reason it was possible.** `check_filter_hosts_can_match` refuses at
load time any profile whose `filter-hosts` does not include the host that will run it:

```
jobs.j.profile: …/p.toml scopes `forget` to `host-e`, which does not include this host
  (`host-e.local`) — that looks like a short hostname where the full one is needed; rustic
  matches the recorded name exactly, so `chezmoi`'s `.chezmoi.hostname` must become
  `.chezmoi.fqdnHostname` here. …retention would silently never run
```

The short-form hint is only emitted when the configured name is genuinely a prefix, so an
unrelated hostname is not misdiagnosed as a chezmoi problem.

**Matching here is deliberately exact, unlike `enabled-on-hosts`.** rusticprofile's own host
gate accepts a short name for a dotted host; `filter-hosts` is matched by *rustic*, which is
not lenient. Being generous would defeat the check. That asymmetry is now asserted by a test
and spelled out in the fixture that exposed it.

**A previously green check was hiding this class entirely.** `check_forget_is_scoped` asks
whether a scoping filter is *present*. It never asked whether it could *match*. A filter
naming a host that does not exist passed validation with exit 0.

**Two test fixtures had to learn the difference**, which is itself the evidence the rule
bites: `schedule` and `run` resolve the hostname themselves and have no `--as-host`, so their
integration fixture now substitutes the real hostname into `filter-hosts`.

### v0.1.3 — correct the lock finding: rustic is not deficient, the mixture is

**v0.0.22 and the `0.1.0` README overstated §7.6, and the overstatement caused a wrong
operational decision.** Corrected in `PLAN.md` §7.6, the README and the shipped example.

The original finding — that rustic writes no repository lock object and is therefore
invisible to restic's mutual exclusion — is **true**, and the measured corruption
(`restic prune` deleting 14 packs from under an in-flight rustic backup, repository failing
`restic check`) is real. What was wrong is the conclusion drawn from it: that rustic is
unsafe under concurrent access, that prune must not run from any tool, and that M4 was the
precondition for prune ever returning.

rustic's FAQ says the opposite, and it is right:

> "Yes, all operations are designed lock-free. This means all commands can run parallel."

The mechanism is **two-phase deletion**. `prune` marks packs and removes them only after
`--keep-delete`, default **23 hours**, so a concurrent backup referencing a marked pack has a
day of grace. `--instant-delete` is the documented opt-out. Verified on a throwaway
repository with three unreferenced packs: default `rustic prune` reported `to delete: 3
packs` and left **all three on disk**; `--instant-delete` removed them.

So the hazard is one row of a four-row matrix — `restic prune` against a rustic writer —
not a property of rustic.

**The operational consequence is the point.** "Nothing reclaims space until M4" was wrong,
and it disabled a prune schedule for no reason. **Finishing the migration is the fix:** once
every host writes with rustic, prune is safe by rustic's own design and the prune host's
schedule returns as a `rustic prune`. M4 becomes defence in depth rather than permission.

**The lesson, recorded rather than tidied away:** *"tool X lacks the mechanism I expected" is
not "tool X is unsafe"*, and the gap between them was a day of unnecessary exposure plus a
disabled schedule. The measurement was sound; reading the tool's own design documentation
before generalising from it was the missing step.

### v0.1.2 — AUR recipes, and a bug they found

Publishing to the AUR was done by hand — a long `podman run` typed at a prompt — which is
exactly the shape of thing this repo turns into a `just` recipe. Four now exist:

| recipe | |
|---|---|
| `just aur-verify` | build + `namcap` in `archlinux:base-devel`; no Arch install needed |
| `just aur-srcinfo` | regenerate `.SRCINFO` from the `PKGBUILD` — it is derived data, never hand-written |
| `just aur-bump VERSION` | set `pkgver`, reset `pkgrel`, refresh `sha256sums` from the real tarball |
| `just aur-publish` | verify, confirm, clone, push |

**`aur-verify` installs `rustic` in the container on purpose**, which is the whole reason it
is a recipe rather than a one-liner: without it the build still passes while the
rustic-backed integration tests skip themselves, so a green `makepkg` proves much less than
it appears to. That was learned the hard way once; a recipe is how it stays learned.

**`aur-publish` refuses rather than failing obscurely.** It checks `.SRCINFO` agrees with the
`PKGBUILD`, re-downloads the release tarball and compares its sha256 against `sha256sums` —
the classic AUR breakage is a bumped `pkgver` with a stale checksum, which fails on the
user's machine and nowhere else — and reports an AUR maintenance window in the AUR's own
words instead of letting `git clone` fail. It takes the same three-way confirmation as the
pre-PR gate (`AUR_CONFIRM`, a terminal, or piped input), so it never blocks a script.

**Running the recipe found a bug the hand-typed version had hidden.** `makepkg` also emits a
`-debug` package, so `./*.pkg.tar.zst` expands to two paths; `tar` reads the second as a
member name and fails with *"Not found in archive"*. Because that sat inside a pipeline the
failure was swallowed, and the recipe printed a success tick over a payload listing that had
never been produced. Now the package is selected explicitly, and the listing is checked
outside a pipeline. Exactly the failure class this project exists to prevent, in its own
tooling.

**Also fixed: `just --list` was showing the wrong description for `open-pr`.** `just` takes
the last contiguous comment block above a recipe as its doc comment, so the usage examples
added in v0.1.0 displaced the real description. Moved inside the recipe body.

The AUR push itself is still outstanding — the AUR has been in a maintenance window
throughout. `just aur-publish` is what completes it.

### v0.1.1 — published, and AUR packaging

**`v0.1.0` is released.** GitHub release with binaries for linux-x86_64, linux-arm64 and
macos-arm64 plus the man page; **crates.io** has `rusticprofile 0.1.0`, so
`cargo install rusticprofile` works. Both `rusticprofile` and `rustic-profile` were free;
the plain name was taken.

**AUR packaging under `packaging/aur/`** — `PKGBUILD`, `.SRCINFO` and the workflow to verify
a change. It is not pushed to the AUR yet: the AUR was down for maintenance
(*"The AUR is down due to maintenance"*) at the time. The name is free (`resultcount: 0`).

**The package is verified, not merely written.** Built end to end in an `archlinux:base-devel`
container: `makepkg` completed, `check()` ran **187 unit and 38 integration tests against
real rustic 0.11.3**, and the payload is exactly the binary, three shell completions, the
man page, README and LICENSE. `namcap PKGBUILD` is clean.

One detail worth keeping: **install `rustic` in the verification container.** Without it the
build still passes, but the rustic-backed integration tests skip themselves with a printed
notice — so the build proves considerably less than it appears to. The first run here made
exactly that mistake.

`depends=('rustic')` is the dependency that matters most and the one `namcap` cannot see:
it reads linked libraries, and rusticprofile links nothing against rustic — it *spawns* it.
The warning is expected and the dependency is correct.

`pkgver` tracks the **released** version, not `Cargo.toml` on `main`. The repository moves on
after a release; the package does not.

### v0.1.0 — the first release

**The version policy's own trigger fired seventeen versions ago and nobody noticed.** §3
reserves `v0.1.0` for "the first version that can actually run a backup end to end" — which
was **v0.0.7**, when Milestone 1 completed. M2 has since landed too. So this is not a
promotion, it is a correction: the tool has been past `0.1.0` by its own definition for some
time, and the version was understating it rather than overstating it.

Two things were fixed first, because both were the project's own failure class turned
inward.

**The README advertised two features that do not exist.** It listed "systemd units and
launchd agents" and "lock coordination against a repository shared by several machines"
under *what it does*. launchd is M3 and unwritten; `lock::budget()` returns `None`. The
second one mattered most — the front page promised fleet lock coordination to people who
would install this and point it at a shared repository, and §7.6 had just established that
**rustic takes no repository lock at all**. Both moved to a *What it does not do yet*
section, with the measured prune-corruption result quoted as a warning. A backup tool's
README is a safety surface, and an aspirational feature list is a way to lose data.

**`schedule` had no platform guard.** `unit_dir()` returns `~/.config/systemd/user`
unconditionally, so on macOS the command wrote systemd units into a directory launchd never
reads, printed the files it created, and exited 0 — files on disk plus a success message,
which is indistinguishable from a working install. That is precisely the silent degradation
this project exists to prevent, shipped inside the tool that opposes it.

`schedule` now refuses on any non-systemd platform, before the config is even read, naming
the milestone rather than saying "unsupported" — the reader needs "not yet", not "never" —
and saying what still works. `status` instead *reports* the platform, because "nothing is
scheduled here, and here is why" is a truthful answer rather than a failure. The check is
runtime (`cfg!`) rather than `#[cfg]`, so unit generation stays testable everywhere; CI runs
`cargo test` on macOS, so the integration test verifies the real behaviour on both sides
rather than asserting one.

225 tests, up from 221.

### v0.0.24 — refuse source paths rustic will not expand

**One `rustic.toml` cannot be shared across hosts, and finding out why turned up a new
silent-destruction path.** `PLAN.md` §5.9 has the measurements.

rustic 0.11.3 expands neither `~` nor `$VAR` in `sources`, and there is no env-var route to
the host filter. That much is merely inconvenient. What matters is how it fails: `~/…` and
`$HOME/…` are *relative* paths to rustic, so they miss the §5.7 hard-fail that an absent
*absolute* path gets. Instead rustic warns, backs up nothing, **saves the snapshot and
exits 0** — leaving a real 0-byte snapshot whose recorded path is the literal string
`$HOME/Sync`. Under the label grouping §7.3 requires, that empty snapshot then wins its
retention slot against the real one, which is the mechanism that already destroyed a
395 MiB snapshot. A "portable" config of this shape does not fail to back up; it replaces
the backups with nothing and reports success.

`filter-hosts = ["$HOSTNAME"]` fails the other way — matches nothing, so retention silently
never runs. That is bug #1 from §2.1, reproduced exactly.

**New refusal: `check_sources_are_expanded`.** Any `sources` entry containing `~` or `$`,
on a job that backs up, is a load-time error naming the set, the path and the consequence.
Batched with every other rule. `forget`-only jobs are exempt — a profile is shared, and a
forget never reads `sources`.

This is the same bargain as the `--name` check: rusticprofile reads a file it does not own,
because nothing else in the chain can catch the mistake and the mistake is invisible.

**Rejected: emitting `--filter-host <hostname>`.** It would remove the need to template the
hostname, and it is the wrong trade — `-P`, the operation, `--name` and `--json` are the
only flags this tool emits, a test asserts it, and the home path would need templating
regardless.

**The useful half of the answer:** `jobs.yaml` needs no templating at all. `${env:HOME}` is
resolved by rusticprofile itself and `enabled-on-hosts` is a host list that reads the same
everywhere, so one byte-identical file serves all seven hosts — prune-host gate included.
Only `rustic.toml` has to be generated. 221 tests, up from 213.

### v0.0.23 — `config --example`: ship the findings as a config

`config --example <jobs|rustic>` writes an annotated starting-point configuration to stdout.

**The `rustic` one is the point.** The delegation boundary means rusticprofile owns almost
nothing, so nearly everything that can silently destroy data lives in rustic's config — and
until now all of it existed only as prose in `PLAN.md` and one hand-written file on one
machine. The example carries, with the measurement behind each: `opendal:gcs` rather than
restic's non-existent `gs:`; scoping filters in `[snapshot-filter]`, where rustic actually
reads them, rather than `[forget]`, where it accepts and ignores them; `group-by =
"host,label"`, without which a 0-byte snapshot evicts a 395 MiB one; exclusion globs with
their leading `!`, since a bare pattern is an *include* filter; snapshot sets split because
rustic hard-fails an entire set when one source is missing; and the password file and
credentials excluded, or the key goes inside the lock.

**Emitted to stdout, never written** — the same decision as `--completions`, and for a
sharper reason: the file this would otherwise overwrite is what stands between a fleet and
its backups. It requires no existing configuration and reads nothing, since an example is
what you want when you have neither file yet.

**Placeholders are static.** `host-a`, `/home/user` — this project's own redaction
vocabulary. Filling in the real hostname and `$HOME` would produce something that runs
as-is, which is exactly the objection: a config that appears to work is one nobody reads.
It also keeps the feature clearly outside the "no templating in any form" non-goal — there
is no template, no expression language, and nothing is substituted.

**The examples are tested through the real binary**, not just asserted to parse: both are
emitted, written out with only the profile directory redirected, and then put through
`config --check` and `plan --format lines`. An example that has drifted out of step with
the validator is worse than none, because it is quoted with authority. 213 tests, up from
203.

Design and the full guidance table in `PLAN.md` §7.7.

### v0.0.22 — rustic takes no repository lock, and the cutover made that matter

Documentation and findings only; no code changed.

**`PLAN.md` §7.6 — exactly one lock authority per repository, and every writer must speak the
same protocol.** A restic lock is an object *inside* the repository, under `locks/`, so on
object storage every machine sharing it sees the same key — that is what makes restic's
mutual exclusion work across a fleet. rustic 0.11.3 writes no such object and checks for
none, so it is invisible to it.

Measured in a throwaway repository rather than argued from documentation: with a rustic
backup frozen mid-write, `restic prune` did not refuse. It deleted 14 packs (487.780 MiB) as
unreferenced, and `restic check --read-data` afterwards reported *"The repository is damaged
and must be repaired"* with five data packs missing. The control — the same prune against a
held restic lock — refused, naming the holder.

Nothing about the prune host changed. Before the cutover this host ran restic and its lock
was seen; the cutover replaced the writer with one that does not speak the protocol. §7.5's
retention crossfire cost snapshots whose data survived, because `prune: false` left the packs
behind. This one deletes the packs.

**M4 is reclassified from prospective to load-bearing.** The designated prune host's timer is
disabled, so nothing reclaims space until M4 lands — and the exclusion M4 implements has to be
written in restic's own `locks/` format, because coordinating only rusticprofile instances
would leave the predecessor and hand-run `restic` outside it.

**A methodological correction worth more than the finding.** The first control test used a
second `restic backup` and expected a refusal. It succeeded, because backups take a *shared*
lock — and briefly looked like evidence that restic does not lock either. Exclusion has to be
tested with an operation that actually excludes. Separately, the prune schedule is
hostname-gated in the predecessor's config, so its absence on the machine in front of you
proves nothing about the fleet; a claim that "nothing prunes this repository" was made on that
evidence the day before and was wrong.

`doctor` (backlog) gains a second check to go with §7.5's: a repository written by rustic
while any restic schedule still runs an exclusive operation. It is the more dangerous of the
two.

### v0.0.21 — the pre-PR gate no longer needs a terminal

Tooling only; no code changed.

`just pr` ended its checklist with a bare `read`, which is fine for a human and wrong for
everything else. Called from a script, a CI step or an agent shell, it either blocked on a
stdin that would never answer or died without printing why — the failure looked like the
gate itself breaking rather than the gate asking a question nobody could hear.

The checklist now takes its answer from `PR_CONFIRM` if set, from an interactive stdin if
there is one, and otherwise from whatever was piped in under a ten-second bound. All four
paths still require an explicit `y`: `PR_CONFIRM=n` and a piped `n` both refuse, so this
widens *who can answer*, not *what counts as an answer*. With nothing to read from, it now
fails in milliseconds and names `PR_CONFIRM` in the message — a gate that cannot be
satisfied from the context it failed in is a wall, not a gate.

`open-pr` also hands `gh` an explicitly empty stdin when there is no terminal. It would
otherwise inherit the pipe the checklist just drained, and `gh` reads stdin itself for
`--body-file -`; failing there loses a gate that had already passed.

The interactive path is unchanged, including `gh pr create` with no arguments.

### v0.0.20 — first host cut over, and a retention hazard from outside the tool

Documentation and findings only; no code changed.

**rusticprofile is now the scheduler for one host.** `schedule --enable` armed the hourly
timer, the predecessor's timer for the same job was disabled, and a triggered run completed
`Result=success` in 6.8 s wall clock: 3 of 3 snapshot sets saved, `forget` retiring exactly
the two snapshots that had aged past `keep-hourly`. First time the tool has been the thing
actually taking backups anywhere.

**`PLAN.md` §7.5 — exactly one retention authority per (repository, host).** Enabling the
timer alongside the predecessor would have destroyed a real snapshot every hour, and the
same thing had already happened twice by hand that afternoon. §7.3's rule — group named sets
by label — protects the sets from each other; it says nothing about a second tool applying
its own retention to the same host. The predecessor's `group-by: host` with `path` and `tag`
off swept our labelled snapshots into its bucket and kept the newest per hour, deleting a
395 MiB `core` snapshot in favour of a 0-byte one written one second later. Our own
correctly grouped `forget` deleted one of *its* 397 MiB snapshots by the mirror-image
mechanism, since an unlabelled foreign snapshot is just another member of the empty-label
group. Two defensible configurations, no overlap in intent, snapshots lost in both
directions.

The operational consequence is an ordering: disable the outgoing tool's retention *before*
enabling the incoming tool's schedule, and confirm from the repository rather than from
either tool's own report. rusticprofile cannot detect this itself — it emits no retention
flags by design — so a `doctor` command is now in the backlog.

**Part 8 corrected.** It claimed the predecessor was authoritative on all seven hosts, which
stopped being true with this cutover, and listed three open upstream PRs that ceased to
exist when the fork was deleted.

### v0.0.19 — M2 complete: schedule, unschedule, status

**Milestone 2 is done. rusticprofile can now schedule itself.**

- `schedule [-n JOB] [--enable]`, `unschedule -n JOB`, `status`. Verified end to end on this
  host: units installed into `~/.config/systemd/user`, `systemctl --user list-unit-files`
  reporting both as `disabled`, and `status` agreeing with systemd's own view.
- **Writing units is not activating them.** `schedule` installs and reloads; `--enable` is a
  separate, explicit flag. On a fleet where the Go tool is still taking the backups, quietly
  adding a second hourly writer to a shared repository is not something a command called
  `schedule` should do as a side effect.
- **Idempotent.** Identical content is not rewritten, so re-running reports `unchanged`
  rather than implying work happened. `unschedule` is safe to repeat and on a host where the
  job was never scheduled — that is the desired end state either way.
- **`status` surfaces the host gate**, so "this host has no prune timer" reads as a decision
  rather than an absence — and distinguishes *not enabled* from *could not tell*, since only
  one of those means the schedule is off.
- **Fixed a real bug found by using it:** the tool panicked on a broken pipe, so
  `rusticprofile status | head` greeted the user with a Rust panic and a backtrace hint. Rust
  ignores `SIGPIPE`, so print macros panic on a closed pipe. The default disposition is now
  restored at startup and the process dies quietly like every other Unix tool.

### v0.0.18 — M2 begins: systemd unit generation

- `schedule/calendar.rs` and `schedule/systemd.rs`. **Pure functions — nothing is written,
  no unit installed, `systemctl` never consulted** — so a unit can be inspected before it
  exists anywhere, the same discipline as `plan` showing an argv before spawning.
- **Validated against real systemd**, not just unit-tested: `systemd-analyze verify` parses
  both generated units with no complaint other than the man page not being installed on this
  machine. Every directive emitted — `Persistent`, `RandomizedDelaySec`, `OnCalendar`,
  `Nice`, `IOSchedulingClass`, `Type=oneshot` — confirmed accepted.
- **No unit ever contains a date.** This is why `${date:…}` is left unresolved at load time:
  a unit written today must not log to today's file forever. The unit carries no log path at
  all and the runner resolves it per run, with a test asserting neither a resolved year nor a
  stray `${date:` can appear.
- **`Persistent=true`** — laptops are asleep at 03:00, and without it a missed run is simply
  skipped, which for an intermittently-online fleet means the schedule quietly does nothing.
  Exactly the class of silent non-event this project exists to prevent.
- **`RandomizedDelaySec`, scaled per interval** — seven machines share one repository and
  would otherwise wake on the same instant. Tested to stay inside its own period, so an
  hourly job never drifts into its successor.
- **Priority lives in the unit**, so no `nice`/`ionice` code is ever written in Rust.
  `Standard` emits nothing rather than `Nice=0`, leaving a deliberate system default alone.

Still to come in M2: `schedule` / `unschedule` / `status`, which is the part that touches
the filesystem and `systemctl`.

### v0.0.17 — verification ladder rungs 7 and 8, and a design finding

**The tool has now backed up to, and forgotten from, the real shared repository.** Ladder
rungs 7 and 8 are done; only rung 9 (fleet rollout) remains, and that is gated on
scheduling rather than on more verification. Full detail in `PLAN.md` §7.3–7.4.

- **Rung 7** — three sets saved, counts +3 on both this host and the repository total, so
  the write was additive and no other host was affected. Exclusions verified against the
  *stored* snapshot, with positive controls so the check could fail.
- **Rung 8** — real `forget`, prune disabled: removed exactly the three snapshots predicted,
  **pack count unchanged**, every other host untouched. `[snapshot-filter]` held under a
  real irreversible operation.
- **Design finding: option B needs label-based retention grouping.** With `group-by = "host"`
  the named snapshot sets compete for one retention slot, so only the last one written each
  period survives. A dry run kept a **0-byte** `nushell` snapshot and deleted the
  **6,256-file** `core` one, and reported success. Fixed with a stable `label` per set and
  `group-by = "host,label"` — label rather than paths, because paths fragment on rename.
- **A near-miss, kept because it validated a guard.** A scripted edit matched `[forget]`
  inside a comment and deleted the `[snapshot-filter]` section. Reconstructing the damage so
  it parses and running `config --check` produces a refusal naming the missing filter — the
  M1 invariant catches exactly this.

**The configuration is not portable and is not chezmoi-managed.** It exists on one machine,
with the hostname and eight absolute paths hard-coded. Templating it is a prerequisite for
rung 9, not an afterthought.

### v0.0.16 — CI timing, and the review workflow verified end to end

- **Pull-request CI is now 83 seconds wall clock**, from roughly 19 minutes at its worst.
  The two contributors were `cargo install cargo-audit` (v0.0.8) and the `fedora-arm` leg
  (v0.0.15); with both addressed, `Rust` at 77s is the critical path.
- **The review workflow's reporting is verified in both directions.** PR #5 exercised the
  skip path ("The review did NOT run"), and PR #6 the working path ("The review ran and
  completed — 5 turns, $0.18"). Neither was assumed from a green check.
- **Corrected while doing so:** the validation skip applies when *the Claude workflow's own
  file* differs from the default branch, not when any workflow file is touched. PR #6 edited
  `rust.yml` and was reviewed normally. `NOTES.md` and the workflow comment already scoped
  this correctly to `claude*.yml`; a PR description did not.

### v0.0.15 — drop fedora-arm from pull-request CI

- `build (fedora-arm)` took **19 minutes** on one merge, essentially all of it `dnf install`
  plus a from-scratch rustup inside a Fedora container on an ARM runner. It set the wall
  clock for every pull request.
- Removed from the PR-time `build` job **only**. It still runs on every tag and manual
  dispatch via `full-test`, and `build-release` still produces the linux-arm64 artifact from
  it — so nothing ships untested.
- What it uniquely covered was the *combination* Fedora + aarch64. `fedora-x64` covers the
  Fedora toolchain, `ubuntu-arm` covers aarch64, and the only platform-specific code here is
  a handful of `nix` syscalls, so an independent failure of just that intersection is
  unlikely enough to be worth catching at tag time rather than on every PR.

### v0.0.14 — make the review outcome visible, properly

- The v0.0.12 fix did not work. `use_sticky_comment` controls *how many* comments are used,
  not *whether* one is posted — confirmed by reading the action's `action.yml`, which is
  what should have happened before configuring it. PR #4 proved it: the review ran with
  sticky enabled (6 turns, `is_error: false`, $0.18) and still posted nothing.
- The actual fix is a step that writes the outcome to `$GITHUB_STEP_SUMMARY`, one click
  from the check mark, distinguishing the three cases that previously all looked identical:
  the review ran and found nothing; the review ran and errored; the review never ran
  because workflow validation skipped it.
- It needs **no extra permissions**, so the review job stays read-only.
- The summary is `tee`d to the job log as well as `$GITHUB_STEP_SUMMARY`. Written only to
  the summary it is visible in the Actions UI and nowhere else — meaning the one thing that
  reports whether a review happened could not itself be checked from the logs or the API.
- The parser is defensive by design — the execution output has changed shape before. It
  reports "ran, shape unrecognised" rather than asserting a turn count that is not there.
  All four shapes and both shell branches were exercised locally before committing.

### v0.0.13 — correct the control-group definition

- `AGENTS.md` listed **four** idle hosts as the control group, including `host-f`.
  `PLAN.md`'s fleet table shows `host-f` as active and identifies it as the development
  machine — the one every throwaway repository and test run this session was created from.
- A control group containing the machine being experimented on is not a control group, so
  this is a defect in a safety rule rather than a typo. There are **three** idle hosts.
- Spotted while redacting the identifiers, where the two documents sat side by side.

### v0.0.12 — make the review visible when it finds nothing

- **This entry was wrong; see v0.0.14.** `use_sticky_comment` means "use just one comment
  to deliver PR comments" — it does not make the action post when it has nothing to say, so
  it did not fix the problem described below. Left here rather than rewritten, because the
  mistake is the useful part: the option was configured without reading `action.yml`.
- `use_sticky_comment: true` on the review job. Without it the action posts **only** when
  it has findings, which makes "reviewed and found nothing" and "skipped without running"
  produce identical evidence: a green check and silence.
- That is not hypothetical. On PR #2 a full review ran — 11 turns, `is_error: false`,
  $0.36 — and posted nothing, because a docs change had nothing to flag. Correct behaviour,
  invisible outcome.
- A sticky comment is updated in place rather than accumulating, so this costs one comment
  per pull request and turns silence back into evidence.
- The two ways a review can produce nothing are now both documented in the workflow file
  itself, where someone changing it will actually read them.

### v0.0.11 — record how the review workflow can silently skip

- **A green "Claude Code Review" check does not prove a review happened.** The action
  validates that the workflow file in a pull request is byte-identical to the copy on the
  default branch, and **skips with a `success` conclusion** when it is not. Observed on the
  very PR that added the workflows: the job went green, posted nothing, and logged
  *"Action skipped due to workflow validation"*.
- The validation is a security control, not a bug: it stops a pull request that rewrites
  the workflow from running that rewritten version with the repository's secret. It is
  working as intended, and it is exactly why adding these workflows could not review itself.
- **The practical consequence:** any PR touching `.github/workflows/claude*.yml` gets no
  review, silently. When changing them, read the job log rather than the check mark — look
  for `Attempt 1 failed: Workflow validation failed`.

### v0.0.10 — Claude review workflows

- `claude-code-review.yml` reviews every pull request; `claude.yml` responds to an
  `@claude` mention. Both authenticate with `CLAUDE_CODE_OAUTH_TOKEN`.
- **Safe on a public repository:** the action itself only runs for users with write access,
  so a passer-by cannot trigger it by commenting. Verified against the action's security
  documentation rather than assumed.
- The review job is **read-only**. Anything that needs to change the repository goes
  through `claude.yml`, which is triggered deliberately and carries the write permissions.
- `allowed_bots: 'dependabot[bot]'` — the action refuses non-human actors unless named, so
  without it the PRs least likely to get a human read would also get no review.
- Actions are pinned by SHA, matching the other workflows here. `anthropics/claude-code-action@v1`
  is an *annotated* tag, so the pin is the dereferenced commit, not the tag object.
- `--allowedTools` includes `just golden` and `just test`, which retch has no equivalent of:
  a changed argv must be regenerated and committed deliberately, never left stale.

### v0.0.9 — redacted infrastructure identifiers

- `PLAN.md` and `AGENTS.md` now use placeholder hostnames, bucket and project names. A pure
  1:1 substitution — 30 lines changed, no text altered beyond the identifiers themselves.
- Host letters follow the snapshot-count ordering so the fleet table still reads the same
  way, and two things are deliberately preserved: the per-host snapshot counts, which are
  the evidence the retention fix worked, and the dotted `.local` form, which is the reason
  `${host_short}` exists at all.
- Source, tests, workflows, `README.md` and this file were already clean; commit messages
  too. Only the two design documents carried anything.

### v0.0.8 — faster CI audit

- The Security Audit job was the slowest thing in CI by four times over — ~373s against the
  Rust job's ~94s — and roughly 5.5 minutes of that was `cargo install cargo-audit`
  compiling a tool whose actual work is reading `Cargo.lock` against an advisory database.
- Now installs a **prebuilt binary published by the RustSec organisation**, with both the
  version and the tarball's sha256 pinned. That adds no new party to trust: the advisory
  database this job consumes comes from the same place. Measured at 0.5s locally against
  the same script the workflow runs.
- Neither pin is managed by dependabot, so bumping the version means updating the hash.
  There is deliberately no fallback to `cargo install` — a checksum mismatch should fail
  loudly rather than quietly take a slower path that skips the verification.
- **The audit deliberately still runs on pull requests.** `just pr` runs it locally but
  advisory-only and non-blocking by design, so it is not a gate; and the advisory database
  changes independently of this code, so a branch that was clean when it was written can be
  flagged by the time it merges.
- Note the `paths:` filter on that workflow never spares a run in practice: the house
  convention bumps the version in `Cargo.toml` on every PR, which always matches it.

### v0.0.7 — the CLI surface, and Milestone 1 complete

- Milestone 1 step 7, and the end of the milestone. `--rustic-binary` on `run` and `plan`.
- Verification-ladder rung 2 is now a test: a recording shim stands in for rustic, the job
  runs end to end, and the argv it received is asserted — with no repository involved.
- The exit-code surface is asserted rather than assumed: 0 / 1 / 2 verified by tests.
- **Found while adding those tests:** two tests running a job of the same name contended on
  the run lock and one was correctly refused, making the suite flaky. The lock is keyed on
  job name and is machine-wide — right in production, where two runs of one job must not
  overlap. Fixed by giving each test its own job name rather than by weakening the lock.

### v0.0.6 — the runner

- Milestone 1 step 6. `run -n <job>` with `--dry-run`. The first version that can actually
  run a backup.
- Stops on failure, continues on partial, and **records skipped operations explicitly** —
  "retention did not run" must be visible rather than inferred from an absence.
- Local per-job `flock`, non-blocking: a second run is refused rather than queued, since an
  hourly timer that waited would pile up behind a long backup. Held in `$XDG_RUNTIME_DIR`,
  so a stale lock cannot survive a reboot. Holding it is a *parameter* of `run_job`, so
  running a job without its lock is not expressible.
- `lock::budget()` returns `None` and is documented as deferred to M4. A plausible-looking
  value would be worse than none: it would read as cross-machine coordination where there
  is none.
- The **`forget` scoping invariant** lands here, with key names verified rather than
  guessed. See `PLAN.md` M1 step 6 and `WIP.md`.
- **Bug found by running it for real: `-P` now gets a resolved absolute path.** A bare
  profile name made rustic search its own paths, which need not include the directory
  rusticprofile validated against — so a job could validate and then run against a
  different profile, or none at all.
- A dry run reports what it *would* save, never what it saved.

### v0.0.5 — exit classification

- Milestone 1 step 5, the most important behaviour in the milestone. `rustic/exit.rs` tells
  a partial backup from a failed one by counting `--json` snapshot objects on stdout, since
  rustic exits 1 for both.
- Characterised empirically against rustic 0.11.3 first; the measurement table is in
  `PLAN.md` M1 step 5 and repeated in the module docs.
- `Verdict::Partial` continues the job so retention still runs — the structural fix for the
  abort-before-retention bug. Partial is only claimed on positive evidence, because the
  opposite error runs `forget` after a backup that saved nothing.
- `backup` invocations now carry `--json`, which changed the golden argv files. The golden
  gate caught it, which is what it is for.
- Three integration tests run against real rustic in a throwaway local repository and skip
  with a notice when it is absent. **CI has no rustic, so CI skips them.**

### v0.0.4 — exec and redaction

- Milestone 1 step 4. `exec/` spawns a child with no shell, forwards SIGINT and SIGTERM to
  it and waits rather than orphaning it, and reports a descriptive `Outcome` that passes no
  judgement on whether the backup worked — that is step 5's job.
- stdout is captured while stderr is inherited, which is what lets an operator watch
  progress live *and* leaves step 5 the `--json` output it needs.
- Redaction masks by variable name, distinguishing a secret from a path: `RUSTIC_PASSWORD`
  is hidden, `RUSTIC_PASSWORD_FILE` is shown, `RUSTIC_PASSWORD_COMMAND` is hidden because a
  command can embed the secret inline. `GOOGLE_APPLICATION_CREDENTIALS` is allowlisted as
  path-valued. The marker is fixed-width so it does not leak the secret's length.
- `plan --show-env` prints the rustic-related environment with masking applied;
  `--show-secrets` prints it in full after a stderr warning. `--show-env` is rejected with
  `--format lines`, which is an exact machine-readable form that extra output would corrupt.
- A test spawns a real child and forwards a signal to it, so the forwarding path is exercised
  rather than assumed.

### v0.0.3 — invocation planning

- Milestone 1 step 3. `plan -n <job>` with `--format lines|human`, printing the exact argv a
  job would run without running it. `rustic/invoke.rs` is a pure function of (config, job).
- Golden tests under `tests/golden/`, one argv element per line so a changed argument is a
  one-line diff. `just golden` regenerates; `just check` fails if any golden is stale or
  untracked. Staleness is detected by content hash rather than git state, so goldens that
  are staged but not yet committed do not trip it.
- **New validation rule: a snapshot-set name may not start with `-`.** Each name becomes its
  own argv element, so a set called `--password` would have put that string on a rustic
  command line. Found by a test that was asserting the wrong thing — see below.
- A test asserts `-P`, the operation and `--name` are the *only* flags any built argv
  contains, which makes the delegation boundary a thing CI enforces rather than a claim.
- The golden gate is enforced in **CI as well as `just check`**. CI runs the cargo commands
  directly and `just` is not installed on every runner, so relying on the pre-push hook alone
  would have let a stale golden reach `main` via `GIT_NO_CHECK=1` or a machine without `just`.

### v0.0.2 — configuration

- Milestone 1 step 2. `config --check` and `config --show -n <job>`, both with `--as-host`
  and `--config`, both hermetic — no rustic binary, no repository, no network.
- Load pipeline fixed at read → parse → host-gate → interpolate → validate, so a directive
  inside a comment can never be evaluated and substitution can never break the parse.
- `${…}` is a closed variable set with a `$${` escape and no control flow of any kind.
  `${date:…}` is validated at load time but resolved per run, so a generated unit file
  cannot freeze one day's date.
- Snapshot-set names are cross-checked against `rustic.toml` — the check that exists purely
  because rustic silently ignores an unknown `--name` alongside a valid one.
- Four validation rules added beyond `PLAN.md`'s original list, now recorded there: declared
  rather than resolved name checking, `enabled-on-hosts: []` refused, log paths must be
  absolute, and `${job}`/`${profile}` refused inside `defaults`.
- Deps added: `serde`, `serde_yaml_ng`, `toml`, `dirs`, `jiff`, `nix`, and `tempfile` (dev).

### v0.0.1 — scaffolding

- Repository scaffolded per the house conventions in `PLAN.md` §2.5: `just` as the only
  task runner, the `check`/`pr`/`open-pr`/`merge-pr` gate triad, real git hooks, CI with a
  SHA-pinned action set, and the community-health file set.
- CLI surface limited to `--help`, `--version` and `--completions`; a bare invocation
  exits non-zero rather than silently succeeding.
- Preceded by the Part 7 decision (option B, named snapshot sets selected per host), which
  unblocked all code. See `PLAN.md` Part 7.
