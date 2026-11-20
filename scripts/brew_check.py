#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
"""Guard the Homebrew formula template (packaging/homebrew/rusticprofile.rb).

Run by `just brew-check` (and therefore `just check` and `just pr`) and by the `brew` job in
`.github/workflows/packaging.yml`. Needs no network and no Homebrew installation.

Like the AUR PKGBUILD, the COPR spec, and the Scoop manifest, the formula is a template:
it carries `@VERSION@` and `@SHA256@` sentinels that `just brew-publish` fills in from the tag
tarball when pushing to the tap (`l1a/homebrew-rusticprofile`). Nothing is committed back here
after a release, so the formula in this repo can never drift behind a tag.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from render_packaging import SENTINEL_RE, RenderError, render

TEMPLATE_VERSION = 1

REPO = Path(__file__).resolve().parent.parent
FORMULA_PATH = REPO / "packaging" / "homebrew" / "rusticprofile.rb"
FAKE_SHA = "e" * 63 + "f"
# Homebrew `brew audit --strict` runs rubocop Layout/LineLength with Max=118.
BREW_LINE_MAX = 118


def check_formula_text(text: str) -> list[str]:
    """Return every problem found in `text`; empty list means the template is sound."""
    problems: list[str] = []

    if not re.search(
        r'^\s*url\s+"https://github\.com/l1a/rusticprofile/archive/refs/tags/v@VERSION@\.tar\.gz"\s*$',
        text,
        re.M,
    ):
        problems.append(
            "url is not the v@VERSION@ tag tarball template (the formula is a template; do not "
            "commit a concrete released version)"
        )

    if not re.search(r'^\s*sha256\s+"@SHA256@"\s*$', text, re.M):
        problems.append("sha256 is not \"@SHA256@\" (checksums belong only in the tap, at publish time)")

    if re.search(r'^\s*version\s+"', text, re.M):
        problems.append(
            "an explicit `version` line is present; Homebrew derives the version from the tag URL, "
            "and a second copy is a second place to drift (`brew audit` rejects it too)"
        )

    if re.search(r'sha256\s+"[0-9a-fA-F]{64}"', text):
        problems.append("a concrete 64-hex sha256 is committed; the template must carry @SHA256@")

    if 'depends_on "rust" => :build' not in text:
        problems.append('missing `depends_on "rust" => :build`')

    if "*std_cargo_args" not in text:
        problems.append("missing `*std_cargo_args` (passes --locked --root --path to cargo install)")

    if 'man1.install "docs/rusticprofile.1"' not in text:
        problems.append('missing `man1.install "docs/rusticprofile.1"` (the man page is committed in-repo)')

    if "generate_completions_from_executable" not in text or '"--completions="' not in text:
        problems.append(
            'missing `generate_completions_from_executable(..., shell_parameter_format: "--completions=")`'
        )

    if not re.search(r"^\s*test\s+do\b", text, re.M):
        problems.append("missing `test do` block (`brew audit --strict` requires one)")

    return problems


def check_rendered_line_lengths(text: str) -> list[str]:
    """Fail lines that `brew audit --strict` would reject (Layout/LineLength Max=118)."""
    problems: list[str] = []
    for i, line in enumerate(text.splitlines(), 1):
        if len(line) > BREW_LINE_MAX:
            problems.append(
                f"rendered line {i} is {len(line)} characters; brew audit --strict allows "
                f"at most {BREW_LINE_MAX} (do not put @TOKEN@ forms in comments — render "
                "expands them)"
            )
    return problems


def check_live_formula() -> list[str]:
    text = FORMULA_PATH.read_text(encoding="utf-8")
    problems = check_formula_text(text)
    if problems:
        return problems

    try:
        rendered = render("brew", "1.2.3", FAKE_SHA)
    except RenderError as exc:
        return [f"render('brew') failed on live formula: {exc}"]

    if SENTINEL_RE.search(rendered):
        problems.append("a @SENTINEL@ survived rendering")
    if "refs/tags/v1.2.3.tar.gz" not in rendered:
        problems.append("rendered formula did not carry v1.2.3 in url")
    if f'sha256 "{FAKE_SHA}"' not in rendered:
        problems.append("rendered formula did not carry the supplied sha256")
    problems.extend(check_rendered_line_lengths(rendered))
    return problems


def _self_test() -> int:
    failures: list[str] = []

    def expect_ok(label: str, probs: list[str]) -> None:
        if probs:
            failures.append(f"{label}: expected clean, got {probs}")

    def expect_bad(label: str, probs: list[str], needle: str) -> None:
        if not any(needle in p for p in probs):
            failures.append(f"{label}: expected a problem containing {needle!r}, got {probs}")

    good = FORMULA_PATH.read_text(encoding="utf-8")
    expect_ok("live formula", check_live_formula())

    expect_bad(
        "hardcoded version in url",
        check_formula_text(good.replace("v@VERSION@.tar.gz", "v0.2.63.tar.gz")),
        "v@VERSION@",
    )
    expect_bad(
        "hardcoded sha256",
        check_formula_text(good.replace('"@SHA256@"', f'"{FAKE_SHA}"')),
        "@SHA256@",
    )
    expect_bad(
        "redundant version line",
        check_formula_text(good.replace('license "GPL-3.0-or-later"', 'version "1.2.3"\n  license "GPL-3.0-or-later"')),
        "explicit `version`",
    )
    expect_bad(
        "dropped std_cargo_args",
        check_formula_text(good.replace("*std_cargo_args", '"--path", "."')),
        "std_cargo_args",
    )
    expect_bad(
        "dropped man page install",
        check_formula_text(good.replace('man1.install "docs/rusticprofile.1"', "")),
        "docs/rusticprofile.1",
    )
    expect_bad(
        "dropped completions",
        check_formula_text(good.replace("generate_completions_from_executable", "# no completions")),
        "generate_completions_from_executable",
    )
    expect_bad(
        "dropped test block",
        check_formula_text(re.sub(r"\s*test do.*?end\s*(?=end\s*\Z)", "\n", good, flags=re.S)),
        "test do",
    )
    expect_bad(
        "rendered line too long",
        check_rendered_line_lengths("x" * (BREW_LINE_MAX + 1) + "\n"),
        str(BREW_LINE_MAX),
    )
    expect_ok("rendered line at max", check_rendered_line_lengths("x" * BREW_LINE_MAX + "\n"))

    if failures:
        for f in failures:
            print(f"brew_check self-test FAIL: {f}", file=sys.stderr)
        return 1
    print(f"brew_check.py self-test passed (template v{TEMPLATE_VERSION})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--self-test", action="store_true", help="run built-in self-tests and exit")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    problems = check_live_formula()
    if problems:
        for p in problems:
            print(f"packaging/homebrew/rusticprofile.rb: {p}", file=sys.stderr)
        return 1
    print("[brew-check] rusticprofile.rb is a template (records no version, no checksum)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
