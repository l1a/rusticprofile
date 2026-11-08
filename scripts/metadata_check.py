#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
"""Assert every channel's description and licence agree with packaging/metadata.toml."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
META = REPO / "packaging" / "metadata.toml"
REPO_SLUG = "l1a/rusticprofile"

FILES = {
    "cargo": "Cargo.toml",
    "pkgbuild": "packaging/aur/PKGBUILD",
    "spec": "packaging/copr/rusticprofile.spec",
}

TOPIC_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,49}$")
GITHUB_DESCRIPTION_MAX = 350
GITHUB_TOPICS_MAX = 20


class ParseError(Exception):
    """metadata.toml is malformed, or a checked file lacks a field this script needs."""


def load_meta(text: str) -> dict:
    """Parse metadata.toml and insist on every key the checks and the syncs rely on."""
    try:
        meta = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise ParseError(f"metadata.toml: {exc}") from exc
    for key in ("summary", "license"):
        if not isinstance(meta.get(key), str) or not meta[key].strip():
            raise ParseError(f"metadata.toml: `{key}` must be a non-empty string")
    github = meta.get("github")
    if (
        not isinstance(github, dict)
        or not isinstance(github.get("description"), str)
        or not isinstance(github.get("topics"), list)
    ):
        raise ParseError("metadata.toml: [github] needs `description` (string) and `topics` (list)")
    copr = meta.get("copr")
    if not isinstance(copr, dict) or not all(
        isinstance(copr.get(k), str) for k in ("description", "instructions")
    ):
        raise ParseError("metadata.toml: [copr] needs `description` and `instructions` paths")
    return meta


def _read(path: Path) -> str:
    return path.read_bytes().decode("utf-8")


def read_texts(meta: dict) -> dict[str, str]:
    texts = {key: _read(REPO / rel) for key, rel in FILES.items()}
    texts["copr_description"] = _read(REPO / meta["copr"]["description"])
    texts["copr_instructions"] = _read(REPO / meta["copr"]["instructions"])
    return texts


def _field(text: str, pattern: str, what: str) -> str:
    m = re.search(pattern, text, re.M)
    if not m:
        raise ParseError(f"no {what}")
    return m.group(1).strip()


def check_copr_markdown(text: str, label: str) -> list[str]:
    problems: list[str] = []
    if not text.strip():
        problems.append(f"{label} is empty")
    if "\r" in text:
        problems.append(f"{label} contains CR bytes; COPR stores exactly what is sent")
    fenced = False
    for number, line in enumerate(text.split("\n"), 1):
        if line.lstrip().startswith("```"):
            fenced = not fenced
            continue
        if not fenced and line.strip() and (line.startswith("    ") or line.startswith("\t")):
            problems.append(
                f"{label} line {number} is indented 4+ spaces outside a code fence, which "
                f"COPR's Markdown renders as a code block: {line.strip()[:60]!r}"
            )
    return problems


def check_channels(meta: dict, texts: dict[str, str]) -> list[str]:
    """Return a list of problems; empty means every channel agrees with metadata.toml."""
    problems: list[str] = []
    summary, licence = meta["summary"], meta["license"]

    def expect(where: str, got: str | None, want: str) -> None:
        if got != want:
            problems.append(f"{where} is {got!r}, not {want!r}")

    try:
        pkg = tomllib.loads(texts["cargo"])["package"]
    except (tomllib.TOMLDecodeError, KeyError) as exc:
        return [f"could not read Cargo.toml [package] table: {exc}"]
    description = pkg.get("description", "")
    if not description.startswith(summary):
        problems.append(
            f"Cargo.toml description {description!r} does not begin with the summary "
            f"{summary!r} (crates.io shows it as the one-liner)"
        )
    expect("Cargo.toml license", pkg.get("license"), licence)

    fields = [
        ("PKGBUILD pkgdesc", "pkgbuild", r'^pkgdesc="([^"]*)"', summary),
        ("PKGBUILD license", "pkgbuild", r"^license=\('([^']*)'\)", licence),
        ("spec Summary", "spec", r"^Summary:\s*(.+)$", summary),
        ("spec License", "spec", r"^License:\s*(.+)$", licence),
    ]
    for where, key, pattern, want in fields:
        try:
            expect(where, _field(texts[key], pattern, where), want)
        except ParseError as exc:
            problems.append(str(exc))

    github_description = meta["github"]["description"]
    topics = meta["github"]["topics"]
    if not github_description.strip():
        problems.append("[github] description is empty")
    if len(github_description) > GITHUB_DESCRIPTION_MAX:
        problems.append(
            f"[github] description is {len(github_description)} characters; GitHub allows "
            f"{GITHUB_DESCRIPTION_MAX}"
        )
    if len(topics) > GITHUB_TOPICS_MAX:
        problems.append(f"[github] has {len(topics)} topics; GitHub allows {GITHUB_TOPICS_MAX}")
    if len(set(topics)) != len(topics):
        problems.append("[github] topics contain a duplicate")
    for topic in topics:
        if not isinstance(topic, str) or not TOPIC_RE.match(topic):
            problems.append(f"[github] topic {topic!r} is not lowercase letters, digits and hyphens")

    problems.extend(check_copr_markdown(texts["copr_description"], "COPR description"))
    problems.extend(check_copr_markdown(texts["copr_instructions"], "COPR instructions"))
    return problems


# ── GitHub sync ──────────────────────────────────────────────────────────────


def _gh(*args: str) -> str:
    gh = shutil.which("gh")
    if gh is None:
        raise RuntimeError("gh is not on PATH")
    result = subprocess.run([gh, *args], capture_output=True, text=True, encoding="utf-8")
    if result.returncode != 0:
        raise RuntimeError(f"`gh {' '.join(args[:3])} ...` failed: {result.stderr.strip()}")
    return result.stdout


def github_state() -> tuple[str, list[str]]:
    data = json.loads(_gh("repo", "view", REPO_SLUG, "--json", "description,repositoryTopics"))
    topics = sorted(t["name"] for t in (data.get("repositoryTopics") or []))
    return data.get("description") or "", topics


def sync_github(meta: dict, dry_run: bool) -> int:
    want_description = meta["github"]["description"]
    want_topics = sorted(meta["github"]["topics"])
    have_description, have_topics = github_state()

    if have_description == want_description and have_topics == want_topics:
        print(f"[github-metadata] {REPO_SLUG} already matches packaging/metadata.toml")
        return 0
    if have_description != want_description:
        print(
            f"[github-metadata] description: {have_description!r}\n"
            f"                          -> {want_description!r}"
        )
    if have_topics != want_topics:
        print(f"[github-metadata] topics: {have_topics} -> {want_topics}")
    if dry_run:
        print("[github-metadata] --dry-run: nothing changed")
        return 0

    if have_description != want_description:
        _gh("repo", "edit", REPO_SLUG, "--description", want_description)
    if have_topics != want_topics:
        args = ["api", "-X", "PUT", f"repos/{REPO_SLUG}/topics"]
        for topic in want_topics:
            args += ["-f", f"names[]={topic}"]
        _gh(*args)

    got_description, got_topics = github_state()
    if got_description != want_description or got_topics != want_topics:
        print(
            f"[github-metadata] read-back disagrees: {got_description!r} {got_topics}",
            file=sys.stderr,
        )
        return 1
    print(f"[github-metadata] {REPO_SLUG} updated and read back")
    return 0


# ── self-test ────────────────────────────────────────────────────────────────

_META = """\
summary = "A local, per-machine scheduler and orchestrator for rustic backups"
license = "GPL-3.0-or-later"
[github]
description = "A local, per-machine scheduler and orchestrator for rustic backups"
topics = ["backup", "rust"]
[copr]
description = "d.md"
instructions = "i.md"
"""

_TEXTS = {
    "cargo": '[package]\nname = "rusticprofile"\ndescription = "A local, per-machine scheduler and orchestrator for rustic backups"\nlicense = "GPL-3.0-or-later"\n',
    "pkgbuild": "pkgname=rusticprofile\npkgdesc=\"A local, per-machine scheduler and orchestrator for rustic backups\"\nlicense=('GPL-3.0-or-later')\n",
    "spec": "Name:           rusticprofile\nSummary:        A local, per-machine scheduler and orchestrator for rustic backups\nLicense:        GPL-3.0-or-later\n",
    "copr_description": "**rusticprofile** is a backup scheduler.\n\n* an item\n\n```\n    indented code is fine inside a fence\n```\n",
    "copr_instructions": "Install:\n\n```\nsudo dnf install rusticprofile\n```\n",
}


def _self_test() -> int:
    failures: list[str] = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        if not cond:
            failures.append(f"{name}: {detail}")

    meta = load_meta(_META)

    def expect_problem(label: str, key: str, old: str, new: str, needle: str) -> None:
        texts = dict(_TEXTS)
        check(f"{label} (fixture carries {old!r})", old in texts[key], "fixture changed shape")
        texts[key] = texts[key].replace(old, new)
        probs = check_channels(meta, texts)
        check(label, any(needle in p for p in probs), f"got {probs}")

    clean = check_channels(meta, _TEXTS)
    check("fixture clean", clean == [], f"got {clean}")

    if META.is_file():
        try:
            live_meta = load_meta(_read(META))
            live = check_channels(live_meta, read_texts(live_meta))
        except (ParseError, FileNotFoundError) as exc:
            live = [str(exc)]
        check("live repository clean", live == [], f"got {live}")

    expect_problem("legacy AUR GPL3 detected", "pkgbuild", "('GPL-3.0-or-later')", "('GPL3')", "PKGBUILD license")
    expect_problem(
        "indented COPR paragraph detected",
        "copr_description",
        "* an item\n",
        "* an item\n\n    an indented paragraph\n",
        "code block",
    )
    expect_problem("pkgdesc drift detected", "pkgbuild", "A local, per-machine", "A different", "PKGBUILD pkgdesc")
    expect_problem("spec Summary drift detected", "spec", "Summary:        A local", "Summary:        A different", "spec Summary")
    expect_problem("spec License drift detected", "spec", "License:        GPL-3.0-or-later", "License:        MIT", "spec License")
    expect_problem("Cargo description drift detected", "cargo", "A local, per-machine", "Something else", "does not begin with the summary")
    expect_problem("Cargo licence drift detected", "cargo", 'license = "GPL-3.0-or-later"', 'license = "MIT"', "Cargo.toml license")
    expect_problem("CR in COPR text detected", "copr_instructions", "Install:\n", "Install:\r\n", "CR bytes")

    texts = dict(_TEXTS, pkgbuild="pkgname=rusticprofile\n")
    probs = check_channels(meta, texts)
    check("missing pkgdesc reported", any("no PKGBUILD pkgdesc" in p for p in probs), f"got {probs}")

    bad = load_meta(_META.replace('["backup", "rust"]', '["backup", "Rust Lang"]'))
    check("bad topic detected", any("topic" in p for p in check_channels(bad, _TEXTS)), "no topic problem")
    bad = load_meta(_META.replace('["backup", "rust"]', '["backup", "backup"]'))
    check("duplicate topic detected", any("duplicate" in p for p in check_channels(bad, _TEXTS)), "no duplicate problem")

    try:
        load_meta('summary = ""\n')
        check("empty summary raises", False, "load_meta accepted an empty summary")
    except ParseError:
        pass

    if failures:
        for f in failures:
            print(f"  FAIL {f}", file=sys.stderr)
        print(f"metadata_check.py self-test FAILED ({len(failures)})", file=sys.stderr)
        return 1
    print("metadata_check.py self-test passed")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true", help="run the built-in assertions")
    ap.add_argument(
        "--sync-github",
        action="store_true",
        help="after checking, set the GitHub description and topics from metadata.toml",
    )
    ap.add_argument("--dry-run", action="store_true", help="with --sync-github: show, change nothing")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    try:
        meta = load_meta(_read(META))
        problems = check_channels(meta, read_texts(meta))
    except (ParseError, FileNotFoundError) as exc:
        print(f"[metadata-check] {exc}", file=sys.stderr)
        return 1

    if problems:
        for p in problems:
            print(f"[metadata-check] {p}", file=sys.stderr)
        print(
            "[metadata-check] packaging/metadata.toml is the source; bring the channel back "
            "in line with it (or change it there, deliberately)",
            file=sys.stderr,
        )
        return 1
    print("[metadata-check] channel descriptions and licences agree with packaging/metadata.toml")

    if args.sync_github:
        try:
            return sync_github(meta, args.dry_run)
        except RuntimeError as exc:
            print(f"[github-metadata] {exc}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
