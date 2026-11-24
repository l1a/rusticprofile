# NOTES.md

Living state for **rusticprofile**: architecture, guidelines, operating invariants (§3a), current state, backlog (§4), and hard-won lessons (§5).
**Not a changelog:** Full pre-0.2.45 release log is `git show 366a984:NOTES.md`. Full design record & measurements (`§6` pre-0.2.68, folded from `PLAN.md`) now live in **`docs/DESIGN.md`** (original `PLAN.md`: `git show 366a984:PLAN.md`). Release/CI/packaging details live in **`docs/RELEASE.md`**.

---

## 1. Project Overview
- **Goal:** Local, per-machine scheduler and orchestrator for `rustic` backups (systemd, launchd, Task Scheduler) — no central server.
- **Stack:** `clap`, `serde`, `serde_yaml`, `toml`, `jiff`, `owo-colors`, `rustic` (delegated backend). GPL-3.0-or-later (`https://github.com/l1a/rusticprofile`).
- **Delegation boundary:** `rustic` owns repository, sources, excludes, retention policy, hooks, env, and metrics. `rusticprofile` owns scheduling, per-host gating, sequencing, exit classification, and lock coordination (`rustic -P <profile> <op> [--name <set>]... [--host/--filter-host] [--json on backup]`). Only read-only exception: parsing `rustic.toml` to validate `[[backup.snapshots]]` names and refuse unexpanded `sources` (`~` / `$`).

---

## 2. Codebase Architecture
Single crate (`src/lib.rs` + thin `src/main.rs`, no workspace).
- `cli.rs` (clap derive), `config/{job,hosts,interp,schedule,rustic_toml,example,validate,paths}.rs`
- `rustic/{invoke,exit,retention}.rs`, `exec/` (spawn, signals/JobObject, redaction), `run/{steps,lock,status,log}.rs`
- `report/`, `doctor/`, `schedule/` (systemd, launchd, Task Scheduler)
- `tests/cli_tests.rs`, `tests/golden/`, `docs/rusticprofile.1.md`, `scripts/`, `scripts/hooks/`

**Load pipeline (fixed order):** read -> parse YAML -> host gating -> `${...}` interpolation -> batched validation (`Violation` / `ValidationErrors`, exit `2`). Parsing before interpolation makes comment-templating bugs impossible; `${date:...}` resolves at runtime.

**Four `exec/` invariants:** stdout captured (`--json` objects) + stderr inherited (live progress); interrupts forwarded AND waited on; environment inherited unmodified; redaction is a backstop (secrets never enter this process).

---

## 3. Specific Development Guidelines
- **Quality & Pre-PR Gate:** `just check` runs `golden-is-current`, `standard-check`, `man-check`, `packaging-check`, `text-check`, `wip-check`, `fmt-check`, `lint` (`cargo clippy --all-targets -- -D warnings`) — all shell-free. Never call `gh pr create` directly (`just open-pr`). `scripts/hooks/pre-push` runs `just check`.
- **Man page:** Regenerate `docs/rusticprofile.1` with `just man` after every version bump (`just pr` checks cleanliness).
- **Version rule:** Every PR from `v0.1.0` to `v1.0.0` is a **patch** bump regardless of new features/CLI commands; minor bump = library API break (`schedule::Backend`) or milestone. Tag `v$VERSION` from clean `main` after `full-test` passes (`docs/RELEASE.md`).
- **Deliberate absences:** No `rustfmt.toml`, `clippy.toml`, `deny.toml`, `rust-toolchain.toml`, MSRV, `[lints]`, `#![deny(...)]`, `CHANGELOG.md`, `post-merge` hook, `benches/`, or `.cargo/audit.toml`.
- **Backup safety & redaction:** Never `restic prune` against a rustic-written repo; write tests to temp repos only; never leak hostnames, bucket names, project IDs, or home paths (`host-a`..`host-h`, `/home/user`, `example-project-000000`, `example-backup-bucket`).

---

## 3a. Operating Invariants — The Rules That Bite
*A backup that quietly does less than it says is worse than one that fails loudly. Evidence & measurements live in `docs/DESIGN.md`.*

### 1. Named Snapshot Sets MUST Group Retention by Label
`group-by = "host,label"` in `rustic.toml` (not `"host"`, where a 0-byte set overwrites a 6,256-file `core` set; and not rustic's default `"host,label,paths"`, where renaming a path mints a fresh retention bucket).

### 2. Exactly ONE Retention Authority per `(repository, host)`
Two tools may back up concurrently, **never `forget`** concurrently (competing `group-by: "host"` swept 395 MiB+ labelled snapshots). Always **disable outgoing retention BEFORE enabling incoming schedule**, and verify in the repo.

### 3. Exactly ONE Lock Protocol per Repository
**NEVER run `restic prune` against a repo any `rustic` client writes to** (measured: `restic prune` deleted 14 packs / 487.8 MiB under a frozen `rustic backup`, corrupting the repo). **`rustic prune` is safe** (lock-free 23h `--keep-delete` grace window unless `--instant-delete`). Completing fleet migration retires the unsafe row; M4 is defence in depth.

### 4. The Delegation Boundary
Jobs emit ONLY `rustic -P <profile> <op>`, `--json` (on `backup`), `--name` per set, and `--host`/`--filter-host` (unless `hostname: rustic`) — pinned by `rustic/invoke.rs` tests. Read-only passthroughs (`snapshots`, `check`) add no flags. Constructed `retention` (`forget --dry-run --json`) shares `build_argv` with `dry_run: true` hardcoded so destructive `forget` is unconstructible. Only read-only exceptions: validating `--name` and rejecting `~`/`$` in `sources` (rustic ignores unknown names when one valid name exists, and saves 0-byte snapshots on unexpanded relative paths).

### 5. Dangerous Decisions Live in `rustic.toml` (`config --example rustic`)
| Trap | Reference |
|---|---|
| `opendal:gcs` (no `gs:` scheme in rustic) | `docs/DESIGN.md §5` |
| Scoping filters belong in `[snapshot-filter]` (`[forget]` silently ignores them) | `docs/DESIGN.md §7` |
| `group-by = "host,label"` | Invariant 1 |
| Exclusion globs require leading `!` (bare pattern is an include filter) | `docs/DESIGN.md §6` |
| Missing absolute source fails the entire set (split optional paths into own sets) | `docs/DESIGN.md §6` |
| `filter-hosts` must match recorded hostname (`short` default) | `docs/DESIGN.md §6` |
| Exclude password file & cloud credentials from backup sets | `docs/DESIGN.md §3` |

### 6. Corollaries
- `jobs.yaml` is byte-identical fleet-wide -> only as new as the oldest binary/chezmoi checkout reading it (removing keys is fleet-safe; adding keys is not). `rustic.toml` is per-host generated.
- XDG paths apply on all platforms (Linux, macOS, Windows).
- `doctor` checks invariants 2 & 3 externally (ordering test: unlabelled snapshot newer than oldest labelled one; local `restic prune` schedule).

---

## Current State (v0.2.68)

*(Released version lives on GitHub Releases / crates.io / tags — never duplicated in prose.)*
- **Built:** Milestones 1, 2, 3, 5 complete (M6 effectively delivered; Windows + Task Scheduler landed in `0.2.0`); M4 deferred by decision. Shipped on GitHub Releases (6 targets), crates.io, AUR, COPR, Homebrew, and Scoop.
- **Commands:** `config --check`/`--show`/`--example` (hermetic); `plan`; `run` (locks, sequences, classifies, logs; `--background` retries failed ops 2×2min); `schedule`/`unschedule`/`status` (systemd, launchd, Task Scheduler; `status --json` `schema: 1`); `snapshots`; `retention`; `doctor` (exit `3` on warn).
- **Key behaviors:** `run` stops on failure and continues on partial (partial requires >= 1 parsed `--json` object); generated units embed absolute `rustic` path once (`doctor` `units-current` catches stale units); `defaults.hostname` (`short` default, `full`, `rustic`); alert on `last_success` in `status --json`.

---

## 4. Backlog
*Live items only — delete finished items rather than striking through.*

### 4.1 Product
- [ ] **AUR staleness detection:** Nothing flags when `aur-publish` was skipped (`SKIP_AUR`/non-Linux host).
- [ ] **Automated rustic measurement re-check:** `docs/DESIGN.md` measurements were verified against `rustic 0.11.4` manually; nothing automates re-checking on rustic upgrades.
- [ ] **Windows PowerShell completions path (`PS_COMP`):** `$PROFILE` under OneDrive doesn't source `~/.config/powershell`; probe `pwsh -NoProfile -c 'Split-Path $PROFILE'` or fall back to XDG.
- [ ] **Windows `permission: system` live test:** Unit-tested only; needs elevated run + credential placement design.
- [ ] **Windows automated argv round-trip test** in `tests/cli_tests.rs` (`CARGO_BIN_EXE_rusticprofile`).
- [ ] **Windows `status | more` broken-pipe panic** (no `SIGPIPE` on Windows).
- [ ] **Measure launchd resume race** (`launchd.plist(5)` wake coalescing).
- [ ] **Measure systemd boot catch-up delay (185–204s) and `NextElapseUSecRealtime` jitter** (for `(±N min)` annotation).
- [ ] **First criterion benchmark** (`benches/`, `[[bench]]`, config parsing).
- **Settled non-changes:** `retention` has no `--json`; `status --json` has no `next_run_iso`; `doctor` refuses chezmoi auditing; inert `network-online.target` directives stay commented (`docs/DESIGN.md §11`).

### 4.2 Tooling & Process
- [ ] **Pre-PR Checklist section in `AGENTS.md` Part 2** mirroring `retch` (`just pr` binds until added).
- [ ] **`just aur-verify` failure visibility:** Currently redirects `pacman` to `/dev/null`.
- [ ] **Docs-only CI path:** Needs a `changes` job reporting all required `build`/`audit` check names (never a `paths:` filter on required workflows); evaluate separate `cargo build` + `cargo test` compilation in `build`.

### 4.3 Fleet & Operations
- [ ] **Document out-of-band repository password recovery & key rotation** (excluded from backup by design).
- [ ] **Move GCS credential JSON into `~/.config/rustic/`** (`credential_path`), then drop root `~/.config` exception glob.
- [ ] **Finish moving passphrase to `~/.config/rustic/dot-files.pw.txt` on `host-e`, `host-c`, `host-g.local`**, then drop `stat` fallback (check via `rusticprofile snapshots`, not `config --check`).
- [ ] **Rung 9 control-group decision (`host-c`, `host-g.local`):** Disable dormant `Persistent=true` restic timer on `host-c` to avoid catch-up during prune window.
- [ ] **Audit `host-e`:** Confirm `host-e` vs `host-e.local` snapshots and regenerate its launchd plist with current generator.
- [ ] **Re-verify `host-f` (reinstalled Fedora):** Re-check schedule, secrets, and `gnupg` `enabled-on-hosts` gate.

---

## 5. Hard-Won Lessons

### 5.1 Verification & Oracles
- **Ask the defining oracle:** Build fixtures matching production (`backup --time` drops nanoseconds; test `PATH` lookup without explicit binary paths; `zsh -c` skips interactive `fpath`; `hostname(1)` != binary's hostname helper).
- **Watch guards fail from a clean baseline with sabotage count asserted:** `STAMP_FORMAT` single constant prevents self-consistent drift; check `git status` on its own line (`if [ -n "$(git status ...)" ]` swallows exit 128 under `set -e`); strip `GIT_CONFIG_COUNT`/`GIT_CONFIG_PARAMETERS` alongside `GIT_CONFIG_KEY_*`/`VALUE_*`.
- **Absence of evidence != evidence of absence:** Empty CI rollup is not green; unable-to-run checks must emit `unknown`; sweeps reporting 0 need positive controls; `--json` moves rustic confirmation from stderr to stdout.
- **No structural regex edits on safety files:** Comment `[forget]` matched and deleted `[snapshot-filter]`! Always run `config --check` after editing `rustic.toml`; audit `Justfile` via `just --show` and running recipes.
- *(Shared shell/oracle rules — `$?` after `| head`, `curl | sh`, `zsh MULTIOS`, `for id in $IDS`, `grep -c $'\r'`, `ConvertFrom-Json` locale — live in `~/AGENTS.md` & `~/.agents/docs/ops/rust-ci-packaging.md`.)*

### 5.2 Testing Conventions
- **Choke points over per-test discipline:** All CLI tests go through one `command()` redirecting `XDG_STATE_HOME`/`XDG_RUNTIME_DIR`/temps; `exec` tests use `run_locked` (`#[must_use]`). Throwaway verification jobs must use non-production names (avoid starving `dot-files` lock).
- **Pure path/env functions:** Avoid `unsafe std::env::set_var` (edition 2024); pass env/platform via params/`cfg!`.
- **Rustic integration tests (4):** Skip with notice when `rustic` is absent (covered in `aur-verify`).
- **Argv golden gate:** `plan --format lines` normalized across platforms pins the exact `rustic` contract; guard help-text/example invariants with behavior tests; keep `clippy --all-targets` on all OS legs.

### 5.3 `rustic`, as Measured (0.11.3 / 0.11.4)
- Non-clean exit is always `1` (even partial). `--json` outputs concatenated pretty-printed JSON (streaming parser needed; `program_version` lies).
- Unknown `--name` alongside valid name exits `0` silently. `~`/`$VAR` unexpanded -> relative path -> 0-byte snapshot exit `0` (missing absolute path fails set exit `1`).
- Scoping filters belong under `[snapshot-filter]`, ignored under `[forget]`. `group-by` defaults to `host,label,paths`. CLI `--host`/`--filter-host` overrides config file.
- `snapshots --json` uses `snapshots` key; `forget --json` uses `items` key (`label` omitted when unset). Dry-run `forget` is byte-identical; `forget` requires a keep rule (`keep-delete` alone fails).
- `rustic prune` deletes after 23h `--keep-delete` (repeating prunes always hold 1 marked generation — predict *live* pack count). Windows drive letter `C:/...` needs `local:C:/...`; bare `-P` checks `%APPDATA%` on Windows (`plan --format lines` shows resolved path).

### 5.4 Schedulers & Platforms
- **systemd:** `linger` user manager starts with `PATH=/usr/local/bin:/usr/bin` -> units hardcode absolute `rustic` path (`schedule` refuses otherwise). `ExecStart=` splits unquoted whitespace and expands `%`/`$` (`%%`, `$$`, refuse control chars/newlines). System units lack `XDG_RUNTIME_DIR` -> lock dir in temp must be `0700` owned real dir (not symlink). Reject newlines in set names / `${date:...}` (`%n`). Log files `0600` no-symlink; `doctor` warns on permissive cred files. `Persistent=true` ignores `RandomizedDelaySec` on catch-up (~5ms); user `network-online.target` does not exist; `INVOCATION_ID`/`JOURNAL_STREAM` are set in interactive terminals; `systemctl list-units` hides disabled units (`list-unit-files`); diff upgrades with `--write-only --unit-dir`.
- **launchd:** `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `PWD=/`; no `linger` (no backups at login window, no next-fire time); `enable` before `bootstrap`, `bootout` before `bootstrap`, `gui/<uid>` via `getuid()`. Mac nanoseconds are multiples of 1000 (`mod N` collides). Escape XML and gate with `plutil -lint`.
- **Task Scheduler (Windows):** Repeating trigger with past boundary runs immediately on registration -> hourly uses 24 triggers. `<Hidden>` hides UI entry, not window; `RestartOnFailure` retries launch failures only. `InteractiveToken` gets console -> `FreeConsole` + `CREATE_NO_WINDOW` (`FreeConsole` closes stdio -> never `Stdio::inherit()`, `os error 50`). Set priority normal, allow battery, remove 3-day limit, UTF-16LE + BOM, MSVCRT `<Arguments>` quoting, `KILL_ON_JOB_CLOSE` job object. `%COMPUTERNAME%` is uppercased NetBIOS -> `GetComputerNameExW`; honor `PATHEXT`. `schtasks` next-run is locale-ambiguous -> `Get-ScheduledTaskInfo` `.ToString('yyyy-MM-ddTHH:mm:sszzz')` (`RandomDelay` re-rolls on query). `schtasks /Delete /TN <folder>` does nothing -> `Unregister-ScheduledTask` (`-TaskPath` trailing `\`) + COM `DeleteFolder`. Config traps: `\U` escape in YAML/TOML, `/var/...` is relative on Windows, `HOME` unset, SSH enters `cmd.exe` (`-EncodedCommand`), `conhost.exe` count lies about visible windows.
- **Everywhere:** Restore default `SIGPIPE` (`status | head` panic on Unix); ignore relative/empty `XDG_*`; `jiff::TimeZone::try_system()` (never silent UTC `system()`); `Timestamp` `Z` vs rustic format.

### 5.5–5.7 Release, Fleet & Documentation Hygiene
- **Syncthing `.git` & `WIP.md`:** Never run git on two hosts simultaneously (`find . .git -name '*sync-conflict*'`; filter `git fsck` `dangling`/`bad sha1 file`; hash conflict copies before deleting; never trust stale chezmoi `--as-host` without `chezmoi update`).
- **Identifier sweeps:** Always run word-boundary `git grep -nwE` across code, commit, and PR body.
- **Duplicated state rots:** Point at the single authority instead of copying; update comparative claims ("the only route...") when adding options; rewrite `NOTES.md` cleanly rather than keeping superseded prose.
- **CI & Claude Code Review hardening:** `claude-code-review.yml` isolates PR head under `.ci-review/head/` with `persist-credentials: false` on both checkouts, refuses cross-repo PRs, strips instruction/config files from head, pins `--model claude-opus-5-5` + `--permission-mode default` + `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` + `use_sticky_comment: false`, and restricts `--allowedTools` to inline comments only; interactive `claude.yml` pins `--permission-mode acceptEdits`.
- *(Release, AUR, COPR, Homebrew, `Protect main`, `Claude Code Review`, and `Justfile` rules live in `docs/RELEASE.md` and `~/.agents/docs/ops/rust-ci-packaging.md`.)*
