# NAME

rusticprofile - a local, per-machine scheduler and orchestrator for rustic backups

# SYNOPSIS

**rusticprofile** [*OPTIONS*]

**rusticprofile config** **--check** [**--as-host** *HOST*] [**--config** *PATH*]

**rusticprofile config** **--show** **-n** *JOB* [**--as-host** *HOST*] [**--config** *PATH*]

**rusticprofile run** **-n** *JOB* [**--dry-run**] [**--rustic-binary** *PATH*] [**--as-host** *HOST*] [**--config** *PATH*]

**rusticprofile schedule** [**-n** *JOB*] [**--enable**] [**--config** *PATH*] [**--unit-dir** *DIR*]

**rusticprofile unschedule** **-n** *JOB* [**--config** *PATH*] [**--unit-dir** *DIR*]

**rusticprofile status** [**--as-host** *HOST*] [**--config** *PATH*] [**--unit-dir** *DIR*]

**rusticprofile plan** **-n** *JOB* [**--format** *FORMAT*] [**--show-env** [**--show-secrets**]] [**--as-host** *HOST*] [**--config** *PATH*]

# DESCRIPTION

**rusticprofile** decides *when* backups run, *which* jobs exist, and *on which hosts*. It installs and manages the systemd units (or launchd agents) that trigger them, sequences the operations within a job, classifies what **rustic**(1) reports back, and coordinates locks against a shared repository.

It does **not** configure backups. The repository, source paths, excludes, retention policy, hooks, environment and metrics all live in rustic's own `rustic.toml`, which covers them natively. rusticprofile constructs no backup flags: a job resolves to `rustic -P <profile> <operation>`, plus `--json` and one `--name` for each snapshot set enabled on the current host.

This separation is deliberate. rustic already provides profiles, hooks, forget policies and Prometheus metrics; a wrapper that re-specified them would mostly reinvent rustic. The gap this fills is local OS-level scheduling with per-host variation and no central server.

The **config** subcommand inspects and validates `jobs.yaml`, and the **plan** subcommand prints the exact command line a job would run without running it. Neither ever invokes **rustic**(1), contacts a repository, or needs a network, so both are safe to run at any time.

The **run** subcommand executes a job, and **schedule** installs the systemd units that trigger it. Invoking **rusticprofile** with no arguments exits non-zero and says so rather than doing anything by default.

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

**--config** *PATH*
:   Read *PATH* instead of the default `jobs.yaml`.

Validation rejects, among other things, any key it does not recognise; a snapshot set that is not defined in the rustic profile it names; a job whose snapshot sets are all gated away on this host; an **enabled-on-hosts** list that is empty; a relative log path; and an unknown or malformed `${...}` reference. Each of these would otherwise cause the tool to quietly do less than the configuration says.

# RUN

**-n** *JOB*
:   The job to run. Required — there is no "run everything" form.

**--dry-run**
:   Ask rustic to report what it would do without writing anything. Reports what *would* be saved, never what was saved.

**--rustic-binary** *PATH*
:   Use this executable instead of the one the configuration names. A bare name is resolved on `PATH`. Applied after the configuration is validated, so it cannot mask a mistake in the file it overrides. Point it at a script that records its arguments and exits, and a job can be exercised end to end without rustic running at all.

Operations run in the order the job lists them. A failed operation stops the job; a **partial** one does not. A backup that saved some of its snapshot sets and failed on others still proceeds to **forget**, because skipping retention after a partly-successful backup is how snapshots accumulate without limit. A backup that saved *nothing* does stop the job, since running retention after it would delete old snapshots with no new ones in place. Operations that did not run are listed explicitly in the summary.

A local lock prevents the same job running twice on this machine, and is not taken by waiting: a second run is refused immediately rather than queued. It says nothing about other machines sharing the repository — that is a later milestone, and until it lands **prune** must not be run against a shared repository.

Exit status is **0** for success or partial, **1** for failure, **130** for interruption.

# SCHEDULE

**schedule** writes a systemd service and timer for a job; **unschedule** removes them; **status** reports what is installed on this host and what is deliberately not.

**-n** *JOB*
:   The job to act on. Optional for **schedule**, where omitting it installs units for every job declaring a `schedule:` block. Required for **unschedule** — removal is always named explicitly.

**--enable**
:   Also enable and start the timer. **Writing units and activating them are deliberately separate.** Installing a unit is inert; starting a timer adds another writer to a repository that may be shared with other machines, and that should be something you asked for rather than a side effect.

**--unit-dir** *DIR*
:   Write or read units in *DIR* instead of the systemd user directory. Intended for inspecting generated units without installing them; `systemctl` is not invoked when this is given.

Units are named `rusticprofile-`*JOB*`.service` and `.timer`, and both commands are idempotent: identical content is not rewritten, and removing units that are not there is not an error. Re-running **schedule** reports `unchanged` rather than implying work happened.

The generated timer sets **Persistent=true**, so a run missed while the machine was asleep is caught up rather than silently skipped, and **RandomizedDelaySec**, so several machines sharing one repository do not all wake on the same instant. Scheduling priority is expressed as `Nice=` and `IOSchedulingClass=` in the unit rather than applied in-process.

Units deliberately contain **no log path and no date**. A unit written today must not log to today's file forever, so `${date:...}` is resolved per run rather than at install time.

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

# SECRETS

rusticprofile stores no secrets and has no secret configuration. The repository password, master key and any cloud credentials belong to **rustic**(1), which offers `--password`, `--password-file` and `--password-command` (and `--key`, `--key-file`, `--key-command`), any of which may be set in its own config file.

Prefer **password-command**. rustic spawns that command itself, so the secret never enters rusticprofile's memory, argument list or environment. rusticprofile never emits a credential-bearing flag into a command line — a process list is world-readable, and rustic's own documentation warns that `--password` can expose the password there.

Anything rusticprofile prints is masked as a backstop. Masking is by variable name and distinguishes a secret from a location: `RUSTIC_PASSWORD` is hidden while `RUSTIC_PASSWORD_FILE` is shown, since a path reveals nothing and is usually the detail worth seeing. A `_COMMAND` variant is hidden, because such a command can embed the secret inline and nothing can distinguish that from a keyring lookup. The replacement marker is fixed, never a run of characters matching the secret's length.

# INTERPOLATION

Strings in the configuration may contain `${...}` references. This is a closed set, not a template language: there are no conditionals, functions, pipelines or loops. An unrecognised name is an error naming the offending key, never an empty string.

`${host}`, `${host_short}`, `${job}`, `${profile}`, `${config_dir}`, `${temp_dir}`, `${os}`, `${arch}`, `${env:NAME}` and `${date:FORMAT}` are recognised. `$${` produces a literal `${`.

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

# SEE ALSO

**rustic**(1), **systemd.timer**(5), **launchd.plist**(5)

# BUGS

Report issues at <https://github.com/l1a/rusticprofile/issues>.
