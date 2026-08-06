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
///
/// Since `0.1.34` rusticprofile *records* this name (`--host` on backup, `--filter-host` on
/// forget/prune), so what this function returns is what ends up in the repository and what
/// retention groups by. That makes the choice of source a data-integrity decision on every
/// platform, not a portability detail — see [`raw_hostname`] for the Windows half.
pub fn current_hostname() -> Result<String> {
    let host = raw_hostname()?
        .into_string()
        .map_err(|_| anyhow::anyhow!("the system hostname is not valid UTF-8"))?;
    if host.is_empty() {
        anyhow::bail!("the system hostname is empty");
    }
    Ok(host)
}

/// The hostname as the OS reports it.
#[cfg(unix)]
fn raw_hostname() -> Result<std::ffi::OsString> {
    nix::unistd::gethostname().context("could not read the system hostname")
}

/// The hostname as the OS reports it.
///
/// **`%COMPUTERNAME%` is not this value, and reaching for it is the trap.** That variable holds
/// the *NetBIOS* name, which Windows stores upper-cased — measured on a host where
/// `COMPUTERNAME` is `HOST-A` while `hostname` and rustic both say `host-a`. Two things break
/// on the difference, and neither is loud:
///
/// - [`host_matches`] is plain equality, so `enabled-on-hosts: [host-a, …]` would stop
///   selecting this host and its gated snapshot sets would simply not run — a backup quietly
///   doing less than it says, which is the failure class this project exists to prevent.
/// - `--host HOST-A` under `group-by = "host,label"` is a *different retention group* from the
///   host's existing history, so the old snapshots stop being selected and accumulate forever
///   (`NOTES.md` §3a invariant 1, and the same split `0.1.34` documents for a changed name).
///
/// So this asks for `ComputerNamePhysicalDnsHostname`, which is the name with its configured
/// case intact — the same value `hostname.exe` prints and rustic would have recorded on its
/// own. Lower-casing `COMPUTERNAME` was rejected: it happens to be right here and would
/// silently rename a host genuinely called `Web1`.
#[cfg(windows)]
fn raw_hostname() -> Result<std::ffi::OsString> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::{
        COMPUTER_NAME_FORMAT, ComputerNamePhysicalDnsHostname, GetComputerNameExW,
    };

    /// Ask once for the length, then once for the value.
    ///
    /// The first call is *expected* to fail with `ERROR_MORE_DATA` and set `len`; treating that
    /// as an error is the usual way this API is got wrong.
    fn query(format: COMPUTER_NAME_FORMAT) -> Result<std::ffi::OsString> {
        let mut len: u32 = 0;
        // SAFETY: a null buffer with a zero length is the documented way to ask for the
        // required size; nothing is written through the pointer.
        unsafe { GetComputerNameExW(format, std::ptr::null_mut(), &mut len) };
        if len == 0 {
            anyhow::bail!("could not read the system hostname (GetComputerNameExW gave no size)");
        }

        let mut buf = vec![0u16; len as usize];
        // SAFETY: `buf` has room for `len` UTF-16 code units, which is what `len` was just
        // reported to require, and `len` is updated to the count actually written.
        let ok = unsafe { GetComputerNameExW(format, buf.as_mut_ptr(), &mut len) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error())
                .context("could not read the system hostname");
        }
        buf.truncate(len as usize);
        Ok(std::ffi::OsString::from_wide(&buf))
    }

    query(ComputerNamePhysicalDnsHostname)
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

    #[cfg(windows)]
    #[test]
    fn the_windows_hostname_names_this_machine() {
        // Guards the `%COMPUTERNAME%` trap from the other side: the two must identify the same
        // machine, so a wrong source (an empty read, a truncated buffer, a stray suffix) fails
        // here. The *case* difference is the whole reason the NetBIOS variable is not used, and
        // it cannot be asserted portably — a machine genuinely named in upper case is legal.
        let host = current_hostname().expect("a Windows host has a hostname");
        assert!(!host.is_empty());
        if let Some(netbios) = std::env::var_os("COMPUTERNAME").and_then(|v| v.into_string().ok()) {
            assert_eq!(
                host.to_lowercase(),
                netbios.to_lowercase(),
                "the DNS hostname and the NetBIOS name must name the same machine"
            );
        }
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
