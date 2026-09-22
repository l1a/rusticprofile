#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Refuse a carriage return in WIP.md, the one text file no other guard can reach.

WIP.md is the gitignored, Syncthing-synced cross-machine handoff file (`AGENTS.md` Part 1
section 3). LF is the base model for every non-binary file in this tree -- measured
2026-09-22 by bytes: 80 tracked text files, 0 carriage returns, and WIP.md itself 0.

WHY THIS IS A SEPARATE GUARD. `.gitattributes` pins `* text=auto eol=lf` and
`scripts/text_check.py` refuses a carriage return in tracked text, but BOTH work through git,
and `git ls-files` never offers a gitignored path. WIP.md is therefore the one text file here
with no automatic protection -- and it is the file a Windows peer is most likely to rewrite,
since it is edited by hand on whichever machine last held the session. The sibling repos
`retch` (#262) and `etr` (#86) closed the same gap the same way.

WHY IT DOES NOT LIVE IN text_check.py. That helper is vendored into both siblings, and the
three bodies already differ while all declare `TEMPLATE_VERSION = 1`; adding a flag to one copy
deepens the drift (`NOTES.md` section 4.2). This repo has no WIP updater script to host it
either, unlike the siblings, so it stands alone.

AN ABSENT FILE PASSES. WIP.md is per-machine and untracked, so CI and a fresh clone legitimately
have none. Count BYTES rather than reaching for `grep -c` -- `~/AGENTS.md` section 17, where
that idiom returns the file's line count wearing a carriage-return costume.
"""

import argparse
import contextlib
import io
import sys
import tempfile
from pathlib import Path


def check_endings(wip_file):
    """Return 0 if `wip_file` is absent or holds no carriage return, else 1."""
    if not wip_file.exists():
        print("WIP.md absent (per-machine, untracked) -- nothing to check")
        return 0
    data = wip_file.read_bytes()
    cr = data.count(b"\r")
    if cr:
        print(
            f"WIP.md contains {cr} carriage return(s); this tree is LF.\n"
            f"  It is gitignored, so .gitattributes and text-check cannot reach it.\n"
            f"  Fix: python3 -c \"import pathlib;p=pathlib.Path('WIP.md');"
            f"p.write_bytes(p.read_bytes().replace(b'\\r\\n',b'\\n').replace(b'\\r',b''))\"",
            file=sys.stderr,
        )
        return 1
    print(f"WIP.md is LF ({data.count(b'\n')} lines)")
    return 0


def self_test():
    """Prove each outcome, including the two refusals, before the real file is judged."""
    failures = []

    def quietly(path):
        # A control firing is the expected result here; printing its refusal would make a
        # passing self-test read like a failing one.
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return check_endings(path)

    def expect(name, got, want):
        if got != want:
            failures.append(f"{name} -- expected {want!r}, got {got!r}")

    with tempfile.TemporaryDirectory() as td:
        d = Path(td)
        cases = (
            ("LF passes", b"# WIP.md\n\nnotes\n", 0),
            ("no newline at all passes", b"# WIP.md", 0),
            ("empty file passes", b"", 0),
            ("CRLF is refused", b"# WIP.md\r\n\r\nnotes\r\n", 1),
            ("a single lone CR is refused", b"# WIP.md\n\nprogress\rbar\n", 1),
        )
        for i, (name, content, want) in enumerate(cases):
            f = d / f"case{i}.md"
            f.write_bytes(content)
            expect(name, quietly(f), want)
        expect("absent passes", quietly(d / "nope.md"), 0)

    if failures:
        for line in failures:
            print(f"FAIL: {line}", file=sys.stderr)
        return 1
    print("wip_check.py self-test passed")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true", help="run the guard's own controls")
    args = parser.parse_args()
    if args.self_test:
        sys.exit(self_test())
    sys.exit(check_endings(Path(__file__).resolve().parent.parent / "WIP.md"))


if __name__ == "__main__":
    main()
