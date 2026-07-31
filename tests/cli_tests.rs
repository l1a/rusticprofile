// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests driving the real binary.
//!
//! No `assert_cmd` / `predicates` — the binary path comes from `CARGO_BIN_EXE_*`, which
//! Cargo sets for integration tests, and `std::process::Command` does the rest.

use std::process::Command;

/// Run the binary with `args`, returning (stdout, stderr, success).
fn run(args: &[&str]) -> (String, String, bool) {
    let bin_path = env!("CARGO_BIN_EXE_rusticprofile");
    let output = Command::new(bin_path)
        .args(args)
        .output()
        .expect("failed to execute rusticprofile binary");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stdout, stderr, output.status.success())
}

#[test]
fn help_succeeds_and_describes_the_tool() {
    let (stdout, stderr, success) = run(&["--help"]);
    assert!(success);
    assert!(stderr.is_empty());
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--completions"));
}

#[test]
fn version_matches_cargo_manifest() {
    let (stdout, stderr, success) = run(&["--version"]);
    assert!(success);
    assert!(stderr.is_empty());
    // Guards against the version drifting out of the binary, which `just pr` checks
    // against the last git tag.
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn completions_generate_for_every_supported_shell() {
    // One case per shell installed by `just install-completions`. A shell that stops
    // generating output would otherwise install an empty completion file silently.
    for shell in ["bash", "zsh", "fish", "elvish", "nushell", "power-shell"] {
        let (stdout, stderr, success) = run(&["--completions", shell]);
        assert!(success, "--completions {shell} exited non-zero: {stderr}");
        assert!(
            !stdout.trim().is_empty(),
            "--completions {shell} produced no output"
        );
        assert!(
            stdout.contains("rusticprofile"),
            "--completions {shell} output does not mention the binary name"
        );
    }
}

#[test]
fn unknown_shell_is_rejected() {
    let (_, stderr, success) = run(&["--completions", "tcsh"]);
    assert!(!success);
    assert!(stderr.contains("invalid value"));
}

#[test]
fn bare_invocation_fails_loudly() {
    // There is nothing to run yet. It must not exit 0 — a silent success here is exactly
    // the failure mode this project exists to prevent, and a systemd unit or wrapper
    // script would happily believe it.
    let (stdout, stderr, success) = run(&[]);
    assert!(!success, "bare invocation must not report success");
    assert!(stdout.is_empty());
    assert!(stderr.contains("nothing to do"));
}

/// Exit code meaning "the configuration is wrong", as distinct from a failed run.
const EXIT_CONFIG_ERROR: i32 = 2;

/// Run the binary and return (stdout, stderr, exit code).
fn run_code(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_rusticprofile"))
        .args(args)
        .output()
        .expect("failed to execute rusticprofile binary");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("process should exit normally"),
    )
}

/// A profile scoped well enough to satisfy the `forget` invariant, since several fixtures
/// below run `forget`.
const PROFILE_TOML: &str = r#"
[[backup.snapshots]]
name = "core"
sources = ["/x"]

[[backup.snapshots]]
name = "gnupg"
sources = ["/y"]

[snapshot-filter]
filter-hosts = ["host-a", "host-b", "host-c"]

[forget]
group-by = "host,label,paths"
"#;

/// Write a jobs.yaml and a rustic profile into a fresh temp dir.
///
/// `RUSTIC_DIR` in the YAML is replaced with the temp dir, so fixtures stay hermetic and
/// never read the developer's real `~/.config/rustic`.
fn fixture(jobs_yaml: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("p.toml"), PROFILE_TOML).unwrap();
    let jobs = dir.path().join("jobs.yaml");
    let rendered = jobs_yaml.replace("RUSTIC_DIR", &dir.path().display().to_string());
    std::fs::write(&jobs, rendered).unwrap();
    let path = jobs.display().to_string();
    (dir, path)
}

const GOOD_CONFIG: &str = "
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
    schedule:
      at: hourly
  dot-files-prune:
    profile: p
    operations: [prune]
    enabled-on-hosts: [host-b]
";

#[test]
fn config_check_accepts_a_valid_configuration() {
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, _stderr, code) = run_code(&[
        "config",
        "--check",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.contains("dot-files"));
}

#[test]
fn config_check_surfaces_the_host_gate() {
    // "This host has no prune job" must be visible, not inferred from an absence.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, _stderr, code) = run_code(&[
        "config",
        "--check",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.contains("not on this host"));
    assert!(stdout.contains("dot-files-prune"));
}

#[test]
fn as_host_makes_another_machines_view_inspectable() {
    // The only way to verify a per-host gate without logging into every machine.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (on_b, _, code) = run_code(&[
        "config",
        "--check",
        "--config",
        &path,
        "--as-host",
        "host-b",
    ]);
    assert_eq!(code, 0);
    assert!(!on_b.contains("not on this host"), "host-b runs everything");
}

#[test]
fn config_check_reports_every_violation_at_once_and_exits_2() {
    let (_dir, path) = fixture(
        "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  a:
    profile: p
    operations: [backup, backup]
    snapshot-sets:
      - name: typo
    log: relative/path.log
",
    );
    let (_stdout, stderr, code) =
        run_code(&["config", "--check", "--config", &path, "--as-host", "h"]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stderr.contains("3 configuration errors"), "got: {stderr}");
    // The unknown snapshot set is the important one: rustic itself would ignore it
    // silently and back up less than intended.
    assert!(stderr.contains("typo"));
    assert!(stderr.contains("silently ignore"));
    assert!(stderr.contains("absolute"));
    assert!(stderr.contains("more than once"));
}

#[test]
fn an_unknown_config_key_is_rejected_by_name() {
    // A key that never takes effect is how a setting can go years without anyone
    // noticing it was never applied.
    let (_dir, path) = fixture(
        "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  a:
    profile: p
    operations: [backup]
    gcs:
      connections: 10
",
    );
    let (_stdout, stderr, code) =
        run_code(&["config", "--check", "--config", &path, "--as-host", "h"]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(
        stderr.contains("gcs"),
        "the error should name the key: {stderr}"
    );
}

#[test]
fn config_show_renders_a_resolved_job() {
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, _stderr, code) = run_code(&[
        "config",
        "--show",
        "-n",
        "dot-files",
        "--config",
        &path,
        "--as-host",
        "host-c",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.contains("backup, forget"));
    // host-c gets `core` only; `gnupg` is gated to host-a.
    assert!(stdout.contains("core"));
    assert!(stdout.contains("gated off on this host"));
}

#[test]
fn config_show_explains_a_gated_off_job_rather_than_denying_it_exists() {
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (_stdout, stderr, code) = run_code(&[
        "config",
        "--show",
        "-n",
        "dot-files-prune",
        "--config",
        &path,
        "--as-host",
        "host-c",
    ]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stderr.contains("not enabled on"), "got: {stderr}");
    assert!(stderr.contains("host-b"), "should name where it does run");
}

#[test]
fn config_show_lists_known_jobs_for_an_unknown_name() {
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (_stdout, stderr, code) = run_code(&[
        "config",
        "--show",
        "-n",
        "nope",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stderr.contains("dot-files"), "got: {stderr}");
}

#[test]
fn a_missing_config_file_is_a_config_error_not_a_panic() {
    let (_stdout, stderr, code) = run_code(&[
        "config",
        "--check",
        "--config",
        "/nonexistent/dir/jobs.yaml",
        "--as-host",
        "h",
    ]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stderr.contains("could not be read"));
}

const GOLDEN_CONFIG: &str = "
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
  whole-profile:
    profile: p
    operations: [backup]
";

/// Compare `plan --format lines` output against a committed golden file.
///
/// Goldens are one argv element per line so a changed argument shows as one changed line.
/// They are hermetic: no rustic binary, no real hostname, no clock, so the same bytes come
/// out on every machine. Regenerate deliberately with `just golden`; `just check` fails if
/// any of them would change, which is what stops a silent argv change from riding along
/// with an unrelated commit.
fn assert_golden(case: &str, job: &str, host: &str) {
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (stdout, stderr, code) = run_code(&[
        "plan",
        "-n",
        job,
        "--format",
        "lines",
        "--config",
        &path,
        "--as-host",
        host,
    ]);
    assert_eq!(code, 0, "plan failed for {case}: {stderr}");

    // The argv now carries the resolved profile path, which contains a per-run temp
    // directory. Normalise it back so goldens stay machine-independent while still
    // recording that a resolved path is passed at all.
    let stdout = stdout.replace(_dir.path().to_string_lossy().as_ref(), "RUSTIC_DIR");

    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{case}.txt"));

    if std::env::var_os("RP_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_path.parent().unwrap()).unwrap();
        std::fs::write(&golden_path, &stdout).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
        panic!(
            "cannot read golden {}: {e}. If this case is new, run `just golden`.",
            golden_path.display()
        )
    });
    assert_eq!(
        stdout, expected,
        "argv changed for `{case}`. If that is intended, run `just golden` and commit the result."
    );
}

#[test]
fn golden_backup_and_forget_with_two_sets() {
    assert_golden("backup-forget-two-sets", "dot-files", "host-a");
}

#[test]
fn golden_backup_and_forget_with_one_set() {
    // Same job, different host: `gnupg` is gated to host-a, so only `core` survives.
    assert_golden("backup-forget-one-set", "dot-files", "host-c");
}

#[test]
fn golden_prune_only() {
    assert_golden("prune-only", "dot-files-prune", "host-b");
}

#[test]
fn golden_backup_with_no_sets() {
    // No `--name` at all, which tells rustic to use every entry the profile defines.
    assert_golden("backup-no-sets", "whole-profile", "host-a");
}

#[test]
fn plan_human_format_labels_each_operation() {
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (stdout, _stderr, code) = run_code(&[
        "plan",
        "-n",
        "dot-files",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.contains("backup"));
    assert!(stdout.contains("forget"));
    assert!(stdout.contains("--name core"));
}

#[test]
fn plan_never_spawns_rustic() {
    // Planning is inspection. It must work with no rustic binary anywhere on PATH, so it
    // is safe to run on a machine that has never had one installed.
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let output = Command::new(env!("CARGO_BIN_EXE_rusticprofile"))
        .args([
            "plan",
            "-n",
            "dot-files",
            "--format",
            "lines",
            "--config",
            &path,
            "--as-host",
            "host-a",
        ])
        .env("PATH", "/nonexistent")
        .output()
        .expect("failed to execute binary");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("rustic"));
}

#[test]
fn plan_explains_a_gated_off_job() {
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (_stdout, stderr, code) = run_code(&[
        "plan",
        "-n",
        "dot-files-prune",
        "--config",
        &path,
        "--as-host",
        "host-c",
    ]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stderr.contains("not enabled on"), "got: {stderr}");
}

/// Run the binary with a controlled environment, returning (stdout, stderr, exit code).
///
/// `env_clear` first so a developer's real `RUSTIC_*` or `GOOGLE_*` variables cannot leak
/// into an assertion — or into test output.
fn run_code_env(args: &[&str], vars: &[(&str, &str)]) -> (String, String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rusticprofile"));
    cmd.args(args).env_clear();
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .expect("failed to execute rusticprofile binary");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().expect("process should exit normally"),
    )
}

#[test]
fn show_env_masks_secrets_by_default() {
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (stdout, _stderr, code) = run_code_env(
        &[
            "plan",
            "-n",
            "dot-files",
            "--show-env",
            "--config",
            &path,
            "--as-host",
            "host-a",
        ],
        &[
            ("RUSTIC_PASSWORD", "hunter2"),
            ("RUSTIC_PASSWORD_FILE", "/etc/pw.txt"),
            ("OPENDAL_BUCKET", "some-bucket"),
        ],
    );
    assert_eq!(code, 0);
    assert!(
        !stdout.contains("hunter2"),
        "the secret must not be printed: {stdout}"
    );
    assert!(stdout.contains("RUSTIC_PASSWORD=<redacted>"));
    // A path is not a secret, and hiding it would remove the detail most worth seeing.
    assert!(stdout.contains("RUSTIC_PASSWORD_FILE=/etc/pw.txt"));
    assert!(stdout.contains("OPENDAL_BUCKET=some-bucket"));
}

#[test]
fn show_secrets_reveals_values_and_warns_first() {
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (stdout, stderr, code) = run_code_env(
        &[
            "plan",
            "-n",
            "dot-files",
            "--show-env",
            "--show-secrets",
            "--config",
            &path,
            "--as-host",
            "host-a",
        ],
        &[("RUSTIC_PASSWORD", "hunter2")],
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("RUSTIC_PASSWORD=hunter2"));
    // The warning belongs on stderr so it survives redirection of stdout, and it must
    // appear at all — printing credentials without saying so would be worse than not
    // offering the option.
    assert!(stderr.contains("warning"), "got stderr: {stderr}");
    assert!(!stderr.contains("hunter2"), "the warning must not echo it");
}

#[test]
fn show_secrets_requires_show_env() {
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (_stdout, _stderr, code) = run_code(&[
        "plan",
        "-n",
        "dot-files",
        "--show-secrets",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_ne!(code, 0, "--show-secrets alone should be a usage error");
}

#[test]
fn show_env_is_rejected_with_the_machine_readable_format() {
    // `--format lines` is an exact argv; appending an environment block would corrupt it
    // for anything parsing the output.
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (stdout, stderr, code) = run_code(&[
        "plan",
        "-n",
        "dot-files",
        "--show-env",
        "--format",
        "lines",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stdout.is_empty(), "nothing should be printed to stdout");
    assert!(stderr.contains("--show-env"), "got: {stderr}");
}

#[test]
fn show_env_reports_an_empty_environment_rather_than_nothing() {
    // Silence here would be ambiguous: "no variables set" and "the feature did not run"
    // would look identical.
    let (_dir, path) = fixture(GOLDEN_CONFIG);
    let (stdout, _stderr, code) = run_code_env(
        &[
            "plan",
            "-n",
            "dot-files",
            "--show-env",
            "--config",
            &path,
            "--as-host",
            "host-a",
        ],
        &[],
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("none set"), "got: {stdout}");
}

/// Whether a real `rustic` is available to exercise against.
///
/// CI does not install one, so the tests below skip there and run locally. They are the
/// only tests that touch a real repository — always a throwaway one under a temp dir,
/// never a shared or production repository.
fn rustic_available() -> bool {
    Command::new("rustic")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A throwaway local repository with a good snapshot set and a broken one.
///
/// Returns (tempdir, profile path without the `.toml` suffix).
fn throwaway_repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().display().to_string();
    std::fs::create_dir_all(dir.path().join("src/good")).unwrap();
    std::fs::write(dir.path().join("src/good/file.txt"), "contents").unwrap();

    std::fs::write(
        dir.path().join("p.toml"),
        format!(
            r#"
[repository]
repository = "{root}/repo"
password = "test-only-throwaway"

[[backup.snapshots]]
name = "good"
sources = ["{root}/src/good"]

[[backup.snapshots]]
name = "broken"
sources = ["{root}/src/does-not-exist"]
"#
        ),
    )
    .unwrap();

    let profile = format!("{root}/p");
    let init = rusticprofile::exec::run(
        &[
            "rustic".into(),
            "-P".into(),
            profile.clone().into(),
            "init".into(),
        ],
        rusticprofile::exec::Stdout::Capture,
    )
    .expect("rustic init should spawn");
    assert!(
        init.exited_zero(),
        "rustic init failed: {}",
        init.describe()
    );

    (dir, profile)
}

#[test]
fn real_rustic_partial_backup_is_classified_as_partial() {
    // The single most important behaviour in Milestone 1, checked against real rustic
    // rather than a fixture. One good snapshot set and one whose source does not exist:
    // rustic saves the first, fails the second, and exits 1 for the whole run. Treating
    // that as a total failure is the bug that let 2810 snapshots accumulate.
    if !rustic_available() {
        eprintln!("skipping: rustic is not installed");
        return;
    }
    use rusticprofile::config::job::Operation;
    use rusticprofile::rustic::{exit, invoke};

    let (_dir, profile) = throwaway_repo();
    let argv = invoke::build_argv(
        "rustic",
        &profile,
        Operation::Backup,
        &["good".to_string(), "broken".to_string()],
        invoke::Options::default(),
    );

    let outcome = rusticprofile::exec::run(&argv, rusticprofile::exec::Stdout::Capture)
        .expect("rustic should spawn");

    // Documents rustic's actual behaviour in the suite, so a future version changing it
    // fails here rather than silently altering what we conclude.
    assert_eq!(
        outcome.code,
        Some(1),
        "rustic should exit 1 for a partial backup"
    );

    let c = exit::classify(Operation::Backup, &outcome, Some(2), false);
    assert_eq!(c.verdict, exit::Verdict::Partial, "summary: {}", c.summary);
    assert_eq!(c.snapshots_saved, 1);
    assert!(
        c.verdict.should_continue(),
        "a partial backup must not abort the job — retention still has to run"
    );
}

#[test]
fn real_rustic_clean_backup_is_classified_as_success() {
    if !rustic_available() {
        eprintln!("skipping: rustic is not installed");
        return;
    }
    use rusticprofile::config::job::Operation;
    use rusticprofile::rustic::{exit, invoke};

    let (_dir, profile) = throwaway_repo();
    let argv = invoke::build_argv(
        "rustic",
        &profile,
        Operation::Backup,
        &["good".to_string()],
        invoke::Options::default(),
    );
    let outcome = rusticprofile::exec::run(&argv, rusticprofile::exec::Stdout::Capture)
        .expect("rustic should spawn");

    assert_eq!(outcome.code, Some(0));
    let c = exit::classify(Operation::Backup, &outcome, Some(1), false);
    assert_eq!(c.verdict, exit::Verdict::Success, "summary: {}", c.summary);
    assert_eq!(c.snapshots_saved, 1);
}

#[test]
fn real_rustic_total_failure_is_classified_as_failure() {
    // Only a broken source: exit 1 with no snapshot objects. This must stop the job —
    // running retention after saving nothing is the direction that loses data.
    if !rustic_available() {
        eprintln!("skipping: rustic is not installed");
        return;
    }
    use rusticprofile::config::job::Operation;
    use rusticprofile::rustic::{exit, invoke};

    let (_dir, profile) = throwaway_repo();
    let argv = invoke::build_argv(
        "rustic",
        &profile,
        Operation::Backup,
        &["broken".to_string()],
        invoke::Options::default(),
    );
    let outcome = rusticprofile::exec::run(&argv, rusticprofile::exec::Stdout::Capture)
        .expect("rustic should spawn");

    assert_eq!(outcome.code, Some(1));
    let c = exit::classify(Operation::Backup, &outcome, Some(1), false);
    assert_eq!(c.verdict, exit::Verdict::Failure, "summary: {}", c.summary);
    assert_eq!(c.snapshots_saved, 0);
    assert!(!c.verdict.should_continue());
}

/// A job config whose job name is unique per test.
///
/// The run lock is keyed on the job name and is machine-wide — correct in production,
/// where two runs of the same job must not overlap, but it means two tests running a
/// job of the same name in parallel would contend and one would be refused. Distinct
/// names keep the tests independent without weakening the lock.
fn run_config_named(job: &str) -> String {
    format!(
        "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
jobs:
  {job}:
    profile: p
    operations: [backup, forget]
    snapshot-sets:
      - name: core
"
    )
}

/// A recording shim: logs its argv to a file and exits 0 without touching a repository.
///
/// This is rung 2 of the verification ladder in `PLAN.md` — the rung that proves a job's
/// argv end to end while rustic never runs.
fn recording_shim(dir: &std::path::Path) -> String {
    let shim = dir.join("shim.sh");
    let log = dir.join("argv.log");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    shim.display().to_string()
}

#[test]
fn rustic_binary_override_lets_a_shim_stand_in_for_rustic() {
    let job = "shim-job";
    let (dir, path) = fixture(&run_config_named(job));
    let shim = recording_shim(dir.path());

    let (stdout, stderr, code) = run_code(&[
        "run",
        "-n",
        job,
        "--config",
        &path,
        "--as-host",
        "host-a",
        "--rustic-binary",
        &shim,
    ]);

    assert_eq!(
        code, 0,
        "the shim exits 0, so the job should succeed: {stderr}"
    );
    assert!(stdout.contains("ok"), "got: {stdout}");

    // Both operations ran, and the argv reaching the child is the planned one.
    let log = std::fs::read_to_string(dir.path().join("argv.log")).expect("shim should have run");
    assert!(log.contains("backup"), "got log: {log}");
    assert!(log.contains("forget"), "forget must still run: {log}");
    assert!(log.contains("--name"));
}

#[test]
fn a_missing_rustic_binary_is_a_run_failure_not_a_panic() {
    // Exit 1, not 2: the configuration is fine, the run is what failed. A monitoring
    // system has to be able to tell those apart.
    let job = "missing-binary-job";
    let (_dir, path) = fixture(&run_config_named(job));
    let (_stdout, _stderr, code) = run_code(&[
        "run",
        "-n",
        job,
        "--config",
        &path,
        "--as-host",
        "host-a",
        "--rustic-binary",
        "/nonexistent/definitely-not-rustic",
    ]);
    assert_eq!(code, 1);
}

#[test]
fn the_exit_code_surface_is_what_the_plan_promises() {
    // 0 success, 1 run failure, 2 config error. 130 (interrupted) needs a real signal and
    // is covered by the exec unit tests instead.
    let (dir, path) = fixture(GOLDEN_CONFIG);
    let shim = recording_shim(dir.path());

    let ok = run_code(&[
        "run",
        "-n",
        "dot-files",
        "--config",
        &path,
        "--as-host",
        "host-a",
        "--rustic-binary",
        &shim,
    ]);
    assert_eq!(ok.2, 0, "a clean run exits 0");

    let bad_config = run_code(&[
        "run",
        "-n",
        "dot-files",
        "--config",
        "/nonexistent/jobs.yaml",
        "--as-host",
        "host-a",
    ]);
    assert_eq!(bad_config.2, EXIT_CONFIG_ERROR, "a config error exits 2");

    let unknown_job = run_code(&[
        "run",
        "-n",
        "no-such-job",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(unknown_job.2, EXIT_CONFIG_ERROR, "an unknown job exits 2");
}

#[test]
fn config_check_never_needs_the_rustic_binary() {
    // Validation is hermetic: no rustic on PATH, no repository, no network. Running it
    // must be safe on any machine at any time.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let output = Command::new(env!("CARGO_BIN_EXE_rusticprofile"))
        .args([
            "config",
            "--check",
            "--config",
            &path,
            "--as-host",
            "host-a",
        ])
        .env("PATH", "/nonexistent")
        .output()
        .expect("failed to execute binary");
    assert_eq!(output.status.code(), Some(0));
}
