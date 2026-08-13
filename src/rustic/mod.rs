// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything about invoking rustic: building the argv, and later classifying what comes
//! back.
//!
//! rusticprofile constructs **no backup flags**. A job resolves to
//! `rustic -P <profile> <operation>` plus one `--name` per snapshot set enabled on this
//! host, and nothing else. Everything a backup needs beyond that — repository, sources,
//! excludes, retention policy, hooks, metrics, credentials — lives in rustic's own config.
//!
//! That is not minimalism for its own sake. A flag catalogue is the one piece the
//! predecessor could not make independent of restic's own documentation, and it is where
//! its most complex and highest-risk code lives.

pub mod exit;
pub mod invoke;
pub mod retention;
