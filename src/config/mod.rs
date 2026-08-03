// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Loading `jobs.yaml` into the set of jobs this machine will run.
//!
//! The pipeline order is fixed and load-bearing:
//!
//! 1. **read** the file
//! 2. **parse** it as YAML
//! 3. **gate** jobs and snapshot sets by hostname
//! 4. **interpolate** `${…}` references
//! 5. **validate** everything, reporting all violations at once
//!
//! Parsing before substitution is what makes two of the predecessor's failure modes
//! structurally impossible: comments are discarded by step 2, so a directive written
//! inside one can never be evaluated, and no substitution can ever produce a document
//! that then fails to parse.
//!
//! Every way this can fail — unreadable file, malformed YAML, semantic violation — comes
//! back as [`ValidationErrors`] and exits 2. One error type, one presentation, and "the
//! config is wrong" stays distinguishable from "the backup failed".

pub mod example;
pub mod hosts;
pub mod interp;
pub mod job;
pub mod paths;
pub mod rustic_toml;
pub mod schedule;
pub mod validate;

use std::path::{Path, PathBuf};

use interp::{Ctx, Env};
use job::{GatedOut, Job, RawConfig};
use validate::{ValidationErrors, Violation};

/// Default rustic executable when `defaults.rustic-binary` is absent.
pub const DEFAULT_RUSTIC_BINARY: &str = "rustic";

/// Inputs to [`load`].
pub struct LoadOptions {
    /// Path to `jobs.yaml`.
    pub path: PathBuf,
    /// Evaluate as though running on this host. Makes another machine's view of the
    /// config inspectable from here, which is the only way to check a per-host gate
    /// without logging into every machine.
    pub as_host: Option<String>,
    /// Clock for `${date:…}`. `None` leaves those references unresolved — see
    /// [`interp::Ctx::now`].
    pub now: Option<jiff::Zoned>,
}

/// The configuration as it applies to one host.
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub rustic_binary: String,
    pub rustic_config_dir: PathBuf,
    /// Jobs this host runs, in name order.
    pub jobs: Vec<Job>,
    /// Jobs excluded by `enabled-on-hosts`, kept so the gate is inspectable rather than
    /// invisible. A job that vanished without trace is indistinguishable from a job that
    /// was never written.
    pub gated_out: Vec<GatedOut>,
    /// Job to use when a command is given no `-n`, if the configuration names one.
    ///
    /// Carried resolved rather than raw so every caller reaches the same answer; validation
    /// has already confirmed it names a declared job.
    pub default_job: Option<String>,
    /// True when `--as-host` named a machine other than this one.
    ///
    /// Some checks cannot be answered for another host from here, and this is what lets
    /// the report **say which** rather than quietly passing. See
    /// [`validate::check_filter_hosts_can_match`].
    pub simulating_another_host: bool,
}

impl Config {
    /// Look up a job by name.
    pub fn job(&self, name: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.name == name)
    }
}

/// Read, gate, interpolate and validate the configuration.
pub fn load(opts: &LoadOptions) -> Result<Config, ValidationErrors> {
    let display_path = opts.path.display().to_string();

    let text = std::fs::read_to_string(&opts.path).map_err(|e| {
        ValidationErrors(vec![Violation::new(
            display_path.clone(),
            format!("could not be read: {e}"),
        )])
    })?;

    let raw: RawConfig = serde_yaml_ng::from_str(&text).map_err(|e| {
        ValidationErrors(vec![Violation::new(
            display_path.clone(),
            format!("is not valid YAML: {e}"),
        )])
    })?;

    // Resolved even when `--as-host` overrides it: telling "this machine" from "the one
    // being simulated" is what lets a check that cannot cross that boundary say so.
    let real_host = hosts::current_hostname()
        .map_err(|e| ValidationErrors(vec![Violation::new("host", format!("{e:#}"))]))?;
    let host = opts.as_host.clone().unwrap_or_else(|| real_host.clone());
    let host_short = hosts::short(&host).to_string();

    let config_dir = paths::user_config_dir().unwrap_or_else(|_| PathBuf::from("."));
    let state_dir = paths::user_state_dir().unwrap_or_else(|_| PathBuf::from("."));
    let temp_dir = std::env::temp_dir();

    let mut violations = validate::check_declared(&raw);
    violations.extend(validate::check_default_job_exists(&raw));

    // `defaults` is resolved outside any job, so `${job}` and `${profile}` are errors
    // there rather than empty strings.
    let base_ctx = |job: Option<&'static str>| Ctx {
        host: &host,
        host_short: &host_short,
        job,
        profile: None,
        config_dir: &config_dir,
        state_dir: &state_dir,
        temp_dir: &temp_dir,
        env: Env::System,
        now: opts.now.as_ref(),
    };

    let rustic_binary = raw
        .defaults
        .rustic_binary
        .clone()
        .unwrap_or_else(|| DEFAULT_RUSTIC_BINARY.to_string());

    let rustic_config_dir = match &raw.defaults.rustic_config_dir {
        Some(raw_dir) => match interp::interpolate(raw_dir, &base_ctx(None)) {
            Ok(resolved) => PathBuf::from(resolved),
            Err(e) => {
                violations.push(Violation::new("defaults.rustic-config-dir", e.to_string()));
                // Fall back so the remaining checks still run and report together.
                paths::default_rustic_config_dir().unwrap_or_else(|_| PathBuf::from("."))
            }
        },
        None => match paths::default_rustic_config_dir() {
            Ok(dir) => dir,
            Err(e) => {
                violations.push(Violation::new(
                    "defaults.rustic-config-dir",
                    format!("{e:#}"),
                ));
                PathBuf::from(".")
            }
        },
    };

    violations.extend(validate::check_snapshot_sets_exist(
        &raw,
        &rustic_config_dir,
    ));
    violations.extend(validate::check_forget_is_scoped(&raw, &rustic_config_dir));
    violations.extend(validate::check_sources_are_expanded(
        &raw,
        &rustic_config_dir,
    ));
    // Skipped when simulating another machine, and reported rather than dropped.
    //
    // The check compares `filter-hosts` in *this* machine's `rustic.toml` against the host
    // that will run the job. Under `--as-host` those are, by construction, a profile from
    // one disk and a hostname from another — and §5.9 requires that profile to differ per
    // host, so they will disagree whenever the simulation is doing its job. Running it
    // anyway reports a defect on every host but this one, which is a false alarm loud
    // enough to make `--as-host` useless for the gate inspection it exists for.
    let simulating_another_host = opts.as_host.as_deref().is_some_and(|h| h != real_host);
    if !simulating_another_host {
        violations.extend(validate::check_filter_hosts_can_match(
            &raw,
            &rustic_config_dir,
            &host,
        ));
    }

    let mut jobs = Vec::new();
    let mut gated_out = Vec::new();

    for (name, raw_job) in &raw.jobs {
        // Step 3: host gating. A gated-out job is *removed*, not emptied — the difference
        // between "this host has no prune job" and "this host has a prune job that never
        // does anything" is exactly the bug this replaces.
        if let Some(allowed) = &raw_job.enabled_on_hosts
            && !allowed.iter().any(|h| hosts::host_matches(h, &host))
        {
            gated_out.push(GatedOut {
                name: name.clone(),
                enabled_on_hosts: allowed.clone(),
            });
            continue;
        }

        let resolved_sets: Vec<String> = raw_job
            .snapshot_sets
            .iter()
            .filter(|set| match &set.enabled_on_hosts {
                Some(allowed) => allowed.iter().any(|h| hosts::host_matches(h, &host)),
                None => true,
            })
            .map(|set| set.name.clone())
            .collect();

        // A job that declared sets and kept none would run `rustic backup` with no
        // `--name`, which means "back up everything the profile defines" — the opposite
        // of what the config asked for. Refuse rather than guess.
        if !raw_job.snapshot_sets.is_empty() && resolved_sets.is_empty() {
            violations.push(Violation::new(
                format!("jobs.{name}.snapshot-sets"),
                format!(
                    "every snapshot set is gated away on `{host}`, which would run a backup \
                     of the whole profile instead of nothing; gate the job itself with \
                     `enabled-on-hosts` if it should not run here"
                ),
            ));
        }

        let job_ctx = Ctx {
            host: &host,
            host_short: &host_short,
            job: Some(name),
            profile: Some(&raw_job.profile),
            config_dir: &config_dir,
            state_dir: &state_dir,
            temp_dir: &temp_dir,
            env: Env::System,
            now: opts.now.as_ref(),
        };

        let log = match &raw_job.log {
            Some(raw_log) => match interp::interpolate(raw_log, &job_ctx) {
                Ok(resolved) => {
                    if !Path::new(&resolved).is_absolute() {
                        violations.push(Violation::new(
                            format!("jobs.{name}.log"),
                            format!(
                                "`{resolved}` is not an absolute path; a scheduled job has no \
                                 predictable working directory"
                            ),
                        ));
                    }
                    Some(resolved)
                }
                Err(e) => {
                    violations.push(Violation::new(format!("jobs.{name}.log"), e.to_string()));
                    None
                }
            },
            None => None,
        };

        jobs.push(Job {
            name: name.clone(),
            profile: raw_job.profile.clone(),
            operations: raw_job.operations.clone(),
            snapshot_sets: resolved_sets,
            declared_snapshot_sets: raw_job.snapshot_sets.len(),
            schedule: raw_job.schedule,
            log,
        });
    }

    if !violations.is_empty() {
        violations.sort();
        return Err(ValidationErrors(violations));
    }

    Ok(Config {
        host,
        rustic_binary,
        rustic_config_dir,
        jobs,
        gated_out,
        default_job: raw.defaults.default_job.clone(),
        simulating_another_host,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A config directory holding `jobs.yaml` and a rustic profile `p.toml`.
    fn fixture(jobs_yaml: &str, profile_toml: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let jobs = dir.path().join("jobs.yaml");
        let mut f = std::fs::File::create(&jobs).unwrap();
        f.write_all(jobs_yaml.as_bytes()).unwrap();
        std::fs::write(dir.path().join("p.toml"), profile_toml).unwrap();
        (dir, jobs)
    }

    /// A profile scoped well enough to satisfy the `forget` invariant, since several
    /// fixtures below run `forget`.
    const PROFILE: &str = r#"
[[backup.snapshots]]
name = "core"
sources = ["/x"]

[[backup.snapshots]]
name = "gnupg"
sources = ["/y"]

[snapshot-filter]
# Every host any test loads this as, including the dotted form. `enabled-on-hosts` matches
# a short name against `host-a.local`, but `filter-hosts` is matched by *rustic*, exactly —
# so both forms have to be listed here. That asymmetry is deliberate and is what
# `check_filter_hosts_can_match` exists to enforce.
filter-hosts = ["host-a", "host-b", "host-c", "host-d", "host-a.local"]

[forget]
group-by = "host,label,paths"
"#;

    fn load_as(jobs_yaml: &str, host: &str) -> Result<Config, ValidationErrors> {
        let (dir, jobs) = fixture(jobs_yaml, PROFILE);
        let yaml = jobs_yaml.replace("RUSTIC_DIR", &dir.path().display().to_string());
        std::fs::write(&jobs, yaml).unwrap();
        let result = load(&LoadOptions {
            path: jobs.clone(),
            as_host: Some(host.to_string()),
            now: None,
        });
        drop(dir);
        result
    }

    const GATED: &str = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  dot-files:
    profile: p
    operations: [backup, forget]
    snapshot-sets:
      - name: core
      - name: gnupg
        enabled-on-hosts: [host-a]
  dot-files-prune:
    profile: p
    operations: [prune]
    enabled-on-hosts: [host-b]
";

    #[test]
    fn host_gating_removes_the_job_entirely() {
        let on_a = load_as(GATED, "host-a").unwrap();
        assert_eq!(on_a.jobs.len(), 1);
        assert_eq!(on_a.jobs[0].name, "dot-files");
        // The prune job is absent, and its absence is recorded rather than silent.
        assert_eq!(on_a.gated_out.len(), 1);
        assert_eq!(on_a.gated_out[0].name, "dot-files-prune");

        let on_b = load_as(GATED, "host-b").unwrap();
        let names: Vec<&str> = on_b.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["dot-files", "dot-files-prune"]);
        assert!(on_b.gated_out.is_empty());
    }

    #[test]
    fn exactly_one_host_gets_the_prune_job() {
        // The fleet-wide property the template gate this replaces never had a test for.
        let fleet = ["host-a", "host-b", "host-c", "host-d", "host-a.local"];
        let with_prune: Vec<&str> = fleet
            .iter()
            .filter(|h| load_as(GATED, h).unwrap().job("dot-files-prune").is_some())
            .copied()
            .collect();
        assert_eq!(with_prune, vec!["host-b"]);
    }

    #[test]
    fn snapshot_sets_are_gated_per_host() {
        assert_eq!(
            load_as(GATED, "host-a").unwrap().jobs[0].snapshot_sets,
            vec!["core", "gnupg"]
        );
        assert_eq!(
            load_as(GATED, "host-c").unwrap().jobs[0].snapshot_sets,
            vec!["core"]
        );
    }

    #[test]
    fn declared_count_survives_gating() {
        // Needed to tell "never had sets" from "had sets, kept none".
        let c = load_as(GATED, "host-c").unwrap();
        assert_eq!(c.jobs[0].declared_snapshot_sets, 2);
        assert_eq!(c.jobs[0].snapshot_sets.len(), 1);
    }

    #[test]
    fn a_dotted_hostname_matches_its_short_form() {
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  j:
    profile: p
    operations: [backup]
    enabled-on-hosts: [host-a]
";
        assert_eq!(load_as(yaml, "host-a.local").unwrap().jobs.len(), 1);
        assert!(load_as(yaml, "host-b.local").unwrap().jobs.is_empty());
    }

    #[test]
    fn all_sets_gated_away_is_an_error_not_an_accidental_full_backup() {
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: core
        enabled-on-hosts: [host-a]
";
        assert!(load_as(yaml, "host-a").is_ok());
        let err = load_as(yaml, "host-z").unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err.0[0].message.contains("gated away"));
    }

    #[test]
    fn log_paths_interpolate_and_must_be_absolute() {
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  j:
    profile: p
    operations: [backup]
    log: /var/log/${job}-${host_short}.log
";
        let c = load_as(yaml, "host-a.local").unwrap();
        assert_eq!(c.jobs[0].log.as_deref(), Some("/var/log/j-host-a.log"));

        let relative = yaml.replace("/var/log/", "logs/");
        let err = load_as(&relative, "host-a.local").unwrap_err();
        assert!(err.0[0].message.contains("absolute"));
    }

    #[test]
    fn a_date_reference_in_a_log_path_stays_deferred() {
        // It must survive to run time, so a unit file cannot freeze one day's date.
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  j:
    profile: p
    operations: [backup]
    log: /var/log/${job}-${date:%Y-%m-%d}.log
";
        let c = load_as(yaml, "h").unwrap();
        assert_eq!(
            c.jobs[0].log.as_deref(),
            Some("/var/log/j-${date:%Y-%m-%d}.log")
        );
    }

    #[test]
    fn unreadable_and_malformed_files_report_as_violations() {
        let missing = load(&LoadOptions {
            path: PathBuf::from("/nonexistent/jobs.yaml"),
            as_host: Some("h".to_string()),
            now: None,
        })
        .unwrap_err();
        assert!(missing.0[0].message.contains("could not be read"));

        let err = load_as("schema: 1\njobs: [oops\n", "h").unwrap_err();
        assert!(err.0[0].message.contains("not valid YAML"));
    }

    #[test]
    fn defaults_may_not_reference_job_or_profile() {
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: /etc/${job}
jobs:
  j:
    profile: p
    operations: [backup]
";
        let err = load_as(yaml, "h").unwrap_err();
        assert!(
            err.0
                .iter()
                .any(|v| v.location == "defaults.rustic-config-dir"),
            "got {err:?}"
        );
    }

    #[test]
    fn the_rustic_binary_defaults_and_can_be_overridden() {
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  j:
    profile: p
    operations: [backup]
";
        assert_eq!(load_as(yaml, "h").unwrap().rustic_binary, "rustic");

        let overridden = yaml.replace("defaults:", "defaults:\n  rustic-binary: /opt/bin/rustic");
        assert_eq!(
            load_as(&overridden, "h").unwrap().rustic_binary,
            "/opt/bin/rustic"
        );
    }

    #[test]
    fn every_violation_is_reported_in_one_pass() {
        let yaml = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  a:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: typo
    log: relative/path.log
  b:
    profile: p
    operations: [backup, backup]
";
        let err = load_as(yaml, "h").unwrap_err();
        // unknown snapshot set + relative log + duplicate operation, together.
        assert!(err.len() >= 3, "expected several violations, got {err}");
        let text = err.to_string();
        assert!(text.contains("typo"));
        assert!(text.contains("absolute"));
    }
}
