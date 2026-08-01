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
  config/         M1  parse, host-gate, interpolate, validate jobs.yaml
  rustic/         M1  build the rustic argv; classify its exit
  exec/           M1  spawn, forward signals, mask secrets in logs
  run/            M1  operation ordering; LockBudget seam
  report.rs       M1  owo-colors output
  schedule/       M2  systemd units; M3 launchd plists

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
- **Version bump on every PR**, and **stay in the `0.0.x` series until Milestone 1 delivers a
  working tool.** `v0.1.0` is reserved for that milestone — the first version that can actually
  run a backup end to end. Until then every bump is a patch bump regardless of how much it
  adds, so `0.0.3`, `0.0.4` and so on; the sibling repos' "minor for features" rule resumes
  only after `v0.1.0`. Publishing a `0.1.0` that cannot back anything up would misrepresent
  what the tool does to anyone reading the version alone. Tag `v$VERSION` from a clean `main`;
  never `cargo publish --allow-dirty`.
- **Deliberate absences.** No `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `rust-toolchain.toml`, MSRV declaration, `[lints]` table, `#![deny(...)]` or
  `CHANGELOG.md`. Their absence is the convention — do not add them.
- **Backup safety**: read-only operations against a production repository are fine; every
  write test goes to a throwaway repository under a temp dir, deleted afterwards; never
  `prune` against a shared repository before M4; never delete snapshots without explicit
  per-step authorisation. See `AGENTS.md` Part 2 §3.
- **No live infrastructure identifiers in tracked files** while the repository is private
  pending redaction. See `WIP.md`.

---

## Current State (v0.0.12)

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

- [ ] **The `forget` scoping invariant is not implemented yet, and must land before step 6
      (the runner) can execute a `forget`.** `PLAN.md` "Risks and non-goals" requires that a
      `forget` resolving to no host/path/tag filter be refused at load time — "forget across
      every host in a shared repository" has to be spelled out, never defaulted. It is not in
      v0.0.2 because the rustic `[forget]` **config key names** were not verified, and
      guessing them for a safety-critical rule is worse than not having the rule yet. Verify
      them empirically (the CLI flags are `--filter-host`, `--filter-paths`, `--filter-tags`
      per `PLAN.md` §5.5; the config spellings need confirming), then add the check alongside
      the existing `rustic.toml` read. Same applies to `--group-by`, which defaults to
      `host,label,paths` and should be required explicitly rather than inherited.
      Nothing can run a `forget` today, so deferring is safe — but only until step 6.
- [ ] First benchmark + `benches/`, `criterion`, `[[bench]]` (see deviation 2 above)
- [ ] Decide what a partial backup should *do* — classify as warning, so `forget` still
      runs, is the design intent; settle the exit code when `rustic/exit.rs` is written
- [ ] Redact infrastructure identifiers before making the repository public (see `WIP.md`)
- [ ] Add a Pre-PR Checklist section to `AGENTS.md` Part 2, mirroring retch's §4
- [ ] Decide what migrates out of `PLAN.md` into this file now that code exists

---

## 5. Release Log

Versions are `0.0.x` until Milestone 1 is complete; see §3. Nothing has been tagged or
published, so no version here has ever left this repository.

*The first two entries were briefly numbered `0.1.0` and `0.2.0` before that policy was set,
and were renumbered in place. No tags existed, so nothing had to be unwound — if you find an
external reference to a rusticprofile `0.1.0` or `0.2.0` from July 2026, it predates the
renumbering and means the versions below.*

### v0.0.12 — make the review visible when it finds nothing (unreleased)

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

### v0.0.11 — record how the review workflow can silently skip (unreleased)

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

### v0.0.10 — Claude review workflows (unreleased)

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

### v0.0.9 — redacted infrastructure identifiers (unreleased)

- `PLAN.md` and `AGENTS.md` now use placeholder hostnames, bucket and project names. A pure
  1:1 substitution — 30 lines changed, no text altered beyond the identifiers themselves.
- Host letters follow the snapshot-count ordering so the fleet table still reads the same
  way, and two things are deliberately preserved: the per-host snapshot counts, which are
  the evidence the retention fix worked, and the dotted `.local` form, which is the reason
  `${host_short}` exists at all.
- Source, tests, workflows, `README.md` and this file were already clean; commit messages
  too. Only the two design documents carried anything.

### v0.0.8 — faster CI audit (unreleased)

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

### v0.0.7 — the CLI surface, and Milestone 1 complete (unreleased)

- Milestone 1 step 7, and the end of the milestone. `--rustic-binary` on `run` and `plan`.
- Verification-ladder rung 2 is now a test: a recording shim stands in for rustic, the job
  runs end to end, and the argv it received is asserted — with no repository involved.
- The exit-code surface is asserted rather than assumed: 0 / 1 / 2 verified by tests.
- **Found while adding those tests:** two tests running a job of the same name contended on
  the run lock and one was correctly refused, making the suite flaky. The lock is keyed on
  job name and is machine-wide — right in production, where two runs of one job must not
  overlap. Fixed by giving each test its own job name rather than by weakening the lock.

### v0.0.6 — the runner (unreleased)

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

### v0.0.5 — exit classification (unreleased)

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

### v0.0.4 — exec and redaction (unreleased)

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

### v0.0.3 — invocation planning (unreleased)

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

### v0.0.2 — configuration (unreleased)

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

### v0.0.1 — scaffolding (unreleased)

- Repository scaffolded per the house conventions in `PLAN.md` §2.5: `just` as the only
  task runner, the `check`/`pr`/`open-pr`/`merge-pr` gate triad, real git hooks, CI with a
  SHA-pinned action set, and the community-health file set.
- CLI surface limited to `--help`, `--version` and `--completions`; a bare invocation
  exits non-zero rather than silently succeeding.
- Preceded by the Part 7 decision (option B, named snapshot sets selected per host), which
  unblocked all code. See `PLAN.md` Part 7.
