# rusticprofile

A local, per-machine scheduler and orchestrator for [rustic](https://rustic.cli.rs) backups.

> **Note**: The name is a lineage marker, not a compatibility claim. rusticprofile makes **no compatibility promises** with [resticprofile](https://github.com/creativeprojects/resticprofile) — not the config schema, not the CLI, not the exit codes, not the output. It cannot read a `profiles.yaml`.

> **Note**: This project was 100% vibe coded. Real programmers are welcome.

## Status

**Milestones 1 and 2 complete — this is `0.1.0`, the first release.** rusticprofile can validate a configuration, show exactly what it would run, run it — taking a local lock, sequencing operations, classifying what rustic reports and summarising the result — and schedule itself with systemd.

**Linux only in practice.** Everything but `schedule` works anywhere; scheduling needs systemd, and macOS launchd is the next milestone. See *What it does not do yet* below, which you should read before pointing several machines at one repository.

Today:

```bash
rusticprofile config --example jobs                  # an annotated starting-point config
rusticprofile config --example rustic                # ...and the rustic side, annotated at length
rusticprofile config --check                         # validate; every problem at once
rusticprofile config --check --as-host host-a.local  # another machine's view of the gates
rusticprofile config --show -n dot-files             # the resolved form of one job
rusticprofile plan -n dot-files                      # the exact rustic argv, without running it
rusticprofile plan -n dot-files --format lines       # one argv element per line
rusticprofile plan -n dot-files --show-env           # ...plus the environment, secrets masked
rusticprofile run -n dot-files --dry-run            # what a run would do, writing nothing
rusticprofile run -n dot-files                      # actually run it

rusticprofile schedule -n dot-files                 # install a systemd timer (inert)
rusticprofile schedule -n dot-files --enable        # ...and actually start it
rusticprofile status                                # what is scheduled here, and what is not
rusticprofile unschedule -n dot-files               # remove the units
```

Installing a timer and starting it are separate on purpose: adding a writer to a shared repository should be something you asked for, not a side effect of installing a file.

`config` and `plan` are hermetic — no rustic binary, no repository, no network — so they are safe to run anywhere. `run` is the one that actually invokes rustic, and `schedule` is the one that touches systemd. Running the binary with no arguments exits non-zero and says so rather than doing anything by default.

## What it does

rusticprofile owns **when** backups run, **which** jobs exist, and **on which hosts**:

- **systemd** units — install, remove, status
- per-host job gating, so one machine runs the prune job and the others do not
- operation sequencing within a job (backup, then forget)
- exit classification, including telling a partial backup apart from a failed one
- refusing configurations that would quietly do less than they say

## What it does not do *yet*

These are planned and not written. They are listed here rather than in the section above
because a backup tool's README is a safety surface, and an aspirational feature list is a
way to lose data.

- **launchd (macOS).** Only systemd is implemented. `schedule` **refuses** on other
  platforms rather than writing units nothing will run; `config`, `plan` and `run` work
  everywhere, so a job can be driven by any scheduler you already have.
- **Repository lock coordination.** `run` takes a local per-job lock so two runs on *one*
  machine cannot overlap. There is **no cross-machine coordination**, and you should read
  the next paragraph before pointing several machines at one repository.

> [!WARNING]
> **Do not run `restic prune` against a repository that rustic may be writing to.**
>
> This is about *mixing* the two tools, not about rustic. rustic is lock-free **by design**
> and it works: `prune` marks packs and only deletes them after `--keep-delete`, 23 hours by
> default, so a concurrent rustic backup has a day of grace. Verified — a default
> `rustic prune` left every pack on disk and only `--instant-delete` removed them.
>
> restic reaches the same safety a different way: an exclusive lock object inside the
> repository, which lets it delete immediately. **rustic does not take that lock**, so the
> two schemes do not compose.
>
> | | safe? | |
> |---|---|---|
> | `rustic prune` + rustic backup | yes | 23h grace period |
> | `restic prune` + restic backup | yes | restic's repository lock |
> | **`restic prune` + rustic backup** | **no** | restic deletes at once, relying on a lock rustic never takes |
>
> Measured for that last row: with a rustic backup in flight, `restic prune` did not wait.
> It deleted 14 packs as unreferenced, and `restic check --read-data` afterwards reported
> *"The repository is damaged and must be repaired."*
>
> If everything touching the repository is rustic, none of this applies. `PLAN.md` §7.6 has
> the measurements.

## What it deliberately does not do

Backup configuration is delegated entirely to rustic's own `rustic.toml`, which already covers the repository, source paths, excludes, retention policy, hooks, environment and Prometheus metrics — natively, with env > config > CLI precedence.

rusticprofile constructs **no** backup flags. A job resolves to:

```
rustic -P <resolved-profile-path> <operation> [--dry-run] [--json] [--name <set>]...
```

That is the whole contract. A wrapper that re-specified rustic's options would mostly reinvent rustic.

Also out of scope for v1: reading resticprofile config, a `migrate` command, restic as a backend, Windows, cron/schtasks, groups, hooks, metrics, and templating in any form — including a "just one small conditional" escape hatch.

**Restore is not here either.** Use `rustic restore` directly. Putting a restore path behind a scheduler adds a layer between you and your data at the exact moment you least want one.

## Why it exists

`rustic_scheduler` already exists in the rustic organisation, but it is client/server: an always-on central scheduling server with clients attached over websockets. That is the wrong shape for a fleet of intermittently-online personal machines. The genuinely unfilled gap is local, per-machine OS-level scheduling with per-host variation and no server at all.

The design also carries hard-won opinions from operating its Go predecessor, where four separate bugs each caused a **silent** degradation — retention that matched zero snapshots for months, a backup that aborted before retention because two source paths did not exist, a config that failed to load because of a template directive inside a comment, and a config block that had never taken effect at all. For a backup tool, silent is the failure class that matters. So:

- unknown config keys are a hard error, not a shrug
- validation is batched and happens before any process is spawned, with exit 2 kept distinct from "the backup failed"
- a job that would do nothing on this host is an error unless the config explicitly says it should do nothing here
- no shell is ever involved — commands are built as an argv and spawned directly, which deletes an entire category of quoting and secret-leaking bugs

`PLAN.md` has the full reasoning, every rejected alternative, and the measurements behind each decision.

## Getting a configuration

Two annotated files, written to stdout:

```bash
rusticprofile config --example jobs   > ~/.config/rusticprofile/jobs.yaml
rusticprofile config --example rustic > ~/.config/rustic/dot-files.toml
rusticprofile config --check
```

The second one is the one to read properly. Because rusticprofile delegates all backup
configuration to rustic, nearly everything that can silently lose data lives in that file —
exclusion globs are *include* filters unless prefixed with `!`, a scoping filter under
`[forget]` is accepted and then ignored, and retention grouped by host alone lets a 0-byte
snapshot evict a 395 MiB one. Every annotation in it records a measured failure.

Placeholders (`host-a`, `/home/user`) are static and deliberately not filled in for you.

## Configuration sketch

```yaml
# ~/.config/rusticprofile/jobs.yaml
schema: 1

jobs:
  dot-files:
    profile: dot-files              # -> rustic -P dot-files
    operations: [backup, forget]
    snapshot-sets:
      - name: core
      - name: gnupg
        enabled-on-hosts: [host-a, host-b]
    schedule:
      at: hourly
      permission: user
      priority: background
```

Note what is *not* in there: no source paths, no excludes, no retention numbers, no repository. Those live in `rustic.toml`.

## Installation

```bash
git clone https://github.com/l1a/rusticprofile.git
cd rusticprofile
just install       # binary + man page + completions for six shells
```

Requires [just](https://github.com/casey/just) and, for the man page, [mandown](https://crates.io/crates/mandown).

## Development

```bash
just setup    # install git hooks (pre-push runs fmt + clippy)
just check    # cargo fmt --check + cargo clippy -D warnings
just test
just open-pr  # runs the full pre-PR gate, then gh pr create
```

See `CONTRIBUTING.md`, and `NOTES.md` for current state and the release log.

## License

GPL-3.0-or-later. See `LICENSE`.
