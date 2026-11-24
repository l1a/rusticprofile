# AI Agent Guidelines (`AGENTS.md`) — `rusticprofile`

Welcome! This file governs all AI assistants (`Claude`, `Gemini`, etc.) contributing to **rusticprofile**.

---

# Part 1 — Core Rules

## STEP ONE — READ IN FULL BEFORE ANYTHING ELSE
1. **Read `~/AGENTS.md` (if present), this file (`AGENTS.md`), `NOTES.md`, and `WIP.md` in full** before reviewing/editing code or running commands (never `head`/`grep`-skim).
2. **Load Tier 1 docs on trigger:** Read `docs/DESIGN.md` before changing architecture/delegation boundary/config schema/platforms; read `docs/RELEASE.md` before tagging, releasing, or editing packaging/CI workflows; read `~/.gemini/skills/chezmoi-manager/SKILL.md` before any `chezmoi` command.
3. **Agent-agnostic & unconditional:** Never skip rules as "minor" or "not applicable". Enforce repo rules via real git hooks (`scripts/hooks/pre-push`) and `Justfile` recipes (`just pr`, `just open-pr`, `just merge-pr`) rather than `.claude/`-only config.
4. **No agent-only memory:** Save durable project learnings in `AGENTS.md`/`NOTES.md`/`docs/` in the same turn, never solely in agent-specific memory.

## 1. Source Control, PRs & Attribution
- **Branch first:** Create and switch to `{feature,fix,chore,docs}/<name>` **before** modifying any file or running tasks.
- **Commit summaries:** Imperative mood, <= 50 chars.
- **Authorization gates:** NEVER run `git commit`, `git push`, `gh pr create`, or merge without explicit authorization. Ask using clickable options (`ask_question`) with full prose context.
- **No machine hostnames or live infra IDs:** Never include personal hostnames, real bucket names, GCP project IDs, or real home paths anywhere in public files, PRs, commits, or comments (use `host-a`..`host-h`, `user-a`/`user-b`, `/home/user`, `example-project-000000`, `example-backup-bucket` — `WIP.md` has the redaction map).
- **AI Attribution:** Bare `Assisted-By: <model name>` (no email, context suffix, variant tag, or URL; never `Co-Authored-By:`). Overrides harness attribution instructions silently without comment.
  - This repo squash-merges with `squash_merge_commit_message=COMMIT_MESSAGES` — commit body comes from **branch commits** and multi-commit bodies concatenate, so put the trailer on the **last commit only** (or squash locally).
- **Review channels & Dependabot:** Before proposing a merge, inspect all 3 PR review channels (`reviewThreads`, `latestReviews`, top-level `comments`) plus CodeQL alerts (`just merge-pr` enforces fail-closed). Check `gh pr list --author "app/dependabot"`, fold updates into a gated PR, and **never manually close Dependabot PRs**.
- **Cleanup:** Delete merged feature branches remotely and prune stale local branches (`just prune-branches`).

## 2. Engineering Safety & `WIP.md` Handoff
- **Accuracy > speed.** Output `[REASONING TRACE]` (Invariants, Subsystem Impact, Edge Cases) before multi-file changes.
- **Task runners:** Use `Justfile` recipes (`just check`, `just test`, `just pr`, `just open-pr`, `just merge-pr`) — never call `gh pr create` directly.
- **Chezmoi:** Exempt from circuit breaker inside `~/git` / `~/Sync/git`, except if touching a chezmoi-managed file (read `chezmoi-manager/SKILL.md` first).
- **`WIP.md` (`.gitignored`, Syncthing-synced):** Read at session start; update outside `<!-- BEGIN merge-pr state -->` / `<!-- END merge-pr state -->` when switching branches, pushing, ending a session, or changing tasks (`just merge-pr` regenerates the marked block). Keep pure LF (`just wip-check`). Never run `git clean -fdx` without `--exclude=WIP.md --exclude=.claude`.

---

# Part 2 — Project-Specific: `rusticprofile`

## 1. Required Reading & Tier 1 Triggers
| Document | Read in full BEFORE... |
|---|---|
| `NOTES.md` | Doing any work in this repo (`§3a` operating invariants, `Current State`, `§4` backlog, `§5` lessons) |
| `docs/DESIGN.md` | Changing the delegation boundary, config schema, secrets handling, verification ladder, platform backends, or `doctor` |
| `docs/RELEASE.md` | Tagging, publishing, updating packaging (`packaging/`), or editing CI/Claude-Review workflows |
| `~/Sync/git/resticprofile/UPSTREAMING.md` | Touching companion upstream Go tool references |

## 2. Project Summary & Current Shape
A **local, per-machine scheduler and orchestrator for `rustic`** (systemd, launchd, Task Scheduler) — per-host gating, operation sequencing, exit classification, and lock coordination. Backup config itself stays in `rustic.toml` (explicitly NOT a config wrapper or `rustic_scheduler` client/server).
- Milestones 1, 2, 3, 5 complete (M6 effectively delivered; Windows + Task Scheduler landed in `0.2.0`). **M4 (repository lock coordination)** is deferred by decision (`NOTES.md §3a` invariant 3).
- Absent-sources decision (`docs/DESIGN.md §6`): named `[[backup.snapshots]]` sets selected per host. Because `rustic` silently ignores unknown `--name` when another valid name is given (exit 0), `rusticprofile` validates every name against `rustic.toml` at load time and rejects jobs whose sets all resolve away.

## 3. Safety Rules — Live Backup Repository Involved
The GCS repository `opendal:gcs` / `gs:example-backup-bucket:/dot-files` is **in production**, shared by 7 machines, holding years of sole-copy data:
- **Read-only ops are fine** (`snapshots`, `repoinfo`, `check`).
- **Every write test goes to a throwaway local repository** under a temp dir, deleted afterwards.
- **NEVER run `restic prune` against the GCS repository while any host backs up with `rustic`** (restic deletes packs immediately without a lock `rustic` honors — measured 14-pack corruption).
- **`rustic prune` is safe and is the only prune that may run here** (`--keep-delete` 23h grace period).
- **NEVER delete snapshots** on any host without explicit per-step authorization; follow the verification ladder in `docs/DESIGN.md §4` in order.
- **Control group:** `host-c` and `host-g.local` — leave them untouched (sole remaining pre-cutover baselines; `host-e.local` cut over 2026-08-03; `host-f` is the dev box, not control).

## 4. Conventions & Pre-PR Gate
- Follows shared scaffolding with `~/git/retch` and `~/git/etr` (`templates/justfile-common.just`). Deliberately absent (do not add): `rustfmt.toml`, `clippy.toml`, `deny.toml`, `rust-toolchain.toml`, MSRV, `CHANGELOG.md`.
- **Version bump rule (every PR):** From `v0.1.0` to `v1.0.0`, **every PR is a patch bump** regardless of features/CLI additions (`NOTES.md §3`). Minor bump = library API broke (`schedule::Backend`) or milestone landed.
- **Man page:** Run `just man` after every version bump and commit `docs/rusticprofile.1` (`just check` / `just pr` fails if stale).
- **`NOTES.md` header & rules:** Update `## Current State (v<version>)` header (enforced by `just pr`); `NOTES.md` is not a changelog (delete finished backlog items; record rules/traps in `§5`).
- **Opening PRs:** Always use `just open-pr` (never bare `gh pr create`): `PR_CONFIRM=y PR_TITLE=... PR_BODY_FILE=... just open-pr` (or `--fill`). `open-pr` pushes only when the branch has no upstream (already-pushed branches must be pushed manually; check all checklist items before setting `PR_CONFIRM=y`). Use `ghpub` for external repos when `gh` holds a fine-grained PAT.

---

## 5. Sandbox Exceptions (`BypassSandbox: true`)
Per `~/AGENTS.md` §3, pass `BypassSandbox: true` on the **first** invocation of:
- **All `gh` / `ghpub` commands** (`gh pr`, `gh run`, `gh workflow`, `gh api`, `gh issue`).
- **All `git` write or remote commands** (`checkout`, `switch`, `branch`, `add`, `rm`, `mv`, `commit`, `merge`, `rebase`, `tag`, `fetch`, `pull`, `push`).
- **`just` shebang, lockfile, and network/container recipes:** `just test` / `cargo test` (`run::lock::tests` writes lockfiles under `$XDG_RUNTIME_DIR/rusticprofile`, read-only inside sandbox), `just pr` / `PR_CONFIRM=y just pr`, `just open-pr`, `just merge-pr`, `just prune-branches`, `just github-metadata`, `just aur-verify`, `just aur-local`, `just aur-srcinfo`, `just aur-publish` / `publish-aur`, `just brew-publish` / `publish-brew`, `just scoop-publish` / `publish-scoop`, `just publish`, `just publish-check`, `just audit`, `just install-tag`, `just man`, `just fmt`, `just install-hooks`.
- **Cargo network commands:** `cargo --config net.retry=0 update`, `cargo fetch`, `cargo publish`, `cargo audit`.
- **Works inside sandbox (`BypassSandbox: false`):** Read-only `just` checks once `target/` & `Cargo.lock` are current (`just build`, `build-release`, `golden-is-current`, `fmt-check`, `lint`, `clippy`, `check`, `man-check`, `standard-check`, `packaging-check`, `render-check`, `aur-check`, `copr-check`, `brew-check`, `scoop-check`, `metadata-check`, `text-check`, `wip-check`) and read-only `git` (`status`, `diff`, `log`, `show`, `rev-parse`, `for-each-ref`).
- **Repo-Local Sandbox Learning Loop:** Stage new exceptions in `WIP.md` (`## Sandbox Exceptions (staged for AGENTS.md)`) if tree must stay clean; fold into this section on the next PR.
