#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
"""Assert packaging/copr/rusticprofile.spec has not drifted from the rest of the repo.

WHY THIS EXISTS
---------------
The spec's `Version:` tracks the last RELEASED tag, not `Cargo.toml`, because `Source0` is a
tag tarball that has to exist before it can be referenced. That is correct and deliberate --
and it means the field is bumped by a human, at release time, in a separate commit, with
nothing checking the result.

`packaging/aur/PKGBUILD` had exactly that shape and exposure. This checker applies retch's
guard to rusticprofile before drift happens.

WHAT IT CHECKS
--------------
All five are offline, deterministic, and free:

  1. `Version:` == `packaging/aur/PKGBUILD`'s `pkgver`. Both pin the last released tag, so
     they are two independent recordings of one fact -- and a disagreement means one of them
     was bumped at release time and the other forgotten. This is the drift that actually
     happened in other repos, and it is the load-bearing check.
  2. `Version:` <= `Cargo.toml`'s version. The in-development version leads and the released
     version trails, so a spec AHEAD of Cargo.toml pins a tag that cannot exist. Deliberately
     one-sided: `Version:` legitimately trails by a whole release cycle, so equality and
     "behind" are both fine and only "ahead" is a defect.
  3. The newest `%changelog` entry's version-release matches `Version:`-`Release:`. rpm does
     not care, so this drifts silently -- and the changelog is what a user reads to find out
     what they installed.
  4. `Source0` still refers to `%{version}` rather than a hardcoded number, which would
     decouple the tarball from `Version:` while leaving both fields looking right.
  5. `cargo build` still passes `--locked`. The spec's own comment calls this load-bearing
     and says "Never drop it" -- with internet-enabled COPR builds and no vendor tarball it
     is the only thing pinning resolution to what was tested. A comment is not a guard.

WHAT IT IS NOT
--------------
It deliberately does NOT:

  - verify the Source0 tarball exists or checksum it (needs the network -- the `copr` CI job
    fetches it for real via `spectool` when it builds the SRPM), or
  - prove the package builds (that is the `copr` CI job, and then COPR itself).

It is the cheap, offline, always-runnable half, wired into `just check` so a drifted spec
cannot reach a commit. The expensive halves run where they belong.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

TEMPLATE_VERSION = 1


class ParseError(Exception):
    """The spec used a construct this parser does not understand."""


def _preamble(text: str) -> str:
    """Everything before the first `%`-section directive.

    `Version:` and friends are preamble tags. Stopping at the first section keeps a
    `Version:` mentioned inside `%description` prose or a `%changelog` body from being read
    as the package's version.
    """
    out = []
    for line in text.splitlines():
        if re.match(r"^%(prep|build|install|check|files|changelog|description|package)\b", line):
            break
        out.append(line)
    return "\n".join(out)


def spec_tag(text: str, tag: str) -> str:
    """Read one preamble tag, e.g. `Version:        0.2.35` -> `0.2.35`.

    Raises rather than returning a default: a tag this cannot find is a tag that would
    otherwise compare equal to another missing one, which is the failure mode the whole file
    exists to prevent.
    """
    m = re.search(rf"^{tag}:\s*(\S.*?)\s*$", _preamble(text), re.MULTILINE)
    if not m:
        raise ParseError(f"spec has no `{tag}:` tag in its preamble")
    return m.group(1)


def spec_version(text: str) -> str:
    return spec_tag(text, "Version")


def spec_release(text: str) -> str:
    """`Release: 1%{?dist}` -> `1`. The dist suffix is rpm's, not ours to compare."""
    return re.sub(r"%\{\?dist\}\s*$", "", spec_tag(text, "Release")).strip()


def spec_source0(text: str) -> str:
    return spec_tag(text, "Source0")


def newest_changelog_entry(text: str) -> str:
    """The `- <version>-<release>` trailer of the newest %changelog entry.

    rpm's changelog is newest-first, so the first `*` line is the current one.
    """
    m = re.search(r"^%changelog\s*$(.*)", text, re.MULTILINE | re.DOTALL)
    if not m:
        raise ParseError("spec has no %changelog section")
    for line in m.group(1).splitlines():
        if line.startswith("*"):
            trailer = re.search(r"-\s*(\S+)\s*$", line)
            if not trailer:
                raise ParseError(f"newest %changelog entry has no `- <version>-<release>`: {line!r}")
            return trailer.group(1)
    raise ParseError("%changelog section contains no entries")


def pkgbuild_pkgver(text: str) -> str:
    m = re.search(r"^pkgver=(\S+)\s*$", text, re.MULTILINE)
    if not m:
        raise ParseError("PKGBUILD has no pkgver")
    return m.group(1).strip("'\"")


def cargo_version(text: str) -> str:
    """The `[package]` version from a Cargo.toml.

    Scoped to the first `version =` after `[package]`, so a dependency's version pin cannot
    be mistaken for the crate's own.
    """
    m = re.search(r"^\[package\]\s*$(.*?)(?=^\[|\Z)", text, re.MULTILINE | re.DOTALL)
    if not m:
        raise ParseError("Cargo.toml has no [package] section")
    v = re.search(r'^version\s*=\s*"([^"]+)"', m.group(1), re.MULTILINE)
    if not v:
        raise ParseError("Cargo.toml [package] has no version")
    return v.group(1)


def _parts(version: str) -> tuple[int, ...]:
    """Numeric comparison tuple. Raises on anything not plain dotted digits.

    Refusing to guess is the point: a pre-release or otherwise unexpected string compared
    with a lenient parser would silently order wrongly, and an ordering check that can be
    wrong in the safe-looking direction is worse than one that stops.
    """
    if not re.fullmatch(r"\d+(\.\d+)*", version):
        raise ParseError(f"cannot compare non-numeric version {version!r}")
    return tuple(int(p) for p in version.split("."))


def compare(spec_text: str, pkgbuild_text: str, cargo_text: str) -> list[str]:
    """Return a list of human-readable problems; empty means the spec is consistent."""
    problems: list[str] = []

    version = spec_version(spec_text)
    release = spec_release(spec_text)
    pkgver = pkgbuild_pkgver(pkgbuild_text)
    cargo = cargo_version(cargo_text)

    # 1. The two packaging targets record the same fact and must agree.
    if version != pkgver:
        problems.append(
            f"spec Version: is {version} but packaging/aur/PKGBUILD pkgver is {pkgver} — "
            f"both track the last RELEASED tag, so one of them was not bumped"
        )

    # 2. One-sided: trailing Cargo.toml is the normal state, leading it is impossible.
    if _parts(version) > _parts(cargo):
        problems.append(
            f"spec Version: {version} is AHEAD of Cargo.toml {cargo} — Source0 pins tag "
            f"v{version}, which cannot exist yet"
        )

    # 3. The changelog is what a user reads to see what they installed.
    newest = newest_changelog_entry(spec_text)
    expected = f"{version}-{release}"
    if newest != expected:
        problems.append(
            f"newest %changelog entry is {newest} but Version-Release is {expected} — "
            f"the changelog was not updated with the bump"
        )

    # 4. A hardcoded Source0 leaves both fields looking correct while decoupling them.
    source0 = spec_source0(spec_text)
    if "%{version}" not in source0:
        problems.append(
            f"Source0 does not reference %{{version}}: {source0!r} — a hardcoded version "
            f"silently decouples the tarball from Version:"
        )

    # 5. The spec's own comment says never to drop this; make that enforceable.
    if not re.search(r"^\s*cargo build\b.*--locked", spec_text, re.MULTILINE):
        problems.append(
            "the %build `cargo build` does not pass --locked — with internet-enabled COPR "
            "builds and no vendor tarball it is the only thing pinning dependency resolution"
        )

    return problems


# --------------------------------------------------------------------------------------
# Self-test
# --------------------------------------------------------------------------------------

_GOOD_SPEC = """\
# COPR spec for rusticprofile.
Name:           rusticprofile
Version:        0.2.35
Release:        1%{?dist}
Summary:        A local, per-machine scheduler and orchestrator for rustic backups

License:        GPL-3.0-or-later
URL:            https://github.com/l1a/rusticprofile
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}-%{version}.tar.gz

%description
rusticprofile is a backup scheduler. Version: numbers in prose must not be
read as the package's own version.

%build
export RUSTFLAGS="%{build_rustflags}"
cargo build --release --locked

%files
%{_bindir}/rusticprofile

%changelog
* Wed Sep 09 2026 Ken Tobias <nobody@example.com> - 0.2.35-1
- Initial COPR packaging
"""

_GOOD_PKGBUILD = "pkgname=rusticprofile\npkgver=0.2.35\npkgrel=1\n"
_GOOD_CARGO = '[package]\nname = "rusticprofile"\nversion = "0.2.36"\n\n[dependencies]\nclap = "4.6"\n'


def _self_test() -> int:
    failures = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        if not cond:
            failures.append(f"{name}: {detail}")

    # The consistent trio is clean.
    clean = compare(_GOOD_SPEC, _GOOD_PKGBUILD, _GOOD_CARGO)
    check("consistent spec", clean == [], f"got {clean}")

    # 1. THE drift this exists to catch: the AUR bumped, the spec forgotten.
    stale = _GOOD_SPEC.replace("Version:        0.2.35", "Version:        0.2.32")
    stale = stale.replace("- 0.2.35-1", "- 0.2.32-1")
    probs = compare(stale, _GOOD_PKGBUILD, _GOOD_CARGO)
    check("stale Version detected",
          any("PKGBUILD pkgver" in p for p in probs), f"got {probs}")

    # 2. A spec ahead of Cargo.toml pins a tag that cannot exist.
    ahead = _GOOD_SPEC.replace("Version:        0.2.35", "Version:        1.0.0")
    ahead = ahead.replace("- 0.2.35-1", "- 1.0.0-1")
    probs = compare(ahead, "pkgname=rusticprofile\npkgver=1.0.0\npkgrel=1\n", _GOOD_CARGO)
    check("ahead-of-Cargo detected", any("AHEAD of Cargo.toml" in p for p in probs), f"got {probs}")

    # ...but TRAILING Cargo.toml is the normal state and must stay silent.
    probs = compare(_GOOD_SPEC, _GOOD_PKGBUILD,
                    '[package]\nname = "rusticprofile"\nversion = "0.2.99"\n')
    check("trailing Cargo is fine", probs == [], f"got {probs}")

    # 3. Version bumped, changelog forgotten — rpm accepts it silently.
    nochangelog = _GOOD_SPEC.replace("Version:        0.2.35", "Version:        0.2.36")
    probs = compare(nochangelog, "pkgname=rusticprofile\npkgver=0.2.36\npkgrel=1\n",
                    '[package]\nname = "rusticprofile"\nversion = "0.2.37"\n')
    check("stale changelog detected", any("%changelog" in p for p in probs), f"got {probs}")

    # 4. A hardcoded Source0 leaves Version: looking right while meaning nothing.
    hardcoded = _GOOD_SPEC.replace(
        "%{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}-%{version}.tar.gz",
        "%{url}/archive/refs/tags/v0.2.35.tar.gz#/rusticprofile-0.2.35.tar.gz")
    probs = compare(hardcoded, _GOOD_PKGBUILD, _GOOD_CARGO)
    check("hardcoded Source0 detected", any("Source0" in p for p in probs), f"got {probs}")

    # 5. --locked dropped from the build.
    unlocked = _GOOD_SPEC.replace("cargo build --release --locked", "cargo build --release")
    probs = compare(unlocked, _GOOD_PKGBUILD, _GOOD_CARGO)
    check("--locked removal detected", any("--locked" in p for p in probs), f"got {probs}")

    # Section bodies must not leak into the preamble.
    check("preamble scoped", spec_version(_GOOD_SPEC) == "0.2.35",
          f"got {spec_version(_GOOD_SPEC)}")

    # The dist suffix is rpm's; comparing it against a changelog entry would never match.
    check("release strips dist", spec_release(_GOOD_SPEC) == "1",
          f"got {spec_release(_GOOD_SPEC)!r}")

    # The changelog is newest-first; the SECOND entry must not be the one compared.
    check("newest changelog entry", newest_changelog_entry(_GOOD_SPEC) == "0.2.35-1",
          f"got {newest_changelog_entry(_GOOD_SPEC)!r}")

    # A dependency pin must not be mistaken for the crate's own version.
    check("cargo version scoped", cargo_version(_GOOD_CARGO) == "0.2.36",
          f"got {cargo_version(_GOOD_CARGO)}")

    # A missing tag must RAISE.
    try:
        spec_version("Name: rusticprofile\n")
        check("missing Version raises", False, "spec_version accepted a spec with no Version:")
    except ParseError:
        pass

    # An uncomparable version must stop rather than order itself wrongly.
    try:
        _parts("0.2.35-rc.1")
        check("non-numeric version raises", False, "_parts accepted a pre-release string")
    except ParseError:
        pass

    if failures:
        for f in failures:
            print(f"  FAIL {f}", file=sys.stderr)
        print(f"copr_check.py self-test FAILED ({len(failures)})", file=sys.stderr)
        return 1
    print(f"copr_check.py self-test passed (template v{TEMPLATE_VERSION})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true", help="run built-in tests and exit")
    ap.add_argument("--root", default=None, help="repository root (default: this script's parent)")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    root = Path(args.root) if args.root else Path(__file__).resolve().parent.parent
    spec = root / "packaging" / "copr" / "rusticprofile.spec"
    pkgbuild = root / "packaging" / "aur" / "PKGBUILD"
    cargo = root / "Cargo.toml"
    for f in (spec, pkgbuild, cargo):
        if not f.is_file():
            print(f"error: {f} not found", file=sys.stderr)
            return 1

    try:
        problems = compare(spec.read_text(encoding="utf-8"),
                           pkgbuild.read_text(encoding="utf-8"),
                           cargo.read_text(encoding="utf-8"))
        version = spec_version(spec.read_text(encoding="utf-8"))
    except ParseError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

    if problems:
        print(f"error: {spec} has drifted:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        print("\nAt release time, bump it with: just copr-bump <version>", file=sys.stderr)
        return 1

    print(f"packaging/copr: rusticprofile.spec is consistent (Version {version})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
