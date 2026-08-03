# AI Agent Guidelines (AGENTS.md)

Welcome! This file contains project-specific guidelines, constraints, and instructions for
all AI assistants (Gemini, Claude, etc.) contributing to the **rusticprofile** project.

This file has two parts:

- **Part 1 — Portable Core**: rules that are identical across all of Ken's repos using this
  pattern (currently `etr`, `retch` and `rusticprofile`). If you change wording here,
  propagate the same change to the Portable Core section in sibling repos so they stay in sync.
- **Part 2 — Project-Specific**: rules that only make sense for `rusticprofile`.

---

# Part 1 — Portable Core

## STEP ONE — READ EVERY INSTRUCTION FILE IN FULL BEFORE ANYTHING ELSE (NO EXCEPTIONS)

This section is not optional, is not subject to the agent's judgment, and comes before
every other instruction in this file, in every other instruction/memory-class file, and
before any code is read for review, reviewed, generated, or edited, and before any shell
command is run other than one needed to locate or read the files this section describes.

**This applies to every "instructions/rules/memory" class file that exists for this
repository or session**, regardless of its name or format — including but not limited
to: `~/AGENTS.md`, this file (`AGENTS.md`), `CLAUDE.md`, `NOTES.md`, `WIP.md`, any file a
skill points to, and any other file whose purpose is to carry standing instructions,
context, or state rather than being source/product code.

1. **Read the ENTIRE file, every time, before doing anything else.** Not the first N
   lines, not a `head`, `grep`, or table-of-contents skim, not "the parts that look
   relevant to this specific request." Read the whole file, start to finish, at the
   start of the session. A partial read produces the same downstream mistakes as no
   read and is not an acceptable substitute for a full read.
2. **Follow what you read without first deciding whether it's "worth it," "necessary,"
   or "applicable" to the task in front of you.** "This looks like a small change" or
   "this rule probably doesn't apply here" are not valid reasons to skip or defer a
   documented instruction. If a documented rule genuinely conflicts with the current
   request, surface the conflict to the user explicitly — do not silently decide it
   doesn't apply and proceed.
3. **This step must complete, in full, before any other work.** Do not begin reviewing
   code, generating code, editing files, or running exploratory/state-changing commands
   until every applicable instruction file for this session has been read in full —
   this includes reading `PLAN.md` per Part 2 §0 below.
4. **This mandate is agent-agnostic.** It applies identically regardless of which coding
   agent or tool is operating (Claude Code, Gemini CLI, or any other). Enforcement
   mechanisms for any *specific* rule in these files should likewise be built to work the
   same way no matter which agent is driving — prefer real git hooks and Justfile
   recipes (e.g. `scripts/hooks/pre-push`, `just open-pr`) over anything under `.claude/`
   when the goal is "block an action no matter what tool is driving." `.claude/` config
   only binds Claude Code and is invisible to every other agent and to a human typing
   commands directly.

## 0. Global Mandates
Before doing anything else in a session, read `~/AGENTS.md` (and any skill files it
references) if it exists on the current machine. It carries standing mandates that
apply across all of Ken's repos and are not repeated here — e.g. the chezmoi
native-command hierarchy, the `[REASONING TRACE]` requirement, and language
requirements. If `~/AGENTS.md` conflicts with this file on a repo-specific detail
(e.g. a project's own branch-naming or checklist convention), this file wins for
that detail; `~/AGENTS.md`'s cross-cutting mandates still apply.

## 1. Source Control & Commit Workflow
* **Branch Naming:** Always name new git branches using the prefix pattern `{feature,fix,chore,etc.}/<branch-name>`.
* **Workflow Mandate:** You MUST create and switch to your feature/fix branch *before* starting any file modifications or executing commands to avoid working on `main` by mistake.
* **Commit Summaries:** Write short, clear subjects (max 50 chars) in the imperative mood.
* **AI Attribution:** Use `Assisted-By: <model name>` (no email address) as the trailer line in commits. Use the actual model name of the AI assistant that helped (e.g. `Gemini 3.5 Flash`, `Claude Sonnet 4.6`, `Claude Opus 4`, etc.).
* **Constraint:** NEVER run background `git commit` or `git push` without explicit authorization.
* **Mandate:** ALWAYS ask for explicit permission before submitting a Pull Request (PR) or performing a merge.
* **Branch Cleanup:** Delete feature branches from the remote after they are merged. Periodically prune abandoned branches that were never PRed.

## 2. Engineering Philosophy & Safety
* **Cognitive Circuit Breaker:** Before modifying files or running commands, identify if target files are managed by `chezmoi` (except if located in `~/git` or `~/Sync/git`). If managed, prioritize chezmoi native commands.
* **Chezmoi — do not improvise, ever:** Read `~/.gemini/skills/chezmoi-manager/SKILL.md`
  in full before running any `chezmoi` command or touching any chezmoi-managed file
  (`chezmoi managed <path>` to check), if it hasn't been read yet this session — this
  applies even when the target path is under `~/git`/`~/Sync/git` and the circuit-breaker
  preamble above is exempted; the skill itself is not optional once a file is confirmed
  chezmoi-managed. This is not hypothetical caution: it has already gone wrong twice —
  once badly enough that the chezmoi source git repo desynced and required multiple
  reverts and cherry-picks to recover, and again when `chezmoi add` was run without
  `CHEZMOI_COMMIT_MSG` and hung on an interactive prompt with no TTY to answer it. Both
  mistakes, and their fixes, are already documented in the skill and don't need
  rediscovering by trial and error:
  - Modify at the destination path, then `CHEZMOI_COMMIT_MSG="message" chezmoi add
    <destination-path>` — never bare `chezmoi add`.
  - Never `chezmoi git -- add/commit/push`; prefer native `chezmoi status`/`chezmoi diff`
    over raw `git -C <chezmoi-source-path> ...` even for read-only checks. The
    `ALLOW_CHEZMOI_GIT=1` git-hook bypass is "human only" — never set it yourself.
  - For `.tmpl` files, use `chezmoi edit <destination>` with `VISUAL`/`EDITOR` set to a
    non-interactive command (e.g. `cp`), never an interactive editor.
  - One improvised command succeeding is not evidence improvisation is safe — the base
    rate includes the desync incident above.
* **Absolute Accuracy:** Absolute accuracy is the primary metric. Speed is irrelevant.
* **The Reasoning Trace:** Before implementing any multi-file change, you MUST output a `[REASONING TRACE]` covering Invariants, Subsystem Impact, and Edge-Cases.
* **Empirical Validation:** Test changes locally (compilation, lints, formatting, and unit tests) before proposing a push. See Part 2 for this project's Pre-PR Checklist and automated gate once scaffolding exists.

## 3. Cross-Machine Work Handoff (WIP.md)
Any agent starting a session on a repository utilizing cross-machine sync MUST read `WIP.md` before doing anything else.
* **Purpose:** `WIP.md` is a `.gitignored` file synced via Syncthing/Insync to carry context that cannot be inferred from git history alone (what is partially done, machine specs, active branch, next-step checklists, caveats).
* **When to Update:**
  * When switching to a new branch (clear old content, write new context).
  * Before switching machines or ending a session.
  * After pushing commits that change the state of the work.
  * After a PR is merged (set `Active Branch: none (main is current)`).
  * Whenever the next-step checklist changes.
* **What to Include:**
  1. **Machine**: OS, distro, and architecture of the last saved state (e.g. `Linux Fedora 44 x86_64`).
  2. **Active branch name** and PR URL (if open).
  3. **Latest commit hash** and message.
  4. **What was implemented**: Concise description of new/modified files.
  5. **Bugs fixed**: What went wrong and how it was resolved.
  6. **Current CI state**: Passing/failing.
  7. **Open tasks**: Checkbox list of remaining work.
  8. **How to resume**: Exact shell commands to check out, build, and verify.
  9. **Why this work**: Motivating context.
* **What NOT to Include:** Full code diffs, large file contents, detailed architecture docs.

## 4. Continuous Learning Loop
At the conclusion of any task involving a specific skill:
1. Did you encounter a failure, edge case, or nuance not currently documented in the skill?
2. Did the user have to correct your workflow?
3. If YES to either, you MUST automatically update the corresponding `SKILL.md` file with the new learning and synchronize the change before declaring the task complete.

**Learnings belong in `AGENTS.md` (this file, `~/AGENTS.md`, or the relevant
`SKILL.md`), never in an agent-specific memory/preference store.** Tools like Claude
Code's auto-memory or Gemini's session memory are invisible to every other agent, and to
a fresh session of the *same* agent elsewhere. Saving a correction only there means the
next agent — or the same agent next time — re-learns it the expensive way. If an
agent-specific memory feature also gets used as a convenience cache, the durable copy of
the learning MUST still land in `AGENTS.md`/`SKILL.md` in the same turn, not deferred.
This file exists specifically so Claude, Gemini, and any other agent working here read
the same standing knowledge — an agent-specific memory entry does not satisfy that.

---

# Part 2 — Project-Specific: rusticprofile

## 0. Start of session — REQUIRED READING

**Read `PLAN.md` in full before anything else in this repository.** It is not a summary —
it is the complete design, the reasoning behind it, and the record of what was already
tested. Nothing here will make sense without it, and re-deriving it is expensive: it took a
long session including three parallel codebase explorations and live testing against a
production backup repository.

`PLAN.md` is structured as:

| Part | Contents |
|---|---|
| 1 | How the design was reached — two pivots that changed the project's shape |
| 2 | Discoveries worth keeping, with `file:line` references |
| 3 | Decisions, and every rejected alternative with its reason |
| 4 | The plan: schema, architecture, milestones, verification ladder |
| 5 | Prerequisite **test results** against rustic 0.11.3 and the live GCS repo |
| 7 | The absent-sources decision — **settled 2026-07-30 (option B)**, with the tests behind it |
| 8 | Related state in other repositories |

Companion document, for the Go tool this project descends from:
`~/Sync/git/resticprofile/UPSTREAMING.md`.

## 1. Current state

**Pre-code.** Nothing is implemented. The repository contains this file, `CLAUDE.md`,
`.gitignore` and `PLAN.md`. Scaffolding is step 1 of Milestone 1 (`PLAN.md` §4).

**The Part 7 decision is settled** (2026-07-30): **option B**, named `[[backup.snapshots]]` sets
selected per host. rusticprofile owns *no* part of the backup source list — `rustic.toml` keeps
every path, and jobs list which named sets run on which host. Code is unblocked; Milestone 1 step 1
(scaffolding) is the next action.

One consequence to know before touching `config/`: rustic **silently ignores an unknown `--name`**
whenever at least one valid name is also given (exit 0, no diagnostic), so rusticprofile validates
every name it emits against `rustic.toml` at load time, and a job whose sets all resolve away on a
host is a load-time error rather than an empty run. `PLAN.md` §7.2 has the measurements.

## 2. What this project is

A **local, per-machine scheduler and orchestrator for `rustic`** — systemd/launchd units,
per-host job gating, run sequencing, exit classification and lock coordination. Backup
configuration itself is delegated to rustic's own TOML config.

It is explicitly **not** a config wrapper: rustic already provides profiles, hooks, forget
policies and Prometheus metrics natively. It is also not `rustic_scheduler`, which exists
but is client/server with a central always-on server — the wrong architecture for a fleet
of intermittently-online personal machines. See `PLAN.md` Part 1.

## 3. Safety rules — a live backup repository is involved

The GCS repository `opendal:gcs` / `gs:example-backup-bucket:/dot-files` is **in production**,
shared by 7 machines, and currently holds the only copy of several years of data.

* **Read-only operations against it are fine** (`snapshots`, `repoinfo`, `check`).
* **Every write test goes to a throwaway local repository**, under a temp dir, deleted after.
* **Never run `restic prune` against the GCS repository while any host backs up with
  rustic.** Measured (`PLAN.md` §7.6): restic deletes packs immediately, which is safe only
  because it holds an exclusive repository lock — and rustic neither takes nor honours that
  lock. A restic prune against a rustic writer deleted 14 packs mid-backup and left the
  repository failing `restic check`.
* **`rustic prune` is safe and is the only prune that may run here.** rustic is lock-free by
  design: prune *marks* packs and deletes them only after `--keep-delete`, 23 hours by
  default, so a concurrent rustic backup has a day of grace. Verified — a default
  `rustic prune` left every pack on disk; only `--instant-delete` removed them.
  * This **supersedes the previous rule**, which said no prune at all until M4. That rule
    was written before rustic's own documentation was read, and it was wrong in a way that
    cost real time: it disabled the fleet's prune schedule for no reason. M4 is defence in
    depth, not permission.
* **Never delete snapshots** on any host without explicit per-step authorisation. Follow the
  verification ladder in `PLAN.md` §4 in order; it is designed so each rung is provably safe
  and the first irreversible step has the smallest possible blast radius.
* The **three** idle hosts (`host-c`, `host-e.local`, `host-g.local`) are the control group.
  Leave them alone. They are the only hosts whose snapshot counts still mean anything as a
  baseline, because nothing has run against them since 52, 140 and 269 days ago respectively.
* `host-f` is **not** in the control group, despite an earlier version of this list saying
  so. It is the development machine — the one this work is done on, and the one every
  throwaway repository and test run has been created from. A control group containing the
  machine being experimented on is not a control group.

## 4. Conventions

This project follows the same scaffolding conventions as `~/git/retch` and `~/git/etr`,
documented in full in `PLAN.md` §2.5. In particular: `just` is the only task runner; the
`check`/`pr`/`open-pr`/`merge-pr` gate triad; `NOTES.md` is the changelog and living state,
not a `CHANGELOG.md`; version bump on every PR — but **`0.0.x` only until Milestone 1 ships a
tool that can actually run a backup, since `v0.1.0` is reserved for that**, so bump the patch
component even for features; and the deliberate *absence* of
`rustfmt.toml`, `clippy.toml`, `deny.toml`, `rust-toolchain.toml` and an MSRV is itself the
convention — do not add them.

Once scaffolding exists, Part 2 should gain a Pre-PR Checklist section mirroring retch's
§4, and `NOTES.md` becomes required reading alongside `PLAN.md`.

## 5. Opening pull requests

Use **`ghpub`**, not `gh`, for any PR against a repository this account does not own. The
default `GH_TOKEN` is a fine-grained PAT, and only classic PATs can write to repositories
you do not own — `gh pr create` fails with
`Resource not accessible by personal access token`. Reads work fine, which makes the failure
look unrelated to auth. See `~/AGENTS.md` §3.
