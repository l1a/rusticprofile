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

  Tag `v$VERSION` from a clean `main`; never `cargo publish --allow-dirty`.
- **Deliberate absences.** No `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `rust-toolchain.toml`, MSRV declaration, `[lints]` table, `#![deny(...)]` or
  `CHANGELOG.md`. Their absence is the convention — do not add them.
- **Backup safety**: read-only operations against a production repository are fine; every
  write test goes to a throwaway repository under a temp dir, deleted afterwards; never
  `prune` against a shared repository before M4; never delete snapshots without explicit
  per-step authorisation. See `AGENTS.md` Part 2 §3.
- **No live infrastructure identifiers in tracked files.** The repository is public, so this
  is now permanent rather than a pre-publication chore: no real hostnames, bucket names,
  project ids or home paths. Hosts are `host-a`…`host-h`; paths are `/home/user`. Grep the
  diff before opening a PR, and beware substring false positives — a redacted hostname can
  hide inside an ordinary English word.

---

## Current State (v0.1.8)

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
- [ ] **Publishing decision — not yet taken.** `rusticprofile` and `rustic-profile` are both
      still free on crates.io, and AUR has no `rusticprofile` package (rechecked 2026-08-01).
      Nothing forces the choice now that the repository is public and carries no
      infrastructure identifiers, so the trade is:

      - **Publishing early** claims the name with real code rather than a placeholder, which
        is not squatting. But a crates.io version can be yanked and never deleted, and the
        README describes a *scheduler* — shipping one that cannot yet schedule (M2/M3) invites
        confusion about what the crate does.
      - **Waiting** costs only the small risk of someone independently choosing a compound
        name whose sole appeal is as a lineage marker for a Go tool with 19 GitHub repos.

      Recommendation: publish at **M2**, when `schedule`/`unschedule`/`status` make the
      README's first paragraph true. AUR later still — a package wants a tagged release
      tarball, and there is no tag yet.
- [ ] Decide what a partial backup should *do* — classify as warning, so `forget` still
      runs, is the design intent; settle the exit code when `rustic/exit.rs` is written
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
- [ ] **M4 blocks space reclamation, as of 2026-08-02.** The designated prune host's timer is
      disabled (`PLAN.md` §7.6), so packs from forgotten snapshots accumulate with nothing to
      reclaim them. This is a deliberate cost, not an oversight, and it is the reason M4 is now
      ahead of M3 in practical priority even though M3 is next in the numbering.
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

### v0.1.6 — the AUR recipes were breaking Syncthing (unreleased)

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

### v0.1.8 — the automatic Claude review is off (unreleased)

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

### v0.1.7 — `schedule` is one step, and the service unit is `static` (unreleased)

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

### v0.1.5 — fix a fixture that lied on CI, and stop `merge-pr` merging red (unreleased)

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

### v0.1.4 — refuse a `filter-hosts` that cannot match this host (unreleased)

**Found by asking whether the fleet rollout was ready. It was not, and the reason was a bug
in the chezmoi template written the same day.**

`.chezmoi.hostname` is the hostname *up to the first `.`*. Templated into `filter-hosts` it
renders `["foo"]` on a machine whose snapshots rustic records as `foo.local` — and rustic
matches that field **exactly**, so the filter selects nothing, `forget` deletes nothing, and
retention silently never runs while every command reports success. That is bug #1 from
`PLAN.md` §2.1, reintroduced.

It was invisible on the host it was written on: `arrakis` has no domain suffix, so
`.chezmoi.hostname` and `.chezmoi.fqdnHostname` are identical there. Two of the seven hosts
are `*.local`, and neither is rolled out yet — the template would have failed only on the
machines nobody was looking at.

**The template is fixed** (`.chezmoi.fqdnHostname`, with a comment saying why). **More
importantly, so is the reason it was possible.** `check_filter_hosts_can_match` refuses at
load time any profile whose `filter-hosts` does not include the host that will run it:

```
jobs.j.profile: …/p.toml scopes `forget` to `chani`, which does not include this host
  (`chani.local`) — that looks like a short hostname where the full one is needed; rustic
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

### v0.1.3 — correct the lock finding: rustic is not deficient, the mixture is (unreleased)

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

### v0.1.2 — AUR recipes, and a bug they found (unreleased)

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

### v0.1.1 — published, and AUR packaging (unreleased)

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

### v0.0.24 — refuse source paths rustic will not expand (unreleased)

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

### v0.0.23 — `config --example`: ship the findings as a config (unreleased)

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

### v0.0.22 — rustic takes no repository lock, and the cutover made that matter (unreleased)

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

### v0.0.21 — the pre-PR gate no longer needs a terminal (unreleased)

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

### v0.0.20 — first host cut over, and a retention hazard from outside the tool (unreleased)

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

### v0.0.19 — M2 complete: schedule, unschedule, status (unreleased)

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

### v0.0.18 — M2 begins: systemd unit generation (unreleased)

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

### v0.0.17 — verification ladder rungs 7 and 8, and a design finding (unreleased)

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

### v0.0.16 — CI timing, and the review workflow verified end to end (unreleased)

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

### v0.0.15 — drop fedora-arm from pull-request CI (unreleased)

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

### v0.0.14 — make the review outcome visible, properly (unreleased)

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

### v0.0.13 — correct the control-group definition (unreleased)

- `AGENTS.md` listed **four** idle hosts as the control group, including `host-f`.
  `PLAN.md`'s fleet table shows `host-f` as active and identifies it as the development
  machine — the one every throwaway repository and test run this session was created from.
- A control group containing the machine being experimented on is not a control group, so
  this is a defect in a safety rule rather than a typo. There are **three** idle hosts.
- Spotted while redacting the identifiers, where the two documents sat side by side.

### v0.0.12 — make the review visible when it finds nothing (unreleased)

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
