#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
"""Guard the AUR packaging: the template in-repo, and the rendered pair at publish time.

TWO MODES, FOR TWO DIFFERENT MOMENTS
------------------------------------
  (default)    packaging/aur/PKGBUILD is a TEMPLATE and must stay one -- no recorded
               version, no recorded checksum, sentinels intact. Wired into `just check`.
  --dir DIR    DIR/PKGBUILD and DIR/.SRCINFO agree field by field. `just aur-publish`
               renders both into a temp directory and runs this against them, so the check
               happens on the bytes actually being pushed to the AUR.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

TEMPLATE_VERSION = 1

SCALARS = ("pkgname", "pkgver", "pkgrel", "pkgdesc", "url")
ARRAYS = ("arch", "license", "depends", "makedepends", "options", "source", "sha256sums")


class ParseError(Exception):
    """The PKGBUILD used a construct this parser does not understand."""


def _strip_quotes(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        return value[1:-1]
    return value


def _split_array(body: str) -> list[str]:
    """Split a bash array body into elements, respecting quotes."""
    out: list[str] = []
    token = ""
    quote = None
    for ch in body:
        if quote:
            if ch == quote:
                quote = None
            else:
                token += ch
        elif ch in "'\"":
            quote = ch
        elif ch.isspace():
            if token:
                out.append(token)
                token = ""
        else:
            token += ch
    if quote:
        raise ParseError(f"unterminated {quote} in array: {body!r}")
    if token:
        out.append(token)
    return out


def parse_pkgbuild(text: str) -> dict[str, list[str]]:
    """Parse the assignments at the top level of a PKGBUILD."""
    fields: dict[str, list[str]] = {}
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        raw = lines[i]
        line = raw.strip()
        i += 1
        if not line or line.startswith("#"):
            continue
        if re.match(r"^[A-Za-z_][A-Za-z0-9_]*\s*\(\)\s*\{", line):
            break
        m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", line)
        if not m:
            continue
        name, rest = m.group(1), m.group(2)
        if name not in SCALARS and name not in ARRAYS:
            continue
        if rest.startswith("("):
            body = rest
            while body.count("(") > body.count(")"):
                if i >= len(lines):
                    raise ParseError(f"unterminated array for {name}")
                body += "\n" + lines[i]
                i += 1
            inner = body[body.index("(") + 1 : body.rindex(")")]
            fields[name] = _split_array(inner)
        else:
            fields[name] = [_strip_quotes(rest)]
    missing = [f for f in ("pkgname", "pkgver", "pkgrel") if f not in fields]
    if missing:
        raise ParseError(f"PKGBUILD is missing required field(s): {', '.join(missing)}")
    return fields


def expand(value: str, fields: dict[str, list[str]]) -> str:
    """Expand `$var` / `${var}` against already-parsed scalar fields."""
    scalars = {k: v[0] for k, v in fields.items() if k in SCALARS}

    def repl(m: re.Match[str]) -> str:
        name = m.group(1) or m.group(2)
        if name not in scalars:
            raise ParseError(f"cannot expand ${name} — not a scalar field in this PKGBUILD")
        return scalars[name]

    return re.sub(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)", repl, value)


def pkgbuild_fields(text: str) -> set[tuple[str, str]]:
    """Return the PKGBUILD's metadata as comparable (key, value) pairs."""
    parsed = parse_pkgbuild(text)
    out: set[tuple[str, str]] = set()
    for key, values in parsed.items():
        for v in values:
            out.add((key, expand(v, parsed)))
    return out


def srcinfo_fields(text: str) -> set[tuple[str, str]]:
    """Return .SRCINFO's declarations as comparable (key, value) pairs."""
    out: set[tuple[str, str]] = set()
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or line.startswith("pkgbase"):
            continue
        if "=" not in line:
            continue
        key, value = (s.strip() for s in line.split("=", 1))
        out.add((key, value))
    return out


def compare(pkgbuild_text: str, srcinfo_text: str) -> list[str]:
    """Return a list of human-readable disagreements; empty means they match."""
    pkg = pkgbuild_fields(pkgbuild_text)
    src = srcinfo_fields(srcinfo_text)
    problems = []
    for key, value in sorted(pkg - src):
        problems.append(f"PKGBUILD has {key} = {value!r}, .SRCINFO does not")
    for key, value in sorted(src - pkg):
        problems.append(f".SRCINFO has {key} = {value!r}, PKGBUILD does not")
    return problems


SHA256_RE = re.compile(r"\b[0-9a-fA-F]{64}\b")
VERSIONISH_RE = re.compile(r"\b[0-9]+\.[0-9]+\.[0-9]+\b")


def check_template(text: str) -> list[str]:
    """Return problems with the PKGBUILD *template*; empty means it is still a template."""
    problems: list[str] = []
    try:
        fields = parse_pkgbuild(text)
    except ParseError as exc:
        return [str(exc)]

    pkgver = fields.get("pkgver", [""])[0]
    if pkgver != "@VERSION@":
        problems.append(
            f"pkgver is {pkgver!r}, not '@VERSION@' — this file is a template and must "
            "record no released version; scripts/render_packaging.py supplies it at "
            "publish time"
        )
    sums = fields.get("sha256sums", [])
    if sums != ["@SHA256@"]:
        problems.append(
            f"sha256sums is {sums!r}, not ['@SHA256@'] — a checksum cannot be computed "
            "before its tag exists, which is why recording one here forced a post-tag "
            "commit on every release"
        )

    for key, values in sorted(fields.items()):
        for value in values:
            if key != "sha256sums" and SHA256_RE.search(value):
                problems.append(f"{key} contains a sha256 digest: {value!r}")
            if VERSIONISH_RE.search(value):
                problems.append(f"{key} contains a version number: {value!r}")

    if 'install -Dm0644 "docs/$pkgname.1"' not in text and 'install -Dm644 "docs/rusticprofile.1"' not in text:
        problems.append(
            "package() no longer installs the committed docs/rusticprofile.1"
        )
    if re.search(r"^\s*[^#\n]*mandown", text, re.M):
        problems.append("a live (non-comment) line references mandown")
    if 'install -Dm0644 LICENSE' not in text and 'install -Dm644 "LICENSE"' not in text:
        problems.append("package() no longer installs LICENSE")
    return problems


# --------------------------------------------------------------------------------------
# Self-test
# --------------------------------------------------------------------------------------

_GOOD_PKGBUILD = """\
# Maintainer: someone <nobody@example.com>
pkgname=rusticprofile
pkgver=0.2.56
pkgrel=1
pkgdesc="A local, per-machine scheduler and orchestrator for rustic backups"
arch=('x86_64' 'aarch64')
url="https://github.com/l1a/rusticprofile"
license=('GPL-3.0-or-later')
depends=('gcc-libs' 'rustic')
makedepends=('cargo')
source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('b06db1c8bd60bbaa6837c5159b1a4f37f150ba5983a6945bad972571a9450554')

build() {
  cd "$pkgname-$pkgver"
  DATE=$(date +"%B %Y")
  cargo build --release --frozen
}
"""

_GOOD_SRCINFO = """\
pkgbase = rusticprofile
\tpkgdesc = A local, per-machine scheduler and orchestrator for rustic backups
\tpkgver = 0.2.56
\tpkgrel = 1
\turl = https://github.com/l1a/rusticprofile
\tarch = x86_64
\tarch = aarch64
\tlicense = GPL-3.0-or-later
\tmakedepends = cargo
\tdepends = gcc-libs
\tdepends = rustic
\tsource = rusticprofile-0.2.56.tar.gz::https://github.com/l1a/rusticprofile/archive/refs/tags/v0.2.56.tar.gz
\tsha256sums = b06db1c8bd60bbaa6837c5159b1a4f37f150ba5983a6945bad972571a9450554

pkgname = rusticprofile
"""


def _self_test() -> int:
    failures = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        if not cond:
            failures.append(f"{name}: {detail}")

    check("matching pair", compare(_GOOD_PKGBUILD, _GOOD_SRCINFO) == [], f"got {compare(_GOOD_PKGBUILD, _GOOD_SRCINFO)}")

    pkg = pkgbuild_fields(_GOOD_PKGBUILD)
    check(
        "source expanded",
        ("source", "rusticprofile-0.2.56.tar.gz::https://github.com/l1a/rusticprofile/archive/refs/tags/v0.2.56.tar.gz") in pkg,
        f"expanded source missing from {sorted(v for k, v in pkg if k == 'source')}",
    )

    stale = _GOOD_SRCINFO.replace("pkgver = 0.2.56", "pkgver = 0.2.50")
    check("stale pkgver detected", len(compare(_GOOD_PKGBUILD, stale)) >= 2, f"got {compare(_GOOD_PKGBUILD, stale)}")

    badsum = _GOOD_SRCINFO.replace(
        "b06db1c8bd60bbaa6837c5159b1a4f37f150ba5983a6945bad972571a9450554",
        "bf51f58b97adf6dda271eb197bea2cecac7e0fad0475f355962bb0a485f1a8bd",
    )
    check("stale sha256 detected", len(compare(_GOOD_PKGBUILD, badsum)) >= 2, f"got {compare(_GOOD_PKGBUILD, badsum)}")

    extra = _GOOD_SRCINFO.replace("\tmakedepends = cargo", "\tmakedepends = cargo\n\tmakedepends = mandown")
    check("extra makedepend detected", any("mandown" in p for p in compare(_GOOD_PKGBUILD, extra)), f"got {compare(_GOOD_PKGBUILD, extra)}")

    check("function bodies ignored", not any(k == "DATE" for k, _ in pkgbuild_fields(_GOOD_PKGBUILD)), "DATE from build() was parsed")
    check("pkgdesc intact", ("pkgdesc", "A local, per-machine scheduler and orchestrator for rustic backups") in pkg, "pkgdesc mangled")

    try:
        expand("$nope/x", parse_pkgbuild(_GOOD_PKGBUILD))
        check("unknown var raises", False, "expand() silently accepted $nope")
    except ParseError:
        pass

    try:
        parse_pkgbuild("pkgname=rusticprofile\n")
        check("missing pkgver raises", False, "parse_pkgbuild accepted a PKGBUILD with no pkgver")
    except ParseError:
        pass

    live = Path(__file__).resolve().parent.parent / "packaging" / "aur" / "PKGBUILD"
    if live.is_file():
        live_problems = check_template(live.read_text(encoding="utf-8"))
        check("live template clean", live_problems == [], f"got {live_problems}")

    tmpl = _GOOD_PKGBUILD.replace("pkgver=0.2.56", "pkgver=@VERSION@").replace(
        "sha256sums=('b06db1c8bd60bbaa6837c5159b1a4f37f150ba5983a6945bad972571a9450554')",
        "sha256sums=('@SHA256@')",
    ) + (
        'package() {\n'
        '  install -Dm0644 "docs/$pkgname.1" "$pkgdir/x"\n'
        '  install -Dm0644 LICENSE "$pkgdir/l"\n'
        "}\n"
    )
    check("template fixture clean", check_template(tmpl) == [], f"got {check_template(tmpl)}")

    pinned = tmpl.replace("pkgver=@VERSION@", "pkgver=0.2.56")
    check("pinned pkgver rejected", any("pkgver" in p for p in check_template(pinned)), f"got {check_template(pinned)}")
    pinned_sum = tmpl.replace(
        "sha256sums=('@SHA256@')",
        "sha256sums=('77ccf85843d24ac3216ab31d2584ff4a95869266c59ddb8bc83819425cfc2033')",
    )
    check("pinned sha256 rejected", any("sha256sums" in p for p in check_template(pinned_sum)), f"got {check_template(pinned_sum)}")
    smuggled = tmpl.replace(
        'url="https://github.com/l1a/rusticprofile"',
        'url="https://github.com/l1a/rusticprofile#77ccf85843d24ac3216ab31d2584ff4a95869266c59ddb8bc83819425cfc2033"',
    )
    check("smuggled digest rejected", any("digest" in p for p in check_template(smuggled)), f"got {check_template(smuggled)}")
    check(
        "regenerated man page rejected",
        any("docs/rusticprofile.1" in p for p in check_template(tmpl.replace('install -Dm0644 "docs/$pkgname.1" "$pkgdir/x"', "true"))),
        "a template that stopped installing the committed man page passed",
    )
    check("live mandown rejected", any("mandown" in p for p in check_template(tmpl + "  mandown docs/rusticprofile.1.md\n")), "non-comment mandown passed")
    check("commented mandown accepted", not any("mandown" in p for p in check_template(tmpl + "# mandown is not used here\n")), "commented mandown rejected")
    dropped = tmpl.replace("install -Dm0644 LICENSE ", "true ")
    check("missing LICENSE rejected", any("LICENSE" in p for p in check_template(dropped)), f"got {check_template(dropped)}")

    if failures:
        for f in failures:
            print(f"  FAIL {f}", file=sys.stderr)
        print(f"aur_check.py self-test FAILED ({len(failures)})", file=sys.stderr)
        return 1
    print(f"aur_check.py self-test passed (template v{TEMPLATE_VERSION})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true", help="run built-in tests and exit")
    ap.add_argument("--dir", default=None, help="directory holding a RENDERED PKGBUILD and .SRCINFO to compare")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    if args.dir is None:
        template = Path(__file__).resolve().parent.parent / "packaging" / "aur" / "PKGBUILD"
        if not template.is_file():
            print(f"error: {template} not found", file=sys.stderr)
            return 1
        problems = check_template(template.read_text(encoding="utf-8"))
        if problems:
            print(f"error: {template} is no longer a valid template:", file=sys.stderr)
            for p in problems:
                print(f"  {p}", file=sys.stderr)
            return 1
        print("packaging/aur/PKGBUILD is a template (records no version, no checksum)")
        return 0

    base = Path(args.dir)
    pkgbuild, srcinfo = base / "PKGBUILD", base / ".SRCINFO"
    for f in (pkgbuild, srcinfo):
        if not f.is_file():
            print(f"error: {f} not found", file=sys.stderr)
            return 1

    try:
        problems = compare(pkgbuild.read_text(encoding="utf-8"), srcinfo.read_text(encoding="utf-8"))
    except ParseError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

    if problems:
        print(f"error: {pkgbuild} and {srcinfo} disagree:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        print("\nRegenerate the pair with: just aur-srcinfo <dir>", file=sys.stderr)
        return 1

    ver = dict((k, v) for k, v in pkgbuild_fields(pkgbuild.read_text(encoding="utf-8")) if k == "pkgver")
    print(f"{base}: PKGBUILD and .SRCINFO agree (pkgver {ver.get('pkgver', '?')})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
