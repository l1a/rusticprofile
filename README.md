# rusticprofile

A local, per-machine scheduler and orchestrator for [rustic](https://rustic.cli.rs) backups.

> **Note**: The name is a lineage marker, not a compatibility claim. rusticprofile makes **no compatibility promises** with [resticprofile](https://github.com/creativeprojects/resticprofile) — not the config schema, not the CLI, not the exit codes, not the output. It cannot read a `profiles.yaml`.

> **Note**: This project was 100% vibe coded. Real programmers are welcome.

## Status

**Milestones 1 and 2 complete.** rusticprofile can validate a configuration, show exactly what it would run, run it — taking a lock, sequencing operations, classifying what rustic reports and summarising the result — and schedule itself with systemd. macOS launchd support is next.

Today:

```bash
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

- systemd units and launchd agents — install, remove, status
- per-host job gating, so one machine runs the prune job and the others do not
- operation sequencing within a job (backup, then forget)
- exit classification, including telling a partial backup apart from a failed one
- lock coordination against a repository shared by several machines

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
