# NAME

rusticprofile - a local, per-machine scheduler and orchestrator for rustic backups

# SYNOPSIS

**rusticprofile** [*OPTIONS*]

**rusticprofile config** **--check** [**--as-host** *HOST*] [**--config** *PATH*]

**rusticprofile config** **--show** **-n** *JOB* [**--as-host** *HOST*] [**--config** *PATH*]

**rusticprofile run** **-n** *JOB* [**--dry-run**] [**--json**] [**--background**] [**--rustic-binary** *PATH*] [**--as-host** *HOST*] [**--config** *PATH*]

**rusticprofile schedule** [**-n** *JOB*] [**--write-only**] [**--config** *PATH*] [**--unit-dir** *DIR*]

**rusticprofile unschedule** **-n** *JOB* [**--config** *PATH*] [**--unit-dir** *DIR*]

**rusticprofile status** [**--json**] [**--as-host** *HOST*] [**--config** *PATH*] [**--unit-dir** *DIR*]

**rusticprofile snapshots** **-n** *JOB* [**--config** *PATH*] [**--as-host** *HOST*] [**--rustic-binary** *PATH*] [**--** *RUSTIC ARGS*]

**rusticprofile plan** **-n** *JOB* [**--format** *FORMAT*] [**--show-env** [**--show-secrets**]] [**--as-host** *HOST*] [**--config** *PATH*]

# DESCRIPTION

**rusticprofile** decides *when* backups run, *which* jobs exist, and *on which hosts*. It installs and manages the systemd units (or launchd agents) that trigger them, sequences the operations within a job, classifies what **rustic**(1) reports back, and coordinates locks against a shared repository.

It does **not** configure backups. The repository, source paths, excludes, retention policy, hooks, environment and metrics all live in rustic's own `rustic.toml`, which covers them natively. rusticprofile constructs no backup flags: a job resolves to `rustic -P <profile> <operation>`, plus `--json` and one `--name` for each snapshot set enabled on the current host.

This separation is deliberate. rustic already provides profiles, hooks, forget policies and Prometheus metrics; a wrapper that re-specified them would mostly reinvent rustic. The gap this fills is local OS-level scheduling with per-host variation and no central server.

The **config** subcommand inspects and validates `jobs.yaml`, and the **plan** subcommand prints the exact command line a job would run without running it. Neither ever invokes **rustic**(1), contacts a repository, or needs a network, so both are safe to run at any time.

The **run** subcommand executes a job, and **schedule** installs the systemd units (Linux) or launchd agent (macOS) that triggers it. Invoking **rusticprofile** with no arguments exits non-zero and says so rather than doing anything by default.

# OPTIONS

**-h, --help**
:   Show help information.

**-V, --version**
:   Print version information.

**--completions** *SHELL*
:   Write a shell completion script to standard output. *SHELL* is one of `bash`, `zsh`, `fish`, `elvish`, `nushell` or `power-shell`.

# CONFIG

**--check**
:   Validate the configuration. **Every problem is reported at once**, not one per run, and nothing is spawned before validation completes. Exits 0 if the configuration is usable, 2 otherwise. Also prints which jobs this host runs and which are excluded by **enabled-on-hosts** — a gate that cannot be seen is indistinguishable from a job that was never written.

**--show** **-n** *JOB*
:   Print the resolved form of one job: its operations, the snapshot sets that survive host gating, its schedule and its log path. If *JOB* exists but is gated off on this host, that is reported as such rather than as an unknown job.

**--example** *WHAT*
:   Write an annotated starting-point configuration to stdout, where *WHAT* is **jobs** or **rustic**. Requires no existing configuration and reads nothing — it is what you ask for when you have neither file yet.

    **jobs** is the small one: rusticprofile owns only which jobs exist, what they run, on which hosts, and when. **rustic** is the one that matters, because the delegation boundary puts nearly everything that can silently lose data in rustic's own config — the exclusion globs that are *include* filters unless prefixed with `!`, the scoping filter that rustic accepts and ignores if placed under **[forget]** instead of **[snapshot-filter]**, and the retention grouping that lets a 0-byte snapshot evict a 395 MiB one. Each annotation records a measured failure, not a preference.

    Emitted to stdout rather than written anywhere, exactly as **--completions** is: placing the file is a deliberate act, and this command cannot overwrite a working configuration. Redirect it yourself.

    Placeholders (`host-a`, `/home/user`) are **static**. Your hostname and home directory are deliberately not filled in — a configuration that appears to work is one nobody reads, and every value in these files is worth reading once.

**--as-host** *HOST*
:   Evaluate as though running on *HOST* instead of the real hostname. This is the only way to check another machine's view of a per-host gate without logging into it.

A configuration may set **defaults.default-job**, which is the job used by **run**, **plan**, **snapshots** and **config --show** when `-n` is omitted. It is validated at load time against the *declared* jobs, so a typo is an error on every machine rather than only where that job happens to run.

Two commands deliberately ignore it. **unschedule** always requires an explicit name — removing a schedule because a configuration file named a default, rather than because someone typed it, is the one action that should never happen by default. **schedule** already treats a missing `-n` as "every job that declares a schedule", which is useful in its own right and would be lost.

**--config** *PATH*
:   Read *PATH* instead of the default `jobs.yaml`.

Validation also rejects a `sources` entry containing `~` or `$` in a profile a job backs up with. rustic expands neither, and the failure is silent rather than loud: the literal string is a *relative* path, so it misses the hard error an absent absolute path produces. rustic warns, backs up nothing, saves a 0-byte snapshot and exits 0 — and that empty snapshot then wins its retention slot against the real one. A `rustic.toml` therefore cannot be shared unmodified between hosts and must be generated per host; `jobs.yaml` has no such limitation and is identical everywhere.

Validation rejects, among other things, any key it does not recognise; a snapshot set that is not defined in the rustic profile it names; a job whose snapshot sets are all gated away on this host; an **enabled-on-hosts** list that is empty; a relative log path; and an unknown or malformed `${...}` reference. Each of these would otherwise cause the tool to quietly do less than the configuration says.

# RUN

**-n** *JOB*
:   The job to run. Required — there is no "run everything" form.

**--dry-run**
:   Ask rustic to report what it would do without writing anything. Reports what *would* be saved, never what was saved.

**--background**
:   Detach from the console so the run shows no window. Windows only; accepted and ignored elsewhere. **schedule** puts this in every generated Task Scheduler definition, and it is the only reason a scheduled run is invisible: Task Scheduler can run a task as the logged-on user only through an interactive logon, which starts it inside that desktop session, where Windows gives a console program a console. The logon types that run outside it need rights an ordinary account does not hold. Typing this by hand discards the run's output — rustic's progress goes nowhere — while the record still reaches the **log:** file.

**--rustic-binary** *PATH*
:   Use this executable instead of the one the configuration names. A bare name is resolved on `PATH`. Applied after the configuration is validated, so it cannot mask a mistake in the file it overrides. Point it at a script that records its arguments and exits, and a job can be exercised end to end without rustic running at all.

Operations run in the order the job lists them. A failed operation stops the job; a **partial** one does not. A backup that saved some of its snapshot sets and failed on others still proceeds to **forget**, because skipping retention after a partly-successful backup is how snapshots accumulate without limit. A backup that saved *nothing* does stop the job, since running retention after it would delete old snapshots with no new ones in place. Operations that did not run are listed explicitly in the summary.

A local lock prevents the same job running twice on this machine, and is not taken by waiting: a second run is refused immediately rather than queued. It says nothing about other machines sharing the repository — that is a later milestone, and until it lands **prune** must not be run against a shared repository.

Exit status is **0** for success or partial, **1** for failure, **130** for interruption.

# SCHEDULE

**schedule** installs a job's OS schedule and arms it; **unschedule** disarms and removes it. Each is a single step and each fully undoes the other. **status** reports what is installed on this host and what is deliberately not.

On **Linux** that means systemd units driven by `systemctl`; on **macOS**, a launchd agent driven by `launchctl`. On a platform with neither, **schedule** refuses and writes nothing — files on disk plus a success message are indistinguishable from a working install, so refusing is the only honest answer. **status** reports the backend it found, and `status --json` names it in a `backend` field.

On **Windows** it means one Task Scheduler task under `\rusticprofile\`*JOB*, driven by `schtasks`. Where there is no backend at all, **status** still prints each job's `last run` and `last success`, because that record is written by **run** and does not come from a service manager; it is the field to watch in any case, since a run that never happens reports nothing.

**-n** *JOB*
:   The job to act on. Optional for **schedule**, where omitting it installs every job declaring a `schedule:` block. Required for **unschedule** — removal is always named explicitly.

**--write-only**
:   Write the schedule without arming it. `schedule` arms by default — that is what the verb means, and **unschedule** is a single step that fully undoes it. Use this to inspect what was generated before anything can fire.

**--unit-dir** *DIR*
:   Write or read units and agents in *DIR* instead of the platform's own directory. Intended for inspecting what is generated without installing it; neither `systemctl` nor `launchctl` is invoked when this is given.

Both commands are idempotent on both platforms: identical content is not rewritten, and removing something that is not there is not an error. Re-running **schedule** reports `unchanged` rather than implying work happened.

Nothing generated contains a **log path or a date**. A unit or agent written today must not log to today's file forever, so `${date:...}` is resolved per run rather than at install time.

## systemd (Linux)

A scheduled job is **two** units, `rusticprofile-`*JOB*`.service` and `.timer`. systemd offers no way for a timer to run a command directly, so this is not a choice rusticprofile makes. Only the timer carries an `[Install]` section; the service is reported as `static`, meaning it can be started by its timer and not enabled independently — a service that could be enabled on its own would run the backup at every login, with no schedule to explain it.

The timer sets **Persistent=true**, so a run missed while the machine was asleep is caught up rather than silently skipped, and **RandomizedDelaySec**, so several machines sharing one repository do not all wake on the same instant. Priority is expressed as `Nice=` and `IOSchedulingClass=` in the unit rather than applied in-process.

## Task Scheduler (Windows)

A scheduled job is **one** task, registered at `\rusticprofile\`*JOB* rather than written into a directory a service manager reads: `schtasks /Create /XML` copies the definition into the service's own store. rusticprofile keeps the definition it generated under `$XDG_STATE_HOME/rusticprofile/tasks` as a record, and the service remains the authority on what is actually scheduled. `permission: system` registers the task under the LocalSystem account instead of your own.

Three things differ from the Unix backends, each measured rather than assumed:

**Hourly is twenty-four triggers, not one repeating trigger.** A repeating trigger whose start boundary lies in the past is treated by Task Scheduler as *currently due* and runs the task the moment it is registered — which would make **schedule** take a backup, and run retention, as a side effect of scheduling. Twenty-four plain daily triggers, one per hour, avoid that while still firing within the hour.

**Both priorities are written out.** `priority: standard` emits `<Priority>5</Priority>` rather than nothing, because Task Scheduler's own default is 7 — already below normal — so silence would quietly mean "de-prioritised" here while meaning "leave the default alone" on the other two platforms.

**Two Task Scheduler defaults are overridden**, because both would stop backups without reporting anything: `DisallowStartIfOnBatteries` and `StopIfGoingOnBatteries` are `true` by default, so a laptop on battery would take no backups and unplugging mid-run would kill one. `ExecutionTimeLimit` is also cleared, since the default of three days would terminate a long first backup rather than let it finish.

A user task, like a launchd agent, runs **only while you are logged on** — there is no `linger` equivalent. `permission: system` runs regardless, as SYSTEM; running as yourself without a login session would need a stored password or S4U, which relocates the credential problem rather than removing it.

## launchd (macOS)

A scheduled job is **one** agent, `~/Library/LaunchAgents/local.rusticprofile.`*JOB*`.plist`, because launchd puts the schedule and the program in the same job. `permission: system` installs a LaunchDaemon under `/Library/LaunchDaemons` instead. Priority becomes `ProcessType=Background`, `Nice` and the `LowPriorityIO` keys — again in the agent, not applied in-process.

Four differences from systemd are worth knowing, because each is a property of launchd rather than of this tool:

**The fleet spread is a real minute.** launchd has no `RandomizedDelaySec`, so the offset is part of `StartCalendarInterval` and **schedule** prints it (`runs hourly at 7 past`). It is chosen once, on first install, and reused afterwards — so re-running **schedule** neither moves your slot nor reports a change. It is bounded by the same window systemd is given, so `at: hourly` means the same thing on both platforms.

**Sleep is handled; an unloaded agent is not.** `launchd.plist`(5) states that a job whose calendar time passes while the machine is asleep runs when it next wakes, with multiple missed intervals coalesced into one — which is what systemd's `Persistent=true` provides. A calendar time that passes while the agent is *not loaded* is not caught up, so the first run after **schedule** is at the next occurrence rather than immediately.

**A user agent runs only while you are logged in.** launchd has no equivalent of systemd's `linger`, so a Mac sitting at the login window takes no backups at all: nothing fails, nothing is logged, and the only evidence is an absence. **schedule** and **status** both say so. Watch `last success` rather than the schedule, or use `permission: system`, which runs regardless of login — as root, which needs its own answer for repository credentials.

**There is no next-fire time to report.** `launchctl print` gives the calendar descriptor and never a next firing, so `next run` reads *not reported by launchd* and the JSON `next_run` is `null`. Computing one and presenting it as launchd's would be inventing a fact about a schedule.

**--json**
:   Emit the report as JSON instead of the human summary, on **run** and **status**. For anything automated: matching the human summary would mean matching English, which is exactly what rusticprofile refuses to do to rustic's own output and for the same reason — a summary line is a message to a person and changes when the wording improves.

    The object opens with a `schema` number. Fields may be **added** without changing it; anything removed or given a new meaning bumps it, so a consumer that ignores unknown fields keeps working.

    **`last_success` is the field worth alerting on.** A schedule can be armed, green and firing while every run fails, and a disabled timer fails nothing at all — neither shows up in `enabled` or `next_run`. `null` there means the job has never succeeded. Note `enabled` and `active` are likewise `null` for "could not tell", which is not the same as `false`: only `false` means the schedule is definitely off.

    rustic's own progress output goes to stderr, so `rusticprofile run --json 2>/dev/null` is parseable as-is.

**status** distinguishes *installed but not enabled* from *state unknown*, since only the former means the schedule is definitely off, and it lists jobs excluded by **enabled-on-hosts** so that a host without a given timer reads as a decision rather than an absence.

# PLAN

**-n** *JOB*
:   The job to plan. Required — there is no "plan everything" form.

**--format** *FORMAT*
:   `human` (default) prints a readable summary, one operation per line. `lines` prints one argv element per line, with a blank line between operations; this is the exact form, and is what the project's golden tests record.

**--show-env**
:   Also print the rustic-related environment the run would inherit, with secret values masked. Not valid with `--format lines`, which is an exact machine-readable form that extra output would corrupt. rusticprofile does not modify the environment; this shows what rustic will receive.

**--show-secrets**
:   Print secret values in full instead of masking them. Requires **--show-env**, and prints a warning to standard error first. Anything capturing the terminal captures the secrets too.

The argv is the whole of what rusticprofile constructs:

    rustic -P RESOLVED-PROFILE-PATH OPERATION [--dry-run] [--json] [--name SET]...

`-P` is given a resolved absolute path rather than a bare profile name, because rustic's own search for a bare name need not include the directory rusticprofile validated against. `--name` appears only for **backup**, once per snapshot set enabled on the current host, as does `--json`. Those three flags plus `-P` are the only ones rusticprofile ever emits; everything else a backup needs comes from rustic's own configuration. `--json` is requested because rustic exits 1 for both a partial and a failed backup, and the count of snapshot objects it writes to standard output is the only reliable way to tell them apart. Progress and diagnostics are unaffected — rustic writes those to standard error.

# SNAPSHOTS

**snapshots -n** *JOB*
:   List the repository's snapshots by handing the query to **rustic**(1).

    **This is a read-only passthrough, and the only thing rusticprofile contributes is the resolved profile path** — the value you would otherwise have to remember and type. Arguments after `--` are given to rustic unchanged; rusticprofile constructs none of them, and rustic's exit code is passed straight through rather than replaced with a verdict of our own.

    ```
    rusticprofile snapshots -n dot-files
    rusticprofile snapshots -n dot-files -- --filter-label core
    ```

    Read-only is what makes this defensible. There is deliberately no passthrough for **forget** or **prune**, which are destructive and whose scoping belongs in the rustic profile where a flag typed at a prompt cannot contradict it, nor for **restore** — putting a restore path behind a scheduler adds a layer between you and your data at the moment you least want one. `snapshots` is also **not** an operation a job may schedule; that set remains **backup**, **forget** and **prune**.

# SECRETS

rusticprofile stores no secrets and has no secret configuration. The repository password, master key and any cloud credentials belong to **rustic**(1), which offers `--password`, `--password-file` and `--password-command` (and `--key`, `--key-file`, `--key-command`), any of which may be set in its own config file.

Prefer **password-command**. rustic spawns that command itself, so the secret never enters rusticprofile's memory, argument list or environment. rusticprofile never emits a credential-bearing flag into a command line — a process list is world-readable, and rustic's own documentation warns that `--password` can expose the password there.

Anything rusticprofile prints is masked as a backstop. Masking is by variable name and distinguishes a secret from a location: `RUSTIC_PASSWORD` is hidden while `RUSTIC_PASSWORD_FILE` is shown, since a path reveals nothing and is usually the detail worth seeing. A `_COMMAND` variant is hidden, because such a command can embed the secret inline and nothing can distinguish that from a keyring lookup. The replacement marker is fixed, never a run of characters matching the secret's length.

# INTERPOLATION

Strings in the configuration may contain `${...}` references. This is a closed set, not a template language: there are no conditionals, functions, pipelines or loops. An unrecognised name is an error naming the offending key, never an empty string.

`${host}`, `${host_short}`, `${job}`, `${profile}`, `${config_dir}`, `${state_dir}`, `${temp_dir}`, `${os}`, `${arch}`, `${env:NAME}` and `${date:FORMAT}` are recognised. `$${` produces a literal `${`.

`${job}` and `${profile}` have no value outside a job and are errors there. `${env:NAME}` is an error when the variable is unset. `${date:FORMAT}` is validated when the configuration loads but resolved when the job runs, so that a generated unit file carries the reference rather than one day's date.

# EXIT STATUS

The contract the implemented commands follow:

**0**
:   Success, including a run that completed with warnings.

**1**
:   A run failed.

**2**
:   The configuration is invalid. Reported before any process is spawned, with every violation listed at once, so a config error is never confused with a backup failure.

**130**
:   Interrupted.

# FILES

**$XDG_CONFIG_HOME/rusticprofile/jobs.yaml**
:   Job definitions: which jobs exist, their operations, host gating and schedule.

**$XDG_CONFIG_HOME/rustic/PROFILE.toml**
:   rustic's own configuration, which owns all backup detail. rusticprofile reads it read-only, to verify that every snapshot-set name it is about to pass actually exists.

**~/Library/LaunchAgents/local.rusticprofile.**JOB**.plist**
:   The launchd agent on macOS, written by **schedule** and removed by **unschedule**. `permission: system` uses `/Library/LaunchDaemons` instead. On Linux the equivalents are `~/.config/systemd/user/rusticprofile-`*JOB*`.{service,timer}`.

**$XDG_STATE_HOME/rusticprofile/tasks/**JOB**.xml**

:   On Windows, the task definition **schedule** generated. Unlike the Unix backends this file is *not* the installation — `schtasks` copies the definition into the Task Scheduler service's own store, and the registration at `\rusticprofile\`*JOB* is what actually runs. This copy is a record of what was registered, and deleting it does not unschedule anything.

**$XDG_STATE_HOME/rusticprofile/status/**JOB**.json**
:   When *JOB* last ran, and when it last **succeeded**. Written after every run, atomically. `last_success` is carried forward across a failed run on purpose: the useful question is not "did the last attempt work?" but "when did this last actually work?", and only that field can reveal a job which has quietly stopped working — a failing run is loud, a run that never happens is not. **status** displays both.

**$XDG_STATE_HOME/rusticprofile/**
:   Where `${state_dir}` points, and where run logs belong. Logs are **state, not configuration** — the XDG Base Directory specification names them as the example of what `XDG_STATE_HOME` is for. Pointing a `log:` at `${config_dir}` instead is also self-defeating when `~/.config` is one of your backup sources: the job then appends to a directory it is in the middle of backing up, and the rustic profile needs an exclusion to compensate.

The variables are honoured where they are set and fall back to `~/.config` and `~/.local/state` where they are not — **on macOS and Windows as well as Linux**, rather than `~/Library/Application Support` or `%APPDATA%`. A relative value is ignored, as the specification requires; note that on Windows a path without a drive letter counts as relative, so a rooted-but-driveless `log:` is refused there. This is deliberate rather than unidiomatic: `jobs.yaml` is meant to be byte-identical across a fleet, and a location that varied by operating system would make one line of one file resolve to two different places.

# SEE ALSO

**rustic**(1), **systemd.timer**(5), **launchd.plist**(5)

# BUGS

Report issues at <https://github.com/l1a/rusticprofile/issues>.
