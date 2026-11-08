**rusticprofile** is a local, per-machine scheduler and orchestrator for [rustic](https://rustic.cli.rs) backups, written in Rust.

It owns **when** backups run, **which** jobs exist, and **on which hosts**:

* **Schedules**: installs, removes, and reports status for **systemd** timer/service units on Linux, **launchd** agents on macOS, and **Task Scheduler** tasks on Windows
* **Per-host job gating**: one machine runs the prune job while others only write snapshots
* **Execution**: sequences operations within a job (`backup`, then `forget`), takes a local lock, classifies rustic exit states (telling partial backups apart from failed ones), and retries transient wake-from-sleep failures when started by the scheduler
* **Observability**: writes a per-run log and a status file whose `last_success` timestamp survives failed runs, plus `retention` previews and `doctor` diagnostics (with `--json` for monitoring)

Backup repository configuration itself is delegated to rustic's native TOML profiles (`~/.config/rustic/*.toml`).

### Links

* [Source and documentation](https://github.com/l1a/rusticprofile)
* [crates.io package](https://crates.io/crates/rusticprofile)

Licensed **GPL-3.0-or-later**.
