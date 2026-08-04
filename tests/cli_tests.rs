// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests driving the real binary.
//!
//! No `assert_cmd` / `predicates` — the binary path comes from `CARGO_BIN_EXE_*`, which
//! Cargo sets for integration tests, and `std::process::Command` does the rest.

use std::process::Command;

/// A scratch state directory shared by every child this file spawns.
///
/// **This exists to stop the test suite writing into the real one, and it is not cosmetic.**
/// `run` records each job at `$XDG_STATE_HOME/rusticprofile/status/<job>.json`, and these
/// fixtures use the job name `dot-files` — which is also the live hourly job on this fleet. So
/// `cargo test` on such a host overwrote the real record with a fixture's, claiming a success
/// on `host-a` that never happened. That destroys the `last_success` history whose entire
/// purpose is revealing a job that has quietly stopped working, and replaces it with a
/// fabrication a monitor would believe. Observed on a real host, seconds after a test run.
///
/// Redirected through `XDG_STATE_HOME` rather than a new flag: that variable is already the
/// documented contract (see the man page's FILES section), so this needs no product surface,
/// and it is honoured on macOS as well as Linux since 0.1.25.
///
/// **One choke point, deliberately.** Every spawn in this file goes through [`command`], so a
/// test added later cannot forget to be hermetic — which a per-test flag could not promise.
fn state_dir() -> &'static std::path::Path {
    use std::sync::OnceLock;
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().expect("scratch state dir"))
        .path()
}

/// The binary, with state redirected away from the developer's real one.
///
/// Use this rather than `Command::new(env!(...))` directly.
fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rusticprofile"));
    cmd.env("XDG_STATE_HOME", state_dir());
    cmd
}

/// Run the binary with `args`, returning (stdout, stderr, success).
fn run(args: &[&str]) -> (String, String, bool) {
    let output = command()
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
    // A bare invocation prints help, as every other CLI does — but must NOT exit 0. A
    // silent success here is exactly the failure mode this project exists to prevent, and
    // a systemd unit or wrapper script would happily believe it.
    let (_stdout, stderr, success) = run(&[]);
    assert!(!success, "bare invocation must not report success");
    assert!(stderr.contains("Usage:"), "help should be shown: {stderr}");
    assert!(stderr.contains("Commands:"), "{stderr}");
}

/// Exit code meaning "the configuration is wrong", as distinct from a failed run.
const EXIT_CONFIG_ERROR: i32 = 2;

/// Run the binary and return (stdout, stderr, exit code).
fn run_code(args: &[&str]) -> (String, String, i32) {
    let output = command()
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
filter-hosts = ["host-a", "host-b", "host-c", "THIS_HOST"]

[forget]
group-by = "host,label,paths"
"#;

/// Write a jobs.yaml and a rustic profile into a fresh temp dir.
///
/// `RUSTIC_DIR` in the YAML is replaced with the temp dir, so fixtures stay hermetic and
/// never read the developer's real `~/.config/rustic`.
fn fixture(jobs_yaml: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    // `THIS_HOST` becomes the real hostname. Commands that resolve the host themselves
    // (`schedule`, `run`) have no `--as-host`, so a profile whose `filter-hosts` cannot
    // match the running machine is now a load-time error — correctly, since that is the
    // silent-retention bug. A fixture for those commands therefore has to name this host.
    // Ask the same source of truth the binary will. Shelling out to `hostname(1)` looked
    // equivalent and is not: the Fedora CI container has no such binary, so the fallback
    // was used while the binary itself resolved the container id — and the fixture then
    // disagreed with the program under test on every containerised runner.
    let host = rusticprofile::config::hosts::current_hostname().expect("hostname");
    std::fs::write(
        dir.path().join("p.toml"),
        PROFILE_TOML.replace("THIS_HOST", &host),
    )
    .unwrap();
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
    let output = command()
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
    let mut cmd = command();
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
        None,
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
        None,
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
        None,
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
fn a_run_records_its_status_under_xdg_state_home_and_nowhere_else() {
    // The mechanism every other test in this file silently depends on, asserted once so it
    // cannot rot: `run` must write its record under $XDG_STATE_HOME.
    //
    // This is a regression test for real damage, not for tidiness. These fixtures use the job
    // name `dot-files`, which is the live hourly job on this fleet, so before the harness
    // redirected state a plain `cargo test` overwrote that job's real record with a fixture's
    // — a fabricated success on `host-a`, replacing the `last_success` history that exists to
    // reveal a job which has quietly stopped working.
    let job = "state-dir-job";
    let (dir, path) = fixture(&run_config_named(job));
    let shim = recording_shim(dir.path());
    let state = dir.path().join("state");

    let output = Command::new(env!("CARGO_BIN_EXE_rusticprofile"))
        .args([
            "run",
            "-n",
            job,
            "--config",
            &path,
            "--as-host",
            "host-a",
            "--rustic-binary",
            &shim,
        ])
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("failed to execute binary");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let record = state
        .join("rusticprofile/status")
        .join(format!("{job}.json"));
    assert!(
        record.is_file(),
        "the record must land under XDG_STATE_HOME, not the real state directory: {}",
        record.display()
    );
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(text.contains(job), "{text}");

    // And `status` reading the same tree must find it — the reader and the writer have to
    // agree on which file is the record, which is why the directory is resolved once and
    // carried on `Config` rather than re-derived per command.
    let seen = Command::new(env!("CARGO_BIN_EXE_rusticprofile"))
        .args(["status", "--config", &path, "--as-host", "host-a"])
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&seen.stdout);
    assert!(
        stdout.contains("last success"),
        "status must read back what run wrote: {stdout}"
    );
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
    let output = command()
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

// --- `config --example` ------------------------------------------------------------
//
// The point of these tests is that the shipped examples are not merely plausible: they
// pass the tool's own validator. An example that has drifted out of step with the rules
// it is meant to demonstrate is worse than no example, because it is quoted with
// authority.

#[test]
fn config_example_emits_to_stdout_without_needing_a_configuration() {
    // An example is what you ask for when you have no configuration yet, so it must not
    // require one — and it must be hermetic, like every other `config` mode.
    for what in ["jobs", "rustic"] {
        let output = command()
            .args(["config", "--example", what])
            .env("PATH", "/nonexistent")
            .env("XDG_CONFIG_HOME", "/nonexistent")
            .output()
            .expect("failed to execute binary");
        assert_eq!(
            output.status.code(),
            Some(0),
            "--example {what} should exit 0"
        );
        assert!(
            !output.stdout.is_empty(),
            "--example {what} should write to stdout"
        );
    }
}

#[test]
fn the_emitted_examples_pass_config_check() {
    // Write both examples out as a real user would, substituting only the directory, and
    // validate them through the real binary. This is the test that keeps the examples
    // honest as validation rules change.
    let dir = tempfile::tempdir().expect("temp dir");

    let jobs_text = String::from_utf8(
        command()
            .args(["config", "--example", "jobs"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let rustic_text = String::from_utf8(
        command()
            .args(["config", "--example", "rustic"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let rustic_dir = dir.path().display().to_string();
    std::fs::write(dir.path().join("dot-files.toml"), &rustic_text).unwrap();

    // Only the profile directory is redirected — every other value is the example as
    // shipped, placeholders and all.
    let jobs_path = dir.path().join("jobs.yaml");
    std::fs::write(
        &jobs_path,
        jobs_text.replace(
            r#"rustic-config-dir: "${env:HOME}/.config/rustic""#,
            &format!("rustic-config-dir: \"{rustic_dir}\""),
        ),
    )
    .unwrap();

    // host-a is the placeholder the examples gate on, so it is the host they describe.
    let (stdout, stderr, code) = run_code(&[
        "config",
        "--check",
        "--config",
        &jobs_path.display().to_string(),
        "--as-host",
        "host-a",
    ]);
    assert_eq!(
        code, 0,
        "the shipped examples must validate.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("dot-files"));
}

#[test]
fn the_emitted_examples_plan_a_real_argv() {
    // Validation proving the files parse is not the same as proving they describe a
    // runnable job. `plan` builds the argv rustic would actually receive.
    let dir = tempfile::tempdir().expect("temp dir");
    let rustic_text = String::from_utf8(
        command()
            .args(["config", "--example", "rustic"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let jobs_text = String::from_utf8(
        command()
            .args(["config", "--example", "jobs"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::fs::write(dir.path().join("dot-files.toml"), rustic_text).unwrap();
    let jobs_path = dir.path().join("jobs.yaml");
    std::fs::write(
        &jobs_path,
        jobs_text.replace(
            r#"rustic-config-dir: "${env:HOME}/.config/rustic""#,
            &format!("rustic-config-dir: \"{}\"", dir.path().display()),
        ),
    )
    .unwrap();

    let (stdout, stderr, code) = run_code(&[
        "plan",
        "-n",
        "dot-files",
        "--format",
        "lines",
        "--config",
        &jobs_path.display().to_string(),
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    assert!(stdout.contains("backup"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("--name") && stdout.contains("core") && stdout.contains("gnupg"),
        "both sets are enabled on host-a, so both must appear:\n{stdout}"
    );
}

// --- platform guard ----------------------------------------------------------------
//
// Only systemd is implemented. Without a guard, `schedule` on macOS writes systemd units
// into a directory launchd never reads, prints the files it created and exits 0 — which is
// exactly what a working install looks like. CI runs `cargo test` on macOS, so this test
// verifies the real behaviour on whichever platform it runs on rather than asserting one.

/// Like `GOOD_CONFIG`, but naming a rustic that exists on every runner.
///
/// `schedule` resolves the rustic binary to an absolute path so the generated unit does not
/// depend on the service manager's `PATH`, and refuses when it cannot — CI has no rustic
/// installed. `/bin/sh` is present on Linux and macOS alike and is never executed here; only
/// its existence is checked. Weakening the resolver for tests would remove the guarantee
/// this test is meant to protect.
const SCHEDULABLE_CONFIG: &str = "
schema: 1
defaults:
  rustic-config-dir: RUSTIC_DIR
  rustic-binary: /bin/sh
jobs:
  dot-files:
    profile: p
    operations: [backup, forget]
    snapshot-sets:
      - name: core
    schedule:
      at: hourly
";

/// What `schedule` should produce on the platform running this test.
///
/// One job is **two** systemd units and **one** launchd agent, because systemd cannot run a
/// command from a timer and launchd can. On a platform with neither, it is a refusal and no
/// files at all — which is the case that must never regress, since files on disk plus a
/// success message are indistinguishable from a working install.
fn schedule_backend_exists() -> bool {
    expected_schedule_files().is_some()
}

fn expected_schedule_files() -> Option<usize> {
    if cfg!(target_os = "linux") {
        Some(2)
    } else if cfg!(target_os = "macos") {
        Some(1)
    } else {
        None
    }
}

#[test]
fn schedule_writes_what_this_platforms_backend_needs() {
    let (dir, path) = fixture(SCHEDULABLE_CONFIG);
    let unit_dir = dir.path().join("units");

    let output = command()
        .args([
            "schedule",
            "-n",
            "dot-files",
            "--config",
            &path,
            "--unit-dir",
            &unit_dir.display().to_string(),
        ])
        .output()
        .expect("failed to execute binary");
    let stderr = String::from_utf8_lossy(&output.stderr);

    let written = std::fs::read_dir(&unit_dir).map(|d| d.count()).unwrap_or(0);

    match expected_schedule_files() {
        Some(expected) => {
            assert_eq!(output.status.code(), Some(0), "stderr:\n{stderr}");
            assert_eq!(
                written, expected,
                "this platform's backend needs {expected} file(s).\nstderr:\n{stderr}"
            );
        }
        None => {
            // The refusal is the whole point: no files, and a message that says which
            // backends exist rather than a bare "unsupported".
            assert_eq!(
                output.status.code(),
                Some(2),
                "scheduling must be refused where it cannot work.\nstderr:\n{stderr}"
            );
            assert_eq!(
                written, 0,
                "no unit may be written on a platform that cannot run it"
            );
            assert!(stderr.contains("systemd"), "stderr:\n{stderr}");
            assert!(stderr.contains("launchd"), "stderr:\n{stderr}");
        }
    }
}

#[test]
fn a_custom_unit_dir_never_arms_anything() {
    // `--unit-dir` is an inspection target. It must not reach `systemctl` or `launchctl` —
    // writing somewhere a service manager does not read and then telling it to load from
    // where it does would arm something nobody asked about.
    if expected_schedule_files().is_none() {
        return; // nothing is written at all here; covered by the test above
    }
    let (dir, path) = fixture(SCHEDULABLE_CONFIG);
    let unit_dir = dir.path().join("units");
    let (stdout, stderr, code) = run_code(&[
        "schedule",
        "-n",
        "dot-files",
        "--config",
        &path,
        "--unit-dir",
        &unit_dir.display().to_string(),
    ]);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    assert!(
        stdout.contains("not armed"),
        "it must say the schedule is inert: {stdout}"
    );
}

#[test]
fn schedule_writes_a_plist_launchd_would_accept() {
    // Substring assertions on the generator prove the content; only a parser proves the
    // file. This is the same check the unit tests run, but on what the *binary* actually
    // wrote — the two could differ, and this is the one that reaches a real disk.
    if !cfg!(target_os = "macos") {
        println!("skipping: launchd agents are macOS-only");
        return;
    }
    let (dir, path) = fixture(SCHEDULABLE_CONFIG);
    let unit_dir = dir.path().join("units");
    let (_out, stderr, code) = run_code(&[
        "schedule",
        "-n",
        "dot-files",
        "--config",
        &path,
        "--unit-dir",
        &unit_dir.display().to_string(),
    ]);
    assert_eq!(code, 0, "stderr:\n{stderr}");

    let plist = unit_dir.join("local.rusticprofile.dot-files.plist");
    assert!(plist.exists(), "expected {}", plist.display());
    let lint = Command::new("plutil")
        .arg("-lint")
        .arg(&plist)
        .output()
        .expect("plutil should be present on macOS");
    assert!(
        lint.status.success(),
        "plutil rejected the written agent:\n{}\n{}",
        String::from_utf8_lossy(&lint.stdout),
        std::fs::read_to_string(&plist).unwrap_or_default()
    );
}

#[test]
fn rescheduling_reports_no_change_rather_than_moving_the_spread() {
    // The offset is chosen at schedule time on launchd, so without reuse a second `schedule`
    // would rewrite an unchanged agent, move this host's slot and report `installed` — and
    // then `unchanged` would never mean anything. Asserted through the binary because the
    // reuse happens in the command, not in the generator.
    if expected_schedule_files().is_none() {
        return;
    }
    let (dir, path) = fixture(SCHEDULABLE_CONFIG);
    let unit_dir = dir.path().join("units");
    let args = [
        "schedule",
        "-n",
        "dot-files",
        "--config",
        &path,
        "--unit-dir",
        &unit_dir.display().to_string(),
    ];
    let (first, stderr, code) = run_code(&args);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    assert!(first.contains("installed:"), "{first}");

    let before: Vec<String> = std::fs::read_dir(&unit_dir)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();

    let (second, stderr, code) = run_code(&args);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    assert!(
        second.contains("unchanged:"),
        "re-running must report no change: {second}"
    );

    let after: Vec<String> = std::fs::read_dir(&unit_dir)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert_eq!(before, after, "the files must be byte-identical");
}

#[test]
fn status_reports_the_backend_rather_than_failing() {
    // `status` asks "what is scheduled here". Every platform gets a truthful answer: the
    // backend's name where there is one, and "nothing, and here is why" where there is not —
    // an answer, not an error.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, stderr, code) = run_code(&["status", "--config", &path, "--as-host", "host-a"]);
    assert_eq!(code, 0, "status must not fail anywhere.\nstderr:\n{stderr}");

    if cfg!(target_os = "linux") {
        assert!(stdout.contains("systemd"), "stdout:\n{stdout}");
    } else if cfg!(target_os = "macos") {
        assert!(stdout.contains("launchd"), "stdout:\n{stdout}");
        // The one thing a macOS schedule cannot promise has to be visible here, because this
        // is the command someone runs to ask whether backups are happening.
        assert!(
            stdout.contains("linger"),
            "the login limitation must be stated: {stdout}"
        );
    } else {
        assert!(stdout.contains("not implemented"), "stdout:\n{stdout}");
    }
}

#[test]
fn status_json_names_the_backend_so_a_null_next_run_is_explicable() {
    // `next_run` is always null under launchd, because launchd reports no next fire time.
    // Without this field a monitor cannot tell that from a timer it failed to read, and the
    // two call for different alerts.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, stderr, code) =
        run_code(&["status", "--json", "--config", &path, "--as-host", "host-a"]);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("status --json must parse");
    assert_eq!(
        value["schema"], 1,
        "adding a field must not bump the schema"
    );

    let expected = if cfg!(target_os = "linux") {
        serde_json::json!("systemd")
    } else if cfg!(target_os = "macos") {
        serde_json::json!("launchd")
    } else {
        serde_json::Value::Null
    };
    assert_eq!(value["backend"], expected, "stdout:\n{stdout}");
}

#[test]
fn schedule_refuses_when_rustic_cannot_be_resolved() {
    // The unit must carry an absolute path to rustic, because a service manager's PATH is
    // not the shell's. Measured on both: with `linger` systemd's user manager starts at boot
    // with `/usr/local/bin:/usr/bin`, and a launchd agent gets
    // `/usr/bin:/bin:/usr/sbin:/sbin`. If rustic cannot be found there is no correct
    // unit or agent to write, so nothing is written.
    //
    // Runs on macOS as well now that launchd is implemented — previously it returned early
    // there, because `schedule` refused for an unrelated reason and the check was untestable.
    if !schedule_backend_exists() {
        return; // `schedule` refuses first, for a different reason
    }
    let (dir, path) = fixture(GOOD_CONFIG); // no `rustic-binary`, and no rustic on PATH
    let unit_dir = dir.path().join("units");
    let output = command()
        .args([
            "schedule",
            "-n",
            "dot-files",
            "--config",
            &path,
            "--unit-dir",
            &unit_dir.display().to_string(),
        ])
        .env("PATH", "/nonexistent")
        .output()
        .expect("failed to execute binary");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "stderr:\n{stderr}");
    assert!(
        stderr.contains("linger"),
        "the reason must be named: {stderr}"
    );
    assert_eq!(
        std::fs::read_dir(&unit_dir).map(|d| d.count()).unwrap_or(0),
        0,
        "no unit may be written when it could not be written correctly"
    );
}

#[test]
fn as_host_does_not_check_a_profile_it_cannot_see() {
    // `filter-hosts` lives in *this* machine's rustic profile, and §5.9 requires that file
    // to differ per host. Under `--as-host` the check would compare a profile from one disk
    // against a hostname from another and report a defect on every host but this one —
    // making `--as-host` useless for the gate inspection it exists for.
    let (_dir, path) = fixture(GOOD_CONFIG); // filter-hosts names host-a/b/c and THIS_HOST
    let (stdout, stderr, code) = run_code(&[
        "config",
        "--check",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(
        code, 0,
        "simulating another host must not error.\nstderr:\n{stderr}"
    );
    // ...but a skipped check has to be visible, or this is just a silent pass.
    assert!(
        stdout.contains("NOT checked"),
        "the skip must be reported: {stdout}"
    );
}

#[test]
fn checking_this_host_still_validates_filter_hosts() {
    // The skip is scoped to simulation. On the real host the check must still run, or the
    // silent-retention bug it exists for comes back.
    //
    // `hostname: rustic` is required from 0.1.34 for this rule to exist at all: under the
    // default `short` rusticprofile passes its own `--filter-host` and the CLI overrides
    // the file, so a wrong `filter-hosts` cannot cause the failure this guards. The rule
    // applies exactly where the profile is the only scope there is.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("p.toml"),
        "[snapshot-filter]\nfilter-hosts = [\"a-host-that-is-not-this-one\"]\n\n\
         [forget]\ngroup-by = \"host,label\"\n\n\
         [[backup.snapshots]]\nname = \"core\"\nsources = [\"/etc\"]\n",
    )
    .unwrap();
    let jobs = dir.path().join("jobs.yaml");
    std::fs::write(
        &jobs,
        format!(
            "schema: 1\n\
             defaults:\n\
             \x20 hostname: rustic\n\
             \x20 rustic-config-dir: {}\n\
             jobs:\n\
             \x20 j:\n\
             \x20   profile: p\n\
             \x20   operations: [backup, forget]\n\
             \x20   snapshot-sets: [{{name: core}}]\n",
            dir.path().display()
        ),
    )
    .unwrap();

    let (_stdout, stderr, code) =
        run_code(&["config", "--check", "--config", &jobs.display().to_string()]);
    assert_eq!(code, 2, "stderr:\n{stderr}");
    assert!(stderr.contains("filter-hosts"), "{stderr}");
}

#[test]
fn snapshots_is_a_passthrough_that_adds_only_the_profile() {
    // The command's entire justification is profile resolution. `plan` renders the argv a
    // job would run; this asserts the query form stays minimal by checking the one thing
    // that can be checked without rustic present — that a bad job name is refused before
    // anything is spawned, and the host gate is still reported.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (_stdout, stderr, code) = run_code(&[
        "snapshots",
        "-n",
        "dot-files-prune",
        "--config",
        &path,
        "--as-host",
        "host-a",
    ]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
    assert!(stderr.contains("not enabled on"), "{stderr}");
}

#[test]
fn snapshots_needs_a_job_name() {
    // No "query everything" form: a passthrough without a profile has nothing to resolve,
    // which is the only thing this command exists to do.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (_stdout, _stderr, code) = run_code(&["snapshots", "--config", &path]);
    assert_eq!(code, EXIT_CONFIG_ERROR);
}

#[test]
fn status_json_is_parseable_and_carries_a_schema() {
    // The point of --json is that nothing downstream has to match English. If this ever
    // emits something a parser chokes on, the flag is worse than useless — a consumer
    // would have been better off with the human output it could at least read.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, stderr, code) =
        run_code(&["status", "--json", "--config", &path, "--as-host", "host-a"]);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("status --json must emit parseable JSON");
    assert_eq!(v["schema"], 1);
    assert_eq!(v["host"], "host-a");
    assert!(v["jobs"].is_array(), "{v}");
}

#[test]
fn status_json_reports_the_host_gate_rather_than_omitting_it() {
    // Same rule as the human output: "this host has no prune job" must be readable as a
    // decision, not inferred from a shorter list.
    let (_dir, path) = fixture(GOOD_CONFIG);
    let (stdout, _stderr, _code) =
        run_code(&["status", "--json", "--config", &path, "--as-host", "host-a"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["not_on_this_host"][0]["job"], "dot-files-prune");
}
