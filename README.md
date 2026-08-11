# rusticprofile

A local, per-machine scheduler and orchestrator for [rustic](https://rustic.cli.rs) backups.

> **Note**: The name is a lineage marker, not a compatibility claim. rusticprofile makes **no compatibility promises** with [resticprofile](https://github.com/creativeprojects/resticprofile) — not the config schema, not the CLI, not the exit codes, not the output. It cannot read a `profiles.yaml`.

> **Note**: This project was 100% vibe coded. Real programmers are welcome.

## Status

**Milestones 1, 2, 3 and 5 complete.** rusticprofile can validate a configuration, show exactly what it would run, run it — taking a local lock, sequencing operations, classifying what rustic reports and summarising the result — schedule itself with **systemd on Linux, launchd on macOS or Task Scheduler on Windows**, and **report what it has been doing**: a per-run log, a status file recording when each job last *succeeded*, and `--json` for anything automated.

**Linux, macOS and Windows.** `schedule` installs systemd units on Linux, a launchd agent on macOS, or a Task Scheduler task on Windows, and refuses anywhere else rather than writing units nothing will run. One limitation is worth knowing before you rely on it, and it applies to **both macOS and Windows**: a user-level schedule runs only while you are logged on, because neither platform has an equivalent of systemd's `linger`. So a Mac at the login window or a PC at the lock screen takes no backups — nothing fails, and the only evidence is an absence. Watch `last success`, or use `permission: system`. On Windows that same interactive logon is why a scheduled run would otherwise open a terminal window every time it fires; `schedule` passes `--background` so the run detaches from its console instead. See *What it does not do yet* below, which you should read before pointing several machines at one repository.

Today:

```bash
rusticprofile config --example jobs                  # an annotated starting-point config
rusticprofile config --example rustic                # ...and the rustic side, annotated at length
rusticprofile config --check                         # validate; every problem at once
rusticprofile config --check --as-host host-a.local  # another machine's view of the gates
rusticprofile config --show -n dot-files             # the resolved form of one job
rusticprofile plan -n dot-files                      # the exact rustic argv, without running it
rusticprofile plan                                   # ...or omit -n to use defaults.default-job
rusticprofile plan -n dot-files --format lines       # one argv element per line
rusticprofile plan -n dot-files --show-env           # ...plus the environment, secrets masked
rusticprofile run -n dot-files --dry-run             # what a run would do, writing nothing
rusticprofile run -n dot-files                       # actually run it
rusticprofile run -n dot-files --json                # ...reporting as JSON, for a wrapper

rusticprofile schedule -n dot-files                  # install and arm this platform's schedule
rusticprofile schedule -n dot-files --write-only     # install it inert, to read first
rusticprofile status                                 # what is scheduled here, plus last run / last success
rusticprofile status --json                          # ...for a monitor; alert on last_success
rusticprofile snapshots -n dot-files                 # list snapshots (read-only passthrough to rustic)
rusticprofile unschedule -n dot-files                # remove the units

rusticprofile doctor                                 # local checks: competing prune schedule, missing secrets
rusticprofile doctor --repository                    # ...plus: is anything else writing retention here?
```

`schedule` and `unschedule` are each a single step, and each fully undoes the other. What a job becomes depends on the platform: **two** systemd units on Linux — a `.timer` and the `.service` it activates, because systemd has no way for a timer to run a command directly, which is true of every tool that schedules with systemd — **one** launchd agent on macOS, or **one** registered task on Windows. On Linux only the timer is enable-able: the service is `static`, so it cannot be armed on its own and run outside its schedule.

`config` and `plan` are hermetic — no rustic binary, no repository, no network — so they are safe to run anywhere. **`doctor` is the deliberate counterpart**: it exists precisely to check what a hermetic command cannot, because the failures that have actually cost data here were configurations that were individually correct and only wrong in combination with another tool. Its default run is still local and instant; `--repository` is opt-in because it is the one check that needs the network and a credential. `run` is the one that actually invokes rustic, and `schedule` is the one that talks to the service manager (`systemctl`, `launchctl` or `schtasks`). Running the binary with no arguments exits non-zero and says so rather than doing anything by default.

## What it does

rusticprofile owns **when** backups run, **which** jobs exist, and **on which hosts**:

- **systemd** units on Linux, **launchd** agents on macOS, **Task Scheduler** tasks on Windows — install, remove, status
- per-host job gating, so one machine runs the prune job and the others do not
- operation sequencing within a job (backup, then forget)
- exit classification, including telling a partial backup apart from a failed one
- refusing configurations that would quietly do less than they say
- on Windows, retrying a failed operation twice at two-minute intervals **when the run was
  started by Task Scheduler** — a missed hourly slot is replayed within seconds of the machine
  waking, which is before the network is back, so without this the run that replaces a missed
  hour is the one most likely to fail. A run you type yourself still fails immediately
- **recording what happened**: a per-run log, and a status file whose `last_success`
  survives a failed run — the one field that reveals a job which has quietly stopped
  working, since a schedule can be armed and green while every run fails
- **`doctor`**: whether a **restic** prune schedule is still armed here (the one combination
  measured to corrupt the repository), whether the credential files the profile names exist,
  and — with `--repository` — whether anything else has been writing retention for a host
- **`--json`** on `run`, `status` and `doctor`, so a monitor never has to match English

## What it does not do *yet*

These are planned and not written. They are listed here rather than in the section above
because a backup tool's README is a safety surface, and an aspirational feature list is a
way to lose data.

- **Backups while nobody is logged in, on macOS *or Windows*.** Only systemd has `linger`. A
  user LaunchAgent runs only inside a login session, and a Windows user task only while you
  are logged on — so a Mac at the login window or a PC at the lock screen takes no backups.
  Nothing fails, and the only evidence is an absence. Watch `last_success`, or use
  `permission: system`, which runs regardless of login as root or SYSTEM. `schedule` and
  `status` both state this. On Windows, running as *yourself* without a login session would
  need a stored password or S4U, which relocates the credential problem rather than removing
  it — so it is deliberately not offered.
- **A next-fire time on macOS.** launchd does not report one, so `next_run` is `null` there
  and `status` says why. `status --json` names the backend so a monitor can tell "launchd
  never tells" from "could not read the timer". Windows and Linux both report a real one.
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

Also out of scope for v1: reading resticprofile config, a `migrate` command, restic as a backend, cron, groups, hooks, metrics, and templating in any form — including a "just one small conditional" escape hatch.

*Windows was on that list until 0.2.0 and is not any more: the machine this project is developed and released from now runs it. `PLAN.md` §7.9 records the reversal, and §5.10 what had to be measured to make it work. `cron` is still out.*

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

defaults:
  default-job: dot-files            # used when a command is given no -n

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
    log: "${state_dir}/${job}-${date:%Y-%m-%d}.log"
```

Note what is *not* in there: no source paths, no excludes, no retention numbers, no repository. Those live in `rustic.toml`.

`${state_dir}` is `$XDG_STATE_HOME/rusticprofile` — logs are state, not configuration. Pointing them at `${config_dir}` is also self-defeating if `~/.config` is one of your backup sources: the job then appends to a directory it is busy backing up.

The XDG variables are honoured **on macOS and Windows too**, falling back to `~/.config` and `~/.local/state` rather than `~/Library/Application Support` or `%APPDATA%` (0.1.25+, Windows in 0.2.0+). On Windows note that a path with no drive letter counts as relative, so a rooted-but-driveless `log:` is refused there — and `HOME` is normally unset, so a `jobs.yaml` written around `${env:HOME}` will not load until you set it. That is deliberate, and it follows from the box below: a `jobs.yaml` meant to be byte-identical across a fleet cannot have `${state_dir}` resolve to a different place depending on the operating system reading it.

**`default-job` is honoured by `run`, `plan`, `snapshots` and `config --show`** — not by `unschedule`, where removal is always named explicitly, nor by `schedule`, where omitting `-n` already means "every job that declares a schedule".

> [!IMPORTANT]
> `jobs.yaml` is designed to be **byte-identical across a fleet**, which makes it only ever as new as the *oldest binary* reading it. Unknown keys and variables are hard errors by design, so a config using `${state_dir}` (0.1.15+) or `default-job` (0.1.20+) will not load on an older build — and that host stops backing up at its next scheduled run. **Upgrade the binaries before pushing the config.**

## Installation

```bash
git clone https://github.com/l1a/rusticprofile.git
cd rusticprofile
just install       # binary + man page + completions for six shells
```

Requires [just](https://github.com/casey/just) and, for the man page, [mandown](https://crates.io/crates/mandown).

`install-completions` tells you whether each shell will actually load them. **zsh is the one that usually will not**: unlike fish and bash-completion, it reads no user directory unless `fpath` names it, so the recipe checks and prints the `fpath+=(…)` line to add rather than claiming success.

**PowerShell** is in the set and works on Windows — verified by generating the script, dot-sourcing it and asking `TabExpansion2`, not merely by the file appearing. It needs one explicit step and the recipe says so: PowerShell loads nothing automatically, so dot-source the file from your `$PROFILE`. `just install-completions` itself runs on Windows once Git's `usr\bin` is on `PATH` (see the Justfile header).

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
