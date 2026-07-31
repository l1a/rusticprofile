// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hostname resolution and per-host job gating.
//!
//! Per-host variation is the reason this tool exists: rustic's TOML has no hostname
//! conditionals, and the template gate this replaces rendered a schedule expression to
//! the empty string on hosts where it did not apply — which is indistinguishable from a
//! job that is present and simply never fires. Here, `enabled-on-hosts` **removes the job
//! entirely**, and the removal is reported rather than assumed.

use anyhow::{Context, Result};

/// The current machine's hostname.
pub fn current_hostname() -> Result<String> {
    let raw = nix::unistd::gethostname().context("could not read the system hostname")?;
    let host = raw
        .into_string()
        .map_err(|_| anyhow::anyhow!("the system hostname is not valid UTF-8"))?;
    if host.is_empty() {
        anyhow::bail!("the system hostname is empty");
    }
    Ok(host)
}

/// The hostname up to its first dot.
///
/// Two machines in the fleet report dotted names (`host-a.local`, `host-b.local`) while the
/// rest are bare. Anything that needs a stable short label — a unit file name, a log file
/// name — wants this form, which is why `${host_short}` exists as its own variable.
pub fn short(host: &str) -> &str {
    host.split('.').next().unwrap_or(host)
}

/// Whether a hostname listed in `enabled-on-hosts` selects the machine named `host`.
///
/// Matching accepts either the full name or the short form, so `host-a` and `host-a.local`
/// both select `host-a.local`. The alternative — exact match only — turns a perfectly
/// reasonable-looking config into a job that silently never runs, and silence is the
/// failure mode this project is built to avoid.
pub fn host_matches(listed: &str, host: &str) -> bool {
    listed == host || listed == short(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_strips_the_domain_part() {
        assert_eq!(short("host-a.local"), "host-a");
        assert_eq!(short("host-c"), "host-c");
        assert_eq!(short(""), "");
    }

    #[test]
    fn matching_accepts_both_full_and_short_forms() {
        assert!(host_matches("host-a.local", "host-a.local"));
        assert!(host_matches("host-a", "host-a.local"));
        assert!(host_matches("host-c", "host-c"));
    }

    #[test]
    fn matching_rejects_a_different_host() {
        assert!(!host_matches("host-c", "host-a.local"));
        assert!(!host_matches("host-a.localhost", "host-a.local"));
        // A short listing must not match a *different* machine that merely shares a
        // prefix — this is a plain equality check on the first label, not a prefix test.
        assert!(!host_matches("host", "host-a.local"));
    }
}
