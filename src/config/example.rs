// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Annotated starting-point configurations, emitted by `config --example`.
//!
//! These are the findings in `PLAN.md` Part 5 and Part 7 shipped as something executable
//! rather than something to be read and reimplemented. Almost everything that can silently
//! destroy data in this design lives in *rustic's* config, not ours, so the `rustic`
//! example carries considerably more weight than the `jobs` one.
//!
//! Three properties, each deliberate and each with a test:
//!
//! - **Static text.** Nothing is interpolated — not the hostname, not `$HOME`. Placeholders
//!   are this project's own redaction vocabulary (`host-a`…`host-h`, `/home/user`), so the
//!   output cannot be pasted into place and left unread. A config that appears to work is
//!   one nobody checks, and every value in these files needs checking once.
//! - **Emitted to stdout**, like `--completions`. Writing the file is the user's explicit
//!   act. The thing this would otherwise overwrite is what stands between a fleet and its
//!   backups.
//! - **Valid.** `tests/cli_tests.rs` writes both examples out and validates them through
//!   the real binary. An example that has drifted out of step with the validator is worse
//!   than no example, because it is quoted with authority.
//!
//! When a validation rule changes, these change with it, and the test is what says so.

/// Which annotated example to emit.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ExampleKind {
    /// `jobs.yaml` — what rusticprofile itself owns
    Jobs,
    /// `rustic.toml` — the delegated backup configuration, with the traps annotated
    Rustic,
}

impl ExampleKind {
    /// The annotated example text, ready to write to stdout.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Jobs => JOBS_YAML,
            Self::Rustic => RUSTIC_TOML,
        }
    }
}

/// Annotated `jobs.yaml`.
///
/// Deliberately short. rusticprofile owns *when*, *which* and *where* — if this file is
/// getting long, something that belongs to rustic has probably leaked into it.
const JOBS_YAML: &str = r##"# ~/.config/rusticprofile/jobs.yaml
#
# What rusticprofile owns: which jobs exist, what operations they run in what order, on
# which hosts, and when. That is all.
#
# What is deliberately NOT here, and belongs in rustic's own config: the repository,
# source paths, exclusions, retention numbers, hooks and credentials. rusticprofile
# constructs no backup flags — a job becomes exactly
#     rustic -P <profile> <operation> [--name <set>]...
# and nothing more. If you find yourself wanting to add a backup setting here, it already
# exists in rustic.toml; see `rusticprofile config --example rustic`.
#
# Placeholders below use host-a/host-b and /home/user. Replace them. Nothing in this file
# is substituted for you, on purpose — every value here is worth reading once.

schema: 1

defaults:
  # The job to act on when a command is given no `-n`.
  #
  # Applies to `run`, `plan`, `snapshots` and `config --show`.
  #
  # Deliberately NOT to two commands:
  #   unschedule  removal is always named explicitly. Deleting a schedule because a config
  #               file said so, rather than because you typed its name, is the one action
  #               that should never happen by default.
  #   schedule    omitting `-n` there already means "every job that declares a schedule",
  #               which is useful and would be lost.
  default-job: dot-files

  # Where the rustic profiles live. `profile: dot-files` below resolves to
  # <this dir>/dot-files.toml, and rusticprofile passes it to rustic as an absolute path:
  # a bare profile name would make rustic search its own paths, which need not include the
  # directory that was just validated.
  rustic-config-dir: "${env:HOME}/.config/rustic"

jobs:
  # A backup followed by retention. This is the ordinary shape.
  dot-files:
    profile: dot-files
    # Ordered, and the order carries the most important rule in the tool: a run stops on
    # failure but CONTINUES on partial. `forget` therefore still runs after a backup that
    # partly succeeded, and does NOT run after one that saved nothing. Aborting retention
    # whenever rustic exits non-zero is how a fleet accumulates thousands of snapshots
    # under a policy that should cap them near fifty.
    operations: [backup, forget]

    # Named sets from rustic.toml's [[backup.snapshots]] entries. Two reasons they are
    # listed individually rather than "back up everything":
    #
    #   1. rustic fails an ENTIRE set if any one of its sources is missing, with no
    #      opt-out — so a path that exists on only some machines has to be its own set, or
    #      one absent directory takes the whole run down.
    #   2. rustic SILENTLY IGNORES an unknown --name whenever at least one valid name is
    #      also given: exit 0, no diagnostic. rusticprofile therefore validates every name
    #      against rustic.toml at load time. This is the one place it reads that file.
    snapshot-sets:
      # Present everywhere, so no gate.
      - name: core

      # Present on some hosts only. `config --check --as-host <name>` shows what any host
      # resolves to without logging into it.
      - name: gnupg
        enabled-on-hosts: [host-a, host-b]

    # Declaring a schedule here installs nothing on its own -- `rusticprofile schedule -n
    # dot-files` is what writes the units and arms the timer, and `unschedule` fully undoes
    # it. Add `--write-only` to install them inert and read them first.
    #
    # A scheduled job becomes TWO systemd units: a .timer and the .service it activates.
    # systemd has no way for a timer to run a command directly, so that is not a choice
    # rusticprofile makes. Only the timer is enable-able; the service is `static`.
    schedule:
      at: hourly           # hourly | daily | weekly | monthly, or an OnCalendar expression
      permission: user     # user | system
      priority: background # background | standard

    # Absolute paths only. ${date:...} is validated now and resolved per run, so a
    # generated systemd unit can never be frozen with the date it was written.
    #
    # ${state_dir} is $XDG_STATE_HOME/rusticprofile, NOT the config directory. Logs are
    # state, and the XDG spec names them as the example of what XDG_STATE_HOME is for.
    # Writing them under ${config_dir} also tends to be self-defeating: if ~/.config is one
    # of your backup sources, the job appends to a directory it is in the middle of backing
    # up, and the rustic profile then needs an exclusion to paper over it.
    log: "${state_dir}/${job}-${date:%Y-%m-%d}.log"

  # Prune, gated to exactly one host.
  #
  # Gated because prune rewrites pack files and there is no reason for several machines to
  # do that work at once -- not because it would be unsafe. rustic is lock-free BY DESIGN:
  # prune marks packs and only deletes them after --keep-delete (23h default), so a
  # concurrent rustic backup has a day of grace.
  #
  # The one combination that IS unsafe is mixing tools. `restic prune` deletes immediately,
  # which is safe only because of an exclusive lock inside the repository -- and rustic
  # never takes that lock. A restic prune against a repository a rustic client is writing
  # to was measured deleting the in-flight packs and leaving it failing `restic check`.
  #
  # So: never run `restic prune` against a repository rustic writes to. If everything
  # touching it is rustic, this is a non-issue.
  dot-files-prune:
    profile: dot-files
    operations: [prune]
    enabled-on-hosts: [host-b]
    schedule:
      at: weekly
      permission: user
      priority: background
"##;

/// Annotated `rustic.toml`.
///
/// This is the file that matters. Every entry with a "NOT a matter of taste" comment was
/// measured against rustic 0.11.3 and a live repository, and getting it wrong is silent in
/// each case — see `PLAN.md` §5.1, §5.5, §5.7, §7.2 and §7.3.
const RUSTIC_TOML: &str = r##"# ~/.config/rustic/dot-files.toml
#
# rustic's own configuration. rusticprofile never writes this file and reads it for exactly
# one thing: enumerating [[backup.snapshots]] names, so it can reject a --name that does not
# exist. Everything else here is between you and rustic.
#
# The annotated items below are NOT matters of taste. Each was measured, each failure mode
# is silent, and several of them destroyed real snapshots before they were understood.
#
# Placeholders: host-a and /home/user. Replace them.

[repository]
# The whole repository string. Note restic's `gs:` scheme DOES NOT EXIST in rustic — only
# local, rclone, rest and opendal. Service parameters come from [repository.options] below;
# there is no `-o` flag.
repository = "opendal:gcs"

# Secrets never reach rusticprofile: it constructs no credential-bearing flags, and there is
# no way to express one. Prefer `password-command` so the secret lives in a keyring or
# password manager rather than on disk.
#   password-command = "secret-tool lookup service backups"
password-file = "/home/user/.config/rustic/dot-files.pw.txt"

[repository.options]
bucket = "example-bucket"
root = "/dot-files"
credential_path = "/home/user/.config/example-credentials.json"

# ---------------------------------------------------------------------------------------
# Scope every snapshot-selecting operation to this machine's own snapshots.
#
# THIS SECTION IS WHAT STOPS ONE MACHINE FORGETTING ANOTHER MACHINE'S HISTORY out of a
# repository several hosts share.
#
# It lives here and nowhere else on purpose: rustic ACCEPTS these keys under [forget] and
# then IGNORES them, and [forget] rejects no unknown keys either — so a config can look
# perfectly scoped and filter nothing at all. rusticprofile refuses at load time to run a
# forget whose profile has no filter here, which is the only reason that mistake is
# survivable.
# ---------------------------------------------------------------------------------------
[snapshot-filter]
filter-hosts = ["host-a"]

[forget]
# Grouping is stated explicitly, and it is "host,label" for two separate reasons.
#
# NOT "host" alone: every named snapshot set would fall into one group and compete for the
# same hourly slot, so retention keeps only whichever set finished last. Measured on a real
# repository — a dry run kept a 0-BYTE snapshot and deleted a 6,256-file one, and reported
# success. An empty snapshot still occupies a retention slot.
#
# NOT the rustic default "host,label,paths", and not paths at all: a renamed source
# directory mints a brand new group with its own full quota, which is how a policy capping
# ~49 snapshots per host left 2,810 in place. Labels are stable by construction; paths are
# not.
group-by = "host,label"
keep-hourly = 24
keep-daily = 7
keep-weekly = 4
keep-monthly = 12
keep-yearly = 2

[backup]
# Options shared by every snapshot set below.
exclude-if-present = ["CACHEDIR.TAG"]
one-file-system = false
tags = ["dot-files"]

# EXCLUSION GLOBS NEED A LEADING `!`.
#
# A bare pattern is an *include* filter: `--glob .cache` produces a snapshot containing only
# .cache, not one with .cache removed. Getting this backwards silently backs up everything
# you meant to leave out, and looks fine until you inspect a stored snapshot.
globs = [
  # Caches and build output — large, and re-creatable by definition.
  "!**/.cache",
  "!**/node_modules",
  "!**/.git",
  "!**/tmp",

  # Transient SQLite sidecars: captured mid-write and always re-creatable, so backing them
  # up is worse than useless.
  "!**/*-wal",
  "!**/*-shm",

  "!**/.local/share/Trash",

  # SECRETS — these must never enter the repository.
  #
  # The credentials file unlocks the bucket this repository lives in, and the password file
  # decrypts the repository. Backing either up here puts the key inside the lock. Verify
  # against a STORED snapshot rather than assuming.
  "!**/.config/rustic/*.pw.txt",
  "!**/example-credentials.json",

  # Scheduled-run logs, if you have pointed them somewhere inside a backed-up tree.
  # With the default `${state_dir}` log path this exclusion is unnecessary — that is
  # $XDG_STATE_HOME, which is not normally a backup source. Kept as an example of the
  # hazard: a job that appends to a directory it is backing up.
  "!**/.config/rusticprofile/logs",
]

# ---------------------------------------------------------------------------------------
# Snapshot sets, split by how reliably each path exists across your machines.
#
# SOURCES MUST BE ABSOLUTE. rustic expands neither `~` nor `$VAR`, and the failure is
# silent and destructive rather than loud: the literal string is a *relative* path, so it
# misses the hard-fail that an absent absolute path gets. rustic logs a warning, backs up
# nothing, saves a 0-byte snapshot and exits 0 -- and that empty snapshot then wins its
# retention slot against the real one. rusticprofile refuses to load a profile whose
# sources contain ~ or $, which is the only reason that mistake is survivable.
#
# So this file is NOT portable between machines: the home path and the hostname filter
# above both have to be real. Generate it per host (chezmoi, or any templating you like)
# rather than trying to make one file work everywhere. Note jobs.yaml has no such problem
# -- it is genuinely identical on every host.
#
# rustic hard-fails an ENTIRE entry if any one of its sources is missing, with no opt-out —
# but one broken entry does not abort the others. So a path that is absent on some hosts
# belongs in its own set, gated per host in jobs.yaml. That is the whole reason for
# splitting; it is not organisational tidiness.
#
# The `label` on each set is what stops retention treating the sets as interchangeable.
# Without it they compete, and the smallest one wins. Keep labels stable forever.
# ---------------------------------------------------------------------------------------

[[backup.snapshots]]
name = "core"
label = "core"
sources = [
  "/home/user/.config",
  "/home/user/.ssh",
]

[[backup.snapshots]]
name = "gnupg"
label = "gnupg"
sources = ["/home/user/.gnupg"]
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_example_is_valid_yaml() {
        let parsed: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(JOBS_YAML).expect("the jobs example must parse as YAML");
        assert!(parsed.get("jobs").is_some(), "it must declare jobs");
    }

    #[test]
    fn rustic_example_is_valid_toml() {
        let parsed: toml::Value =
            toml::from_str(RUSTIC_TOML).expect("the rustic example must parse as TOML");
        assert!(
            parsed.get("backup").is_some(),
            "it must declare a backup section"
        );
    }

    /// The three findings that cost real snapshots. If someone "tidies" one of these out of
    /// the example, the example stops being worth shipping.
    #[test]
    fn rustic_example_carries_the_findings_that_cost_snapshots() {
        assert!(
            RUSTIC_TOML.contains(r#"group-by = "host,label""#),
            "label grouping is what stops named sets evicting each other"
        );
        assert!(
            RUSTIC_TOML.contains("[snapshot-filter]") && RUSTIC_TOML.contains("filter-hosts"),
            "the host scoping filter must be present, and in this section"
        );
        assert!(
            RUSTIC_TOML.contains(r#""!**/.cache""#),
            "exclusion globs must be shown with their leading `!`"
        );
    }

    /// Every snapshot set carries a label, since a set without one rejoins the group it was
    /// split out of and the split stops meaning anything.
    #[test]
    fn every_snapshot_set_in_the_example_has_a_label() {
        let parsed: toml::Value = toml::from_str(RUSTIC_TOML).unwrap();
        let sets = parsed["backup"]["snapshots"]
            .as_array()
            .expect("snapshot sets");
        assert!(!sets.is_empty());
        for set in sets {
            assert!(
                set.get("label").is_some(),
                "snapshot set {:?} has no label",
                set.get("name")
            );
        }
    }

    /// The examples are a pair: every `--name` the jobs example emits has to exist in the
    /// rustic example, which is the very check `config --check` performs at load time.
    #[test]
    fn the_two_examples_agree_on_snapshot_set_names() {
        let rustic: toml::Value = toml::from_str(RUSTIC_TOML).unwrap();
        let declared: Vec<&str> = rustic["backup"]["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();

        let jobs: serde_yaml_ng::Value = serde_yaml_ng::from_str(JOBS_YAML).unwrap();
        for (_, job) in jobs["jobs"].as_mapping().unwrap() {
            let Some(sets) = job.get("snapshot-sets").and_then(|s| s.as_sequence()) else {
                continue;
            };
            for set in sets {
                let name = set["name"].as_str().unwrap();
                assert!(
                    declared.contains(&name),
                    "jobs example names a set `{name}` the rustic example does not declare"
                );
            }
        }
    }

    /// Nothing is interpolated at emit time. `${env:HOME}` is jobs.yaml's own interpolation
    /// syntax, resolved by the loader when the file is read — not something this module
    /// expands — and no real hostname or home directory may be baked in here.
    #[test]
    fn examples_are_static_text_with_placeholder_identifiers() {
        for text in [JOBS_YAML, RUSTIC_TOML] {
            assert!(
                !text.contains("/home/ktobias") && !text.contains("/Users/"),
                "a real home directory leaked into an example"
            );
        }
        assert!(
            RUSTIC_TOML.contains("/home/user"),
            "the rustic example must use the placeholder home"
        );
        assert!(
            JOBS_YAML.contains("host-a"),
            "the jobs example must use placeholder hostnames"
        );
    }

    #[test]
    fn kind_selects_the_matching_text() {
        assert!(ExampleKind::Jobs.text().contains("schema: 1"));
        assert!(ExampleKind::Rustic.text().contains("[repository]"));
    }
}
