// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! `${…}` substitution over already-parsed strings.
//!
//! **This is not a template language and must never become one.** There are no
//! conditionals, functions, pipelines or loops — not even "just one small conditional".
//! The predecessor rendered its entire config as a Go template *before* parsing it, which
//! produced two separate silent failures: a `{{ if }}` written inside a `#` comment was
//! still compiled, and a structural `{{ if }}` block made the file invalid YAML to a
//! second parser that had to read it raw.
//!
//! Both are structurally impossible here, because substitution runs **after** the YAML
//! parse. Comments are gone by then; a `${…}` in one is never seen.
//!
//! The variable set is closed. An unrecognised name is a load-time error that names the
//! offending key and lists what is valid — it is never passed through, and never resolves
//! to the empty string.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// Every variable name accepted outside the `env:` and `date:` prefixes.
const SIMPLE_VARIABLES: &[&str] = &[
    "host",
    "host_short",
    "job",
    "profile",
    "config_dir",
    "temp_dir",
    "os",
    "arch",
];

/// Where `${env:NAME}` reads from.
///
/// Tests use [`Env::Fixed`] rather than mutating the process environment, which would be
/// shared across threads and make results depend on test execution order.
pub enum Env<'a> {
    System,
    Fixed(&'a BTreeMap<String, String>),
}

impl Env<'_> {
    fn get(&self, name: &str) -> Option<String> {
        match self {
            Env::System => std::env::var(name).ok(),
            Env::Fixed(map) => map.get(name).cloned(),
        }
    }
}

/// Everything a `${…}` reference can resolve against.
pub struct Ctx<'a> {
    pub host: &'a str,
    pub host_short: &'a str,
    /// `None` outside a job — e.g. while resolving `defaults`, where `${job}` has no
    /// meaning. Referencing it there is an error rather than an empty string.
    pub job: Option<&'a str>,
    /// `None` outside a job, for the same reason as [`Ctx::job`].
    pub profile: Option<&'a str>,
    pub config_dir: &'a Path,
    pub temp_dir: &'a Path,
    pub env: Env<'a>,
    /// Clock for `${date:…}`.
    ///
    /// `None` leaves `${date:…}` **unresolved**, re-emitted verbatim. That is what makes
    /// a generated systemd unit correct: baking today's date into a unit file would
    /// freeze it at install time and every later run would write to the wrong path. The
    /// runner passes `Some` and resolves it per run; unit generation passes `None`.
    pub now: Option<&'a jiff::Zoned>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InterpError {
    UnknownVariable(String),
    UnsetEnv(String),
    EmptyEnvName,
    EmptyDateFormat,
    BadDateFormat {
        format: String,
        reason: String,
    },
    EmptyVariable,
    Unterminated,
    /// A valid variable used somewhere it has no value.
    NotAvailableHere(String),
}

impl fmt::Display for InterpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InterpError::UnknownVariable(name) => write!(
                f,
                "unknown variable `${{{name}}}`; valid names are {}, ${{env:NAME}} and ${{date:FORMAT}}",
                SIMPLE_VARIABLES
                    .iter()
                    .map(|v| format!("${{{v}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            InterpError::UnsetEnv(name) => write!(
                f,
                "environment variable `{name}` referenced by ${{env:{name}}} is not set"
            ),
            InterpError::EmptyEnvName => write!(f, "`${{env:}}` is missing a variable name"),
            InterpError::EmptyDateFormat => write!(f, "`${{date:}}` is missing a format string"),
            InterpError::BadDateFormat { format, reason } => {
                write!(
                    f,
                    "`${{date:{format}}}` is not a valid time format: {reason}"
                )
            }
            InterpError::EmptyVariable => write!(f, "`${{}}` is missing a variable name"),
            InterpError::Unterminated => {
                write!(
                    f,
                    "unterminated `${{` — every reference needs a closing `}}`"
                )
            }
            InterpError::NotAvailableHere(name) => write!(
                f,
                "`${{{name}}}` has no value here; it is only available inside a job definition"
            ),
        }
    }
}

/// Substitute every `${…}` reference in `input`.
///
/// `$${` produces a literal `${`, which is the only escape. A lone `$` is literal.
pub fn interpolate(input: &str, ctx: &Ctx) -> Result<String, InterpError> {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if input[i..].starts_with("$${") {
            out.push_str("${");
            i += 3;
            continue;
        }
        if input[i..].starts_with("${") {
            let rest = &input[i + 2..];
            let close = rest.find('}').ok_or(InterpError::Unterminated)?;
            let name = &rest[..close];
            out.push_str(&resolve(name, ctx)?);
            i += 2 + close + 1;
            continue;
        }
        // Not a reference: copy one whole character, not one byte, so multi-byte text
        // survives intact.
        let ch = input[i..]
            .chars()
            .next()
            .expect("index is on a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }

    Ok(out)
}

fn resolve(name: &str, ctx: &Ctx) -> Result<String, InterpError> {
    if name.is_empty() {
        return Err(InterpError::EmptyVariable);
    }

    if let Some(var) = name.strip_prefix("env:") {
        if var.is_empty() {
            return Err(InterpError::EmptyEnvName);
        }
        return ctx
            .env
            .get(var)
            .ok_or_else(|| InterpError::UnsetEnv(var.to_string()));
    }

    if let Some(format) = name.strip_prefix("date:") {
        if format.is_empty() {
            return Err(InterpError::EmptyDateFormat);
        }
        // Validate against a fixed reference instant whether or not it is being resolved
        // now, so a malformed format is caught at load time rather than at 03:00 on the
        // night the backup runs.
        let reference = jiff::Timestamp::UNIX_EPOCH.to_zoned(jiff::tz::TimeZone::UTC);
        jiff::fmt::strtime::format(format, &reference).map_err(|e| InterpError::BadDateFormat {
            format: format.to_string(),
            reason: e.to_string(),
        })?;

        return match ctx.now {
            Some(now) => Ok(jiff::fmt::strtime::format(format, now)
                .expect("format already validated against a reference instant")),
            None => Ok(format!("${{date:{format}}}")),
        };
    }

    match name {
        "host" => Ok(ctx.host.to_string()),
        "host_short" => Ok(ctx.host_short.to_string()),
        "job" => ctx
            .job
            .map(str::to_string)
            .ok_or_else(|| InterpError::NotAvailableHere("job".to_string())),
        "profile" => ctx
            .profile
            .map(str::to_string)
            .ok_or_else(|| InterpError::NotAvailableHere("profile".to_string())),
        "config_dir" => Ok(ctx.config_dir.to_string_lossy().into_owned()),
        "temp_dir" => Ok(ctx.temp_dir.to_string_lossy().into_owned()),
        "os" => Ok(std::env::consts::OS.to_string()),
        "arch" => Ok(std::env::consts::ARCH.to_string()),
        other => Err(InterpError::UnknownVariable(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ctx<'a>(env: &'a BTreeMap<String, String>, now: Option<&'a jiff::Zoned>) -> Ctx<'a> {
        Ctx {
            host: "host-a.local",
            host_short: "host-a",
            job: Some("dot-files"),
            profile: Some("dot-files-profile"),
            config_dir: Path::new("/cfg"),
            temp_dir: Path::new("/tmp"),
            env: Env::Fixed(env),
            now,
        }
    }

    #[test]
    fn every_simple_variable_resolves() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(interpolate("${host}", &c).unwrap(), "host-a.local");
        assert_eq!(interpolate("${host_short}", &c).unwrap(), "host-a");
        assert_eq!(interpolate("${job}", &c).unwrap(), "dot-files");
        assert_eq!(interpolate("${profile}", &c).unwrap(), "dot-files-profile");
        assert_eq!(interpolate("${config_dir}", &c).unwrap(), "/cfg");
        assert_eq!(interpolate("${temp_dir}", &c).unwrap(), "/tmp");
        assert_eq!(
            interpolate("${os}", &c).unwrap(),
            std::env::consts::OS.to_string()
        );
        assert_eq!(
            interpolate("${arch}", &c).unwrap(),
            std::env::consts::ARCH.to_string()
        );
    }

    #[test]
    fn the_variable_list_in_the_error_matches_what_actually_resolves() {
        // Guards against SIMPLE_VARIABLES and the match arms drifting apart, which would
        // make the error message advertise a name that does not work, or omit one that
        // does.
        let env = env_map(&[]);
        let c = ctx(&env, None);
        for name in SIMPLE_VARIABLES {
            assert!(
                interpolate(&format!("${{{name}}}"), &c).is_ok(),
                "`{name}` is advertised in the error message but does not resolve"
            );
        }
    }

    #[test]
    fn text_around_references_is_preserved() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(
            interpolate("/var/log/${job}-${host_short}.log", &c).unwrap(),
            "/var/log/dot-files-host-a.log"
        );
    }

    #[test]
    fn env_resolves_and_reports_unset_names() {
        let env = env_map(&[("HOME", "/home/k")]);
        let c = ctx(&env, None);
        assert_eq!(interpolate("${env:HOME}/x", &c).unwrap(), "/home/k/x");
        assert_eq!(
            interpolate("${env:NOPE}", &c).unwrap_err(),
            InterpError::UnsetEnv("NOPE".to_string())
        );
    }

    #[test]
    fn an_unset_env_var_never_becomes_an_empty_string() {
        // The predecessor's worst failures all resolved quietly to nothing. A path that
        // silently loses a component would write backups somewhere unintended.
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert!(interpolate("${env:MISSING}/logs", &c).is_err());
    }

    #[test]
    fn the_dollar_escape_produces_a_literal_reference() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(interpolate("$${host}", &c).unwrap(), "${host}");
        assert_eq!(interpolate("a$${b}c", &c).unwrap(), "a${b}c");
    }

    #[test]
    fn a_lone_dollar_is_literal() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(interpolate("100$ and $x", &c).unwrap(), "100$ and $x");
    }

    #[test]
    fn unknown_variables_are_rejected_with_the_valid_list() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        let err = interpolate("${hostname}", &c).unwrap_err();
        assert_eq!(err, InterpError::UnknownVariable("hostname".to_string()));
        let rendered = err.to_string();
        assert!(rendered.contains("${host}"), "should list valid names");
        assert!(rendered.contains("${env:NAME}"));
    }

    #[test]
    fn no_template_language_leaks_in() {
        // Anything resembling a conditional, a pipeline or a function call must be an
        // error, not a silently-passed-through string.
        let env = env_map(&[]);
        let c = ctx(&env, None);
        for hostile in [
            "${if eq .Hostname \"host-c\"}",
            "${host | upper}",
            "${randInt}",
            "${.Env.HOME}",
        ] {
            assert!(
                interpolate(hostile, &c).is_err(),
                "`{hostile}` must not be accepted"
            );
        }
    }

    #[test]
    fn unterminated_references_are_rejected() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(
            interpolate("${host", &c).unwrap_err(),
            InterpError::Unterminated
        );
    }

    #[test]
    fn empty_names_are_rejected() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(
            interpolate("${}", &c).unwrap_err(),
            InterpError::EmptyVariable
        );
        assert_eq!(
            interpolate("${env:}", &c).unwrap_err(),
            InterpError::EmptyEnvName
        );
        assert_eq!(
            interpolate("${date:}", &c).unwrap_err(),
            InterpError::EmptyDateFormat
        );
    }

    #[test]
    fn date_stays_literal_without_a_clock_and_resolves_with_one() {
        let env = env_map(&[]);

        // No clock: re-emitted verbatim, so a generated unit file keeps the reference
        // instead of freezing one day's date into it.
        let deferred = ctx(&env, None);
        assert_eq!(
            interpolate("log-${date:%Y-%m-%d}.txt", &deferred).unwrap(),
            "log-${date:%Y-%m-%d}.txt"
        );

        // With a clock: resolved.
        let now = jiff::Timestamp::UNIX_EPOCH.to_zoned(jiff::tz::TimeZone::UTC);
        let resolved = ctx(&env, Some(&now));
        assert_eq!(
            interpolate("log-${date:%Y-%m-%d}.txt", &resolved).unwrap(),
            "log-1970-01-01.txt"
        );
    }

    #[test]
    fn a_malformed_date_format_fails_at_load_time_even_when_deferred() {
        // The whole point: catching this at 03:00 during the backup would be too late.
        // Deferred resolution makes it easy to miss, so validation happens either way.
        let env = env_map(&[]);
        let c = ctx(&env, None);
        for bad in ["%J", "%", "%-"] {
            assert!(
                matches!(
                    interpolate(&format!("${{date:{bad}}}"), &c),
                    Err(InterpError::BadDateFormat { .. })
                ),
                "`{bad}` should be rejected"
            );
        }
    }

    #[test]
    fn unusual_but_valid_formats_are_accepted() {
        // Guards against over-eager validation: `%Q` is a real jiff specifier (the IANA
        // time zone identifier), and rejecting it would be a false positive.
        let now = jiff::Timestamp::UNIX_EPOCH.to_zoned(jiff::tz::TimeZone::UTC);
        let env = env_map(&[]);
        let c = ctx(&env, Some(&now));
        assert_eq!(interpolate("${date:%Q}", &c).unwrap(), "UTC");
    }

    #[test]
    fn job_and_profile_are_errors_outside_a_job() {
        // In `defaults`, `${job}` has no value. Resolving it to an empty string would
        // silently produce a path like `/logs/-2026-07-31.log`.
        let env = env_map(&[]);
        let mut c = ctx(&env, None);
        c.job = None;
        c.profile = None;
        assert_eq!(
            interpolate("${job}", &c).unwrap_err(),
            InterpError::NotAvailableHere("job".to_string())
        );
        assert_eq!(
            interpolate("${profile}", &c).unwrap_err(),
            InterpError::NotAvailableHere("profile".to_string())
        );
        // Host-level variables remain usable there.
        assert_eq!(interpolate("${host_short}", &c).unwrap(), "host-a");
    }

    #[test]
    fn multibyte_text_survives() {
        let env = env_map(&[]);
        let c = ctx(&env, None);
        assert_eq!(
            interpolate("sauvegarde-é-${job}", &c).unwrap(),
            "sauvegarde-é-dot-files"
        );
    }
}
