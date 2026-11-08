#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
"""Render packaging/{aur,copr} for a released version.

WHY THIS EXISTS
---------------
The AUR PKGBUILD and the COPR spec both need the version of the release they package, and
the PKGBUILD needs its tarball's sha256. A checksum cannot be computed before the tag
exists, so previously the released version was recorded in-repo (`packaging/aur/PKGBUILD`,
`packaging/aur/.SRCINFO`, and `packaging/copr/rusticprofile.spec`), requiring a post-release
bump commit.

This script inverts that: **the templates record nothing, and the version is supplied once,
at publish or SRPM build time.** A stale checksum cannot be committed because no checksum is
committed, and the offline guards (`aur-check`, `copr-check`) assert that the templates
still carry their `@VERSION@` / `@SHA256@` sentinels.
"""

from __future__ import annotations

import argparse
import re
import sys
import time
from dataclasses import dataclass
from pathlib import Path

TEMPLATE_VERSION = 1

REPO_ROOT = Path(__file__).resolve().parent.parent

SENTINEL_RE = re.compile(r"@[A-Z0-9_]+@")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
VERSION_RE = re.compile(r"^[0-9]+(\.[0-9]+)+$")

MAINTAINER = "Ken Tobias <634380+l1a@users.noreply.github.com>"


class RenderError(Exception):
    """The render could not be completed safely."""


@dataclass(frozen=True)
class Target:
    path: str
    needs_sha: bool
    what: str


TARGETS = {
    "aur": Target("packaging/aur/PKGBUILD", True, "AUR PKGBUILD"),
    "copr": Target("packaging/copr/rusticprofile.spec", False, "COPR spec"),
}


def cargo_version(text: str) -> str:
    """Return the `[package]` version from a Cargo.toml."""
    section = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped[1:-1]
            continue
        if section != "package":
            continue
        m = re.match(r'^version\s*=\s*"([^"]+)"', stripped)
        if m:
            return m.group(1)
    raise RenderError("no [package] version found in Cargo.toml")


def repo_version(root: Path = REPO_ROOT) -> str:
    return cargo_version((root / "Cargo.toml").read_text(encoding="utf-8"))


def validate_version(version: str) -> str:
    if not VERSION_RE.match(version):
        raise RenderError(f"not a version number: {version!r}")
    return version


def validate_sha256(sha: str) -> str:
    if not SHA256_RE.match(sha):
        raise RenderError(
            f"not a 64-character lowercase hex sha256: {sha!r} "
            "(compute it from the real tarball; SKIP and placeholders are refused)"
        )
    if len(set(sha)) == 1:
        raise RenderError(f"sha256 is a single repeated character -- placeholder? {sha!r}")
    return sha


def substitute(text: str, values: dict[str, str], *, what: str) -> str:
    """Replace each @KEY@ with its value, requiring at least one hit for every key."""
    for key, value in values.items():
        token = f"@{key}@"
        text, n = re.subn(re.escape(token), value.replace("\\", "\\\\"), text)
        if n == 0:
            raise RenderError(
                f"{what}: {token} matched nothing -- the template no longer carries it."
            )
    leftover = SENTINEL_RE.findall(text)
    if leftover:
        raise RenderError(
            f"{what}: sentinel(s) survived rendering: {sorted(set(leftover))}. "
            "Refusing to write a file that would be published with a placeholder in it."
        )
    return text


def prepend_changelog(spec: str, version: str, stamp: str) -> str:
    """Insert a %changelog entry for `version` directly under the `%changelog` header."""
    if re.search(rf"^\*.*-\s*{re.escape(version)}-\S*\s*$", spec, re.M):
        return spec
    entry = (
        f"* {stamp} {MAINTAINER} - {version}-1\n"
        f"- Update to {version}\n"
        "\n"
    )
    out, n = re.subn(r"^%changelog\n", f"%changelog\n{entry}", spec, count=1, flags=re.M)
    if n != 1:
        raise RenderError("COPR spec has no %changelog section to prepend to")
    return out


def render(
    target: str,
    version: str,
    sha256: str | None = None,
    *,
    root: Path = REPO_ROOT,
    stamp: str | None = None,
) -> str:
    """Return the rendered text for `target` at `version`."""
    if target not in TARGETS:
        raise RenderError(f"unknown target {target!r}; expected one of {sorted(TARGETS)}")
    spec = TARGETS[target]
    validate_version(version)

    template = (root / spec.path).read_text(encoding="utf-8")
    values = {"VERSION": version}
    if spec.needs_sha:
        if sha256 is None:
            raise RenderError(f"{spec.what} pins a checksum: --sha256 is required")
        values["SHA256"] = validate_sha256(sha256)
    elif sha256 is not None:
        raise RenderError(f"{spec.what} pins no checksum: --sha256 is not accepted")

    text = template
    if target == "copr":
        text = prepend_changelog(text, version, stamp or time.strftime("%a %b %d %Y", time.gmtime()))
    return substitute(text, values, what=spec.what)


# --------------------------------------------------------------------------------------
# Self-test
# --------------------------------------------------------------------------------------

_SHA = "77ccf85843d24ac3216ab31d2584ff4a95869266c59ddb8bc83819425cfc2033"


def _self_test() -> int:
    failures: list[str] = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        if not cond:
            failures.append(f"{name}: {detail}")

    def refuses(name: str, fn) -> None:
        try:
            fn()
        except RenderError:
            return
        failures.append(f"{name}: expected a RenderError, got none")

    aur = render("aur", "1.2.3", _SHA)
    check("aur pkgver", "\npkgver=1.2.3\n" in aur, "rendered PKGBUILD has no pkgver=1.2.3")
    check("aur sha", f"sha256sums=('{_SHA}')" in aur, "rendered PKGBUILD has no sha256sums")
    check("aur no sentinel", not SENTINEL_RE.search(aur), "sentinel survived")

    copr = render("copr", "1.2.3", stamp="Mon Jan 05 2026")
    check("copr version", "\nVersion:        1.2.3\n" in copr, "rendered spec has no Version: 1.2.3")
    check("copr no sentinel", not SENTINEL_RE.search(copr), "sentinel survived")
    newest = re.search(r"^%changelog\n\* [^\n]*- ([0-9.]+)-(\S+)$", copr, re.M)
    check(
        "copr changelog coherent",
        newest is not None and newest.group(1) == "1.2.3",
        f"newest changelog entry is {newest.group(1) if newest else None!r}, not 1.2.3",
    )
    check("copr changelog history kept", copr.count("* ") > 1, "historical changelog entries dropped")

    already = re.search(r"^%changelog\n\* [^\n]*- ([0-9.]+)-", copr, re.M)
    if already:
        v = already.group(1)
        again = render("copr", v, stamp="Tue Feb 03 2026")
        n = len(re.findall(rf"^\*.*- {re.escape(v)}-", again, re.M))
        check("copr render is idempotent", n == 1, f"rendering {v} again produced {n} changelog entries")

    refuses("bad version", lambda: render("aur", "not-a-version", _SHA))
    refuses("bad sha", lambda: render("aur", "1.2.3", "SKIP"))
    refuses("short sha", lambda: render("aur", "1.2.3", _SHA[:63]))
    refuses("uppercase sha", lambda: render("aur", "1.2.3", _SHA.upper()))
    refuses("placeholder sha", lambda: render("aur", "1.2.3", "0" * 64))
    refuses("sha for copr", lambda: render("copr", "1.2.3", _SHA))
    refuses("missing sha", lambda: render("aur", "1.2.3"))
    refuses("unknown target", lambda: render("nope", "1.2.3"))
    refuses("sentinel gone", lambda: substitute("pkgver=0.1.2\n", {"VERSION": "1.2.3"}, what="t"))
    refuses("unknown sentinel", lambda: substitute("a=@VERSION@ b=@WHAT@\n", {"VERSION": "1.2.3"}, what="t"))

    manifest = (
        '[package]\nname = "rusticprofile"\nversion = "9.9.9"\n\n'
        '[dependencies]\nclap = "4.6"\n'
    )
    check("cargo version", cargo_version(manifest) == "9.9.9", f"got {cargo_version(manifest)!r}")
    refuses("cargo no package", lambda: cargo_version('[dependencies]\nversion = "1.0.0"\n'))
    check("live cargo version", VERSION_RE.match(repo_version()) is not None, f"repo_version() returned {repo_version()!r}")

    if failures:
        for f in failures:
            print(f"  FAIL {f}", file=sys.stderr)
        print(f"render_packaging.py self-test FAILED ({len(failures)})", file=sys.stderr)
        return 1
    print(f"render_packaging.py self-test passed (template v{TEMPLATE_VERSION})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true", help="run built-in tests and exit")
    ap.add_argument("--print-version", action="store_true", help="print Cargo.toml's [package] version and exit")
    ap.add_argument("--target", choices=sorted(TARGETS), help="which packaging file to render")
    ap.add_argument("--version", help="released version (default: Cargo.toml's)")
    ap.add_argument("--sha256", help="sha256 of the release tarball (aur only)")
    ap.add_argument("--out", help="write here instead of stdout")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    try:
        if args.print_version:
            print(repo_version())
            return 0
        if not args.target:
            ap.error("--target is required (or use --print-version / --self-test)")
        text = render(args.target, args.version or repo_version(), args.sha256)
    except RenderError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if args.out:
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text, encoding="utf-8", newline="\n")
        print(f"rendered {TARGETS[args.target].what} -> {out}")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
