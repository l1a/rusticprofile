# Release, Packaging & CI Reference (`rusticprofile`)

Loaded on demand before tagging, publishing, updating packaging templates/metadata, or editing CI/Claude-Review workflows.
*(See `~/.agents/docs/ops/rust-ci-packaging.md` for shared cross-repo CI, AUR, COPR, Homebrew, Scoop, and Claude Code Review invariants.)*

---

## 1. Pre-PR Documentation & Parity Checklists
- **Man page (`docs/rusticprofile.1.md` -> committed `docs/rusticprofile.1`):** Generated via `just man` (mandown). Never edit `docs/rusticprofile.1` directly; run `just man` after every version bump and commit it (`just pr` fails if dirty).
- **Templates & Metadata (`packaging/metadata.toml`):** Single source of truth for crates.io, AUR (`rusticprofile-bin`), COPR (`kentobias/rusticprofile`), Homebrew (`l1a/homebrew-rusticprofile`), Scoop (`l1a/scoop-rusticprofile`), and GitHub About (`just github-metadata`). Never commit concrete versions/checksums or literal `@TOKEN@` placeholders in comments.
- **No live infrastructure identifiers:** Public repo (`host-a`..`host-h`, `user-a`/`user-b`, `/home/user`, `example-project-000000`, `example-backup-bucket`). Run word-boundary sweep (`git grep -nwE ...`) across diff, commit message, and PR body before opening a PR (`WIP.md` holds the redaction map locally).

---

## 2. CI Ruleset (`Protect main`) & Required Checks
- `main` requires a PR (0 approvals, no force-push/deletion) with required checks: `build (fedora-x64)`, `build (macos)`, `build (ubuntu-arm)`, `build (windows)`, and `audit`.
- **Never require `full-test`, `build-release`, or `release`** (skipped on PRs), and **never add `paths:` filters to `rust.yml`/`security.yml`** (skipped required checks stall PRs at `Expected — waiting for status`).
- **Tag only after post-merge `full-test` is green on that exact commit** (`full-test` runs the release-shaped containers + rustic-backed integration tests skipped on PRs). Verify `git rev-parse main^{tree}` equals the tested PR tip's tree.
- **`Claude Code Review` (`claude-code-review.yml` + `.claude/commands/ci-code-review.md`):** Keep `CLAUDE_CODE_OAUTH_TOKEN` synced in Actions & Dependabot secrets; see `~/.agents/docs/ops/rust-ci-packaging.md` §1.

---

## 3. Releasing & Six-Channel Verification
- **Order:** Tag on clean `main` (after `full-test` is green) -> wait for `release.yml` & `copr.yml` -> `just publish` (crates.io -> AUR -> Homebrew -> Scoop; `SKIP_AUR=1` for split-host AUR) -> `just github-metadata`.
- **Note on AUR here:** `pkgver` tracks the released tag (legitimately trailing `Cargo.toml` between releases); `just aur-bump` takes bare version and `AUR_CONFIRM=y` non-interactively. Container tests in `aur-verify` install `rustic` so integration tests run.
- **Verify published crate 3 ways:** (1) crates.io API, (2) `cargo install --locked rusticprofile` into a throwaway root, and (3) **run that binary** (`--help`, version, etc.). Verify AUR via fresh `git clone` and COPR via `curl -L` per chroot. Never install fleet binaries from the synced working tree (`cargo install --path .`). While `README.md` documents `just install-tag vX.Y.Z` for external source builds, on crates.io + chezmoi-managed maintainer hosts (`~/AGENTS.md` §2) update via `cargo install-update` or `cargo install --locked rusticprofile` so `~/.cargo/.crates.toml` / `.crates2.json` and chezmoi-managed completions/man pages are not clobbered.
- **Verification Ladder:** Follow `docs/DESIGN.md §4` when validating against live repositories.
