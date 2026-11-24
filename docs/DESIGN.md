# Design Record (`rusticprofile`)

**Folded in from `PLAN.md` in `0.2.46`, and extracted from `NOTES.md §6` in `0.2.68`.** This is the architectural design record: why the tool is shaped this way, every rejected alternative, and the measurements against `rustic` behind `NOTES.md §3a`. Full pre-fold history: `git show 366a984:PLAN.md`.

**Decisions that move the delegation boundary, reverse a non-goal, or add a platform MUST be written here before writing the code.**

| `PLAN.md` | Section here |
|---|---|
| Part 1, Part 3, §2.1–§2.4 | §1 |
| Part 4 (schema and rules) | §2 |
| §4.1 | §3 |
| Part 4 (verification ladder) | §4 |
| §5.1–§5.4, §5.6, §5.8 | §5 |
| §5.7, §5.9, §7.1, §7.2 | §6 |
| §5.5, §7.3–§7.5 | §7 |
| §7.6 | §8 |
| §5.12, §7.7, §7.8, §7.14 | §9 |
| §5.10, §7.9, §7.13 | §10 |
| §5.11, §7.10, §7.12 | §11 |
| §7.11 | §12 |

---

## 1. Why a Scheduler, and Not a Port or a Wrapper

The request began as *"rewrite resticprofile in Rust"*. Two findings changed its shape:

- **A faithful port is neither tractable nor desirable.** `resticprofile` is ~30k lines + ~27k of tests, and config compatibility would mean reimplementing viper/mapstructure, Go `text/template` (with 22 custom functions), and HCL v1 before any backup logic.
- **`rustic` already owns most of what a wrapper would provide** — profiles (`-P`), hooks at four levels, env, metrics/OpenTelemetry, snapshot filters, and named snapshot sets. `rustic_scheduler` is client/server with an always-on central server, wrong for seven intermittently-online personal machines. **The unfilled gap is local, per-machine OS-level scheduling for `rustic`, with per-host variation and no central server.**
- **Four silent bugs in the predecessor's live setup motivated "validate loudly at load time":**
  1. Retention matched **0** snapshots for months (`backup.source` copied into retention `--path`, requiring snapshots to contain *all* listed paths).
  2. Retention never ran (**2,810 snapshots** accumulated under a ~49/host cap) because two absent sources made `restic` exit 3, treated as failure, aborting before retention.
  3. Config failed with `missing value for if` because templating ran before YAML parsing (`{{ if }}` inside a `#` comment still compiled).
  4. `.Env` scanning silently disabled (raw YAML pre-scan couldn't read structural template blocks), plus a `gcs: connections: 10` map silently dropped from flags -> **unknown keys are a hard error**.
- **No shell, ever.** Direct `std::process::Command` with an argv eliminates `sh -c` quoting bugs (guarantee is weaker on Windows, which has no argv — §10); only log output needs secret masking.
- **Rejected alternatives:** full resticprofile config compatibility; reading `profiles.yaml`; `migrate` subcommand; `restic` backend / vendored `commands.json`; `resticprofile --dry-run` as golden oracle; `rustic_scheduler`; contributing to abandoned `rustic-backup`.
- **Non-goals:** reading resticprofile config; `crond`; groups; **restore** (use `rustic` directly); hooks/metrics (`rustic` owns them); templating in any form; compatibility promises with `resticprofile`. (Windows was a non-goal until 2026-08-06, §10.)

---

## 2. The Config Schema's Rules
- **`enabled-on-hosts` removes the job entirely** on other hosts (matching short form for dotted hosts).
- **Parse first, interpolate second** (`NOTES.md §2`); interpolation has a closed variable set (`$${` for literal, no conditionals/functions/loops).
- **Every emitted `--name` is checked against `rustic.toml`** (§6), and **a job whose sets all resolve away on a host is an error**, never an empty run (`enabled-on-hosts: []` also refused).
- **Names are validated as declared, not as resolved** (catches typos behind other hosts' gates).
- **A set name may not start with `-`** (prevents argument injection via `--password` etc.).
- **Validation is batched:** every violation at once, exit `2` (distinct from backup failure `1`).

---

## 3. Secrets
- **`rusticprofile` holds no secrets and has no secret configuration.** Prefer `password-command` in `rustic.toml` `[repository]`: **`rustic` spawns it itself**, so the secret never enters `rusticprofile`'s memory, argv, or environment. Verified 2026-07-31 (`password-command` works and a wrong password fails).
- **Hard rules:** Never emit `--password`, `--key`, or any secret-bearing value into an argv; no shell.
- **Secret managers for unattended scheduled paths:**
  - `gnome-keyring` via `secret-tool`: fits Linux *user* timers once unlocked at login (not system units). Windows DPAPI master keys are excluded from backups by construction.
  - Proton Pass `pass-cli`: **rejected for scheduled paths** (PAT relocation + network round-trip + session expiry).
  - GCP key stays a plain `0600` file because `opendal` requires a path (whether inline `credential` works via `[repository.options]` is unverified).

---

## 4. The Verification Ladder
Strictly ordered against the live shared repository; each rung provably safe before the next:

| # | Action | Risk |
|---|---|---|
| 1 | `plan --format lines` — inspect argv | none |
| 2 | `--rustic-binary <shim>` — recording stand-in, exits 0 | none; rustic never runs |
| 3 | Read-only rustic: `snapshots`, `repoinfo`, `check` | none |
| 4 | `run --dry-run` | none; writes nothing |
| 5 | `forget --dry-run` scoped to this host, diffed vs predecessor | none |
| 6 | Full write path against throwaway local repo | isolated |
| 7 | Real backup (additive) | one extra snapshot |
| 8 | Real `forget`, prune disabled, smallest active host | first irreversible step |
| 9 | Fleet rollout | as documented |

Rung 5 caught that the predecessor's hand-run `forget` was not host-scoped (would have removed 337 other hosts' snapshots). Rung 9 stands at 5 of 7 hosts (`host-c` and `host-g.local` remain the untouched control group).

---

## 5. `rustic`: Repository Access, Exit Codes and Output
Measured against `rustic 0.11.3` (re-verified against `0.11.4` on 2026-08-24):
- **Schemes:** Only `local`, `rclone`, `rest`, `opendal` (no `gs:` alias — use `opendal:gcs`; no `-o` flag -> service params live in `[repository.options]` or `OPENDAL_*`). `rustic` reads restic-written repos fine.
- **Exit codes:** `0` = success, **`1` = everything else** (missing source, all missing, wrong password, missing repo — no warning tier).
- **Operations:** `backup` does not chain `forget` (only `forget --prune` chains prune); `--dry-run` exists on `backup`, `forget`, and `prune`.
- **`backup --json`:** Snapshot objects on stdout, diagnostics on stderr; objects are **concatenated pretty-printed JSON** (`grep -c '^{'` miscounts -> streaming parser required), and `program_version` does not report the binary version.
  - 1 good set -> exit `0`, 1 object; 1 good + 1 broken -> exit `1`, 1 object (partial); only broken / wrong password -> exit `1`, 0 objects.

---

## 6. Missing Sources, Unknown Names, Unexpanded Paths
- **`rustic` hard-fails a set on any missing absolute source** (exit 1, no snapshot saved), with no option to tolerate absent paths.
- **Settled 2026-07-30: Option B (named `[[backup.snapshots]]` sets selected per host).** `rustic.toml` owns every source; `rusticprofile` picks which sets run per host (rejected A: scattered per-host `rustic.toml`s; C: CLI paths). One broken set does not abort other sets (`1 of 2` saved, exit 1).
- **Unknown `--name` trap:** `--name core --name nosuch` exits **0** and silently ignores `nosuch` (only `--name nosuch` alone fails). Hence load-time validation against `rustic.toml`, and `!` prefix reminders on exclusion globs.
- **Unexpanded paths trap:** `rustic` expands neither `~` nor `$VAR` (nor `RUSTIC_FILTER_*`). Worse, `$HOME/Sync` is relative and skips the missing-source error -> saves a **0-byte snapshot, exit `0`**, winning its retention slot! Hence `check_sources_are_expanded`, per-host generated `rustic.toml`, untemplated `jobs.yaml`, and XDG paths across all OSes.
- **Hostname ownership (`0.1.34`):** `rustic` records `foo` on Linux and `foo.local` on macOS. For `--host` and `--filter-host`, CLI overrides config file -> `backup` gets `--host`, `forget`/`prune` get `--filter-host`, `snapshots` passthrough gets nothing (`--filter-host` unions). `defaults.hostname` supports `short` (default), `full`, or `rustic` (only for existing group preservation or domain collisions).

---

## 7. Retention Grouping & the Second Authority (`NOTES.md §3a` Invariants 1–2)
- Scoping filters under `[forget]` are silently ignored (no unknown-key error); they belong under `[snapshot-filter]`. `--group-by` defaults to `host,label,paths` (fragmentation let 2,810 snapshots survive); `--filter-paths` matches supersets.
- **Label grouping (`host,label`):** Under `group-by = "host"`, a 0-byte `nushell` snapshot finished last and kept its slot while deleting a 6,256-file `core` snapshot.
- **Second tool retention:** Predecessor's `group-by: "host"` swept our labelled snapshots (deleting a 395.591 MiB `core` snapshot in favor of a 0-byte one written 1s later), and vice versa (397.9 MiB deleted). Disabling outgoing retention BEFORE enabling incoming schedule is mandatory; `doctor` catches overlaps externally (§12).

---

## 8. Locks and Prune (`NOTES.md §3a` Invariant 3)
- `restic` uses repository-level `locks/` (shared for backup, exclusive for prune). `rustic` writes and checks **no** lock files (verified by freezing a rustic backup with `SIGSTOP`: `restic prune` deleted 14 packs / 487.78 MiB and corrupted the repo with 5 missing data packs; a second `restic backup` is not an exclusion control).
- **`rustic` is lock-free by design:** `rustic prune` marks packs and deletes only after `--keep-delete` (23 h default; `--instant-delete` deletes at once). Thus `rustic prune` + concurrent `rustic backup` is safe, and finishing the fleet migration retires the `restic prune` + `rustic backup` hazard.
- **If M4 is ever built:** Must write `restic`'s `locks/` format so hand-run `restic` / legacy tools honor it too.

---

## 9. Constructed Commands, Examples & Passthroughs (`NOTES.md §3a` Invariants 4–5)
- **`config --example <jobs|rustic>`:** Prints to stdout only (never writes files) with static placeholders (`host-a`, `/home/user`); tested through the real binary.
- **`snapshots` passthrough:** Read-only, appends caller args, inherits stdout/exit code (not a schedulable `Operation`). `check` would also qualify; `forget`, `prune`, `restore` never do.
- **`retention` (`0.2.27`):** Constructed read-only command (`forget --dry-run --json`) because `rustic forget --dry-run` computes retention winners (`items` key vs `snapshots` key; leaves repo byte-identical; refuses `forget` with no keep rule; `keep-delete` alone insufficient). Hardcodes `dry_run: true` through `build_argv` so non-dry-run form is unconstructible. Designed for restore-hunting (`--near DATE`, oldest snapshot per period) rather than listing redundant newest snapshots; `config --show` includes the delegated profile block directly.

---

## 10. Windows (`0.2.0`, Settled 2026-08-06)
- **In scope:** Windows builds/runs + Task Scheduler backend. Out: `crond`, `restic` backend, WSL-only backups.
- **Locking & Signals:** `share_mode(0)` file open (no `flock`); console `Ctrl+C` interactively, Job Object `KILL_ON_JOB_CLOSE` for scheduled runs.
- **Hostnames & PATH:** `%COMPUTERNAME%` is uppercased NetBIOS -> use `GetComputerNameExW`; honor `PATHEXT` for `rustic.exe`; `nix` lives under `cfg(unix)` deps; XDG paths used on Windows too; `quote_argument` is unit-tested against MSVCRT rules, with no automated argv round-trip test yet (`NOTES.md` §4.1).
- **Locale-free `next run`:** `status` queries `Get-ScheduledTaskInfo` via Windows PowerShell 5.1 (`-NoProfile -NonInteractive`, `.ToString('yyyy-MM-ddTHH:mm:sszzz')`, avoiding `schtasks` locale-ambiguous dates and `'o'` fractional seconds), displaying `(±5 min)` for `RandomDelay` and keeping `status --json` `next_run` untouched.

---

## 11. The Resume Race & Retry (`0.2.16`)
- **Measurements:** On Windows Modern Standby, `StartWhenAvailable` replays missed runs ~0.2s after wake before network returns. On systemd, `Persistent=true` catch-ups fire within 5–14 ms regardless of `RandomizedDelaySec` (0 / 300s / 3600s), ~11–12s before network becomes usable after `PM: suspend exit`.
- **Settled design:** Detached runs (`--background`, emitted by `schedule` on systemd & Task Scheduler, not launchd) retry a plain `Failure` **twice more at 2-minute intervals** (never `Interrupted`, `Partial`, or dry-run; no new `jobs.yaml` key; `run_job` signature unchanged; `INVOCATION_ID`/`JOURNAL_STREAM` are set in interactive terminals so `--background` is the explicit gate; no `network-online.target` wait so local repos work and non-network stalls retry).

---

## 12. `doctor` — What It Checks, and What It Refuses To (`0.2.13` / `0.2.48`)
- **Built checks:**
  1. Retention authority (`--repository`): **an unlabelled snapshot newer than the oldest labelled one** (not "mix of labelled and unlabelled", which stays red for 2 years of migrated restic history).
  2. Lock authority: live `restic prune` schedule on this host (installed-but-disabled is `ok` and noted).
  4. Credential files exist (`0600` check warns on group/other perms; missing file does not suppress warnings on another file).
  5. `units-current` (`0.2.48`): renders via `install::render_*`, reuses installed spread offset on launchd/Task Scheduler, treats comment-only diffs as `ok`, flags non-comment diffs as `warn` (exit `3`), and returns `unknown` under `--as-host`/`--rustic-binary` or missing `rustic`.
- **Rejected checks:** Stale chezmoi checkout (out of boundary/layering), pack accounting (stateless), secret-file permissions across OSes.
- **Third severity:** `ok`, `warn` (exit `3`), **`unknown`** (never report `ok` when unable to verify).
