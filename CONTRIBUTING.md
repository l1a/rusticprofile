# Contributing to rusticprofile

Thank you for considering a contribution to `rusticprofile`.

## Before you start

Two documents carry most of the context:

- **`PLAN.md`** — the full design, the reasoning behind it, every rejected alternative with its reason, and the measurements taken against real repositories. It is the design record, not a summary.
- **`NOTES.md`** — living project state, current milestone, and the release log. This project has no `CHANGELOG.md`; `NOTES.md` is it.

## How Can I Contribute?

### Reporting Bugs

- **Check for existing issues:** Before opening a new issue, please search the issue tracker to see if the problem has already been reported.
- **Use the Bug Report template:** Include your OS, your `rusticprofile` version and your `rustic` version.
- **Say if it was silent.** A backup that quietly covered less than configured, or a retention rule that matched nothing without complaining, is a higher-severity bug here than a crash. Crashes are obvious; silent degradation is what this project exists to prevent.
- **Redact.** Please strip credentials, bucket names and hostnames from any config you paste.

### Suggesting Enhancements

- **Check the delegation boundary first.** Repository, sources, excludes, retention policy, hooks, environment and metrics belong to rustic and are configured in `rustic.toml`. Requests for those belong upstream. This project owns scheduling, per-host gating, operation sequencing, exit classification and lock coordination.
- **Open a Feature Request** describing the desired behaviour and any edge cases.

### Submitting Pull Requests

1. **Fork the repository** and create your branch from `main`, named `{feature,fix,chore}/<name>`.
2. **Run the gate:** `just pr` runs the full pre-PR checklist. Use `just open-pr` rather than calling `gh pr create` directly — that recipe is the only thing that gates PR creation regardless of what tool is driving.
3. **Install the hooks once:** `just install-hooks`. `pre-push` runs `just check` before every push.
4. **Write a clear commit message:** imperative mood, 50 characters or fewer in the subject.
5. **Link related issues** in your PR description.

## Development Setup

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable — there is no MSRV and no `rust-toolchain.toml`, deliberately)
- [just](https://github.com/casey/just) (command runner)
- [mandown](https://crates.io/crates/mandown) (for generating the man page)
- [rustic](https://rustic.cli.rs) (to exercise anything that actually runs a backup)

### Build and Run

```bash
git clone https://github.com/l1a/rusticprofile.git
cd rusticprofile
just setup      # installs git hooks
cargo build
```

### Checking your changes

```bash
just check      # cargo fmt --check + cargo clippy -D warnings
just test
```

## Backup safety rules

Contributions are developed against real backup repositories, so these are not optional:

- **Read-only operations against a production repository are fine** (`snapshots`, `repoinfo`, `check`).
- **Every write test goes to a throwaway repository** under a temporary directory, deleted afterwards.
- **Never run `prune` against a shared repository** until lock coordination lands (`PLAN.md` M4).
- **Never delete snapshots** without explicit, per-step authorisation.

## Conventions worth knowing

Some things are missing on purpose, and their absence is the convention: there is no `rustfmt.toml`, `clippy.toml`, `deny.toml`, `rust-toolchain.toml`, MSRV declaration, `[lints]` table or `CHANGELOG.md`. Please do not add them.

`just` is the only task runner. Enforcement lives in real git hooks and Justfile recipes rather than in any editor- or agent-specific configuration, so that it binds every contributor and every tool equally.

## Licensing

By contributing to `rusticprofile`, you agree that your contributions will be licensed under the project's **GPL-3.0-or-later License**.
