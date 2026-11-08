#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 l1a
"""Verify that `tests/golden/` argv snapshots are current and tracked in git.

WHY THIS SCRIPT EXISTS
----------------------
`golden-is-current` runs inside `just check` on Linux, macOS, and Windows. Previously it
was a `#!/usr/bin/env bash` shebang recipe calling `find`, `sha256sum`, and `sort`, none of
which are on a default Windows PATH (where `find` resolves to `C:\\Windows\\system32\\find.exe`
and `cygpath` is required for any shebang recipe). Moving the check into Python (invoked via
`"{{PY}}"`) makes `just check` 100% shell-free across all platforms.

HOW IT CHECKS
-------------
1. Staleness is checked by content, not by git state: compute SHA-256 digests of every
   `tests/golden/**/*.txt` file before and after running
   `RP_UPDATE_GOLDEN=1 cargo test --test cli_tests golden_ -q`. Comparing against `git status`
   instead would fail on goldens that are staged but not yet committed.
2. Untracked files under `tests/golden/` are refused (`git ls-files --others --exclude-standard
   -- tests/golden`), because CI would regenerate an untracked golden from scratch and never
   notice a change.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable

TEMPLATE_VERSION = 1

REPO_ROOT = Path(__file__).resolve().parent.parent

_GIT_LOCAL_ENV_VARS = frozenset(
    {
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_SUPER_PREFIX",
    }
)


def _git_env() -> dict[str, str]:
    env = {
        k: v
        for k, v in os.environ.items()
        if k not in _GIT_LOCAL_ENV_VARS
        and not k.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))
    }
    env["GIT_TERMINAL_PROMPT"] = "0"
    return env


def snapshot_hashes(golden_dir: Path) -> dict[str, str]:
    """Return `{rel_posix_path: sha256_hex}` for all `*.txt` files under `golden_dir`."""
    if not golden_dir.is_dir():
        return {}
    out: dict[str, str] = {}
    for path in sorted(golden_dir.rglob("*.txt")):
        if path.is_file():
            rel = path.relative_to(golden_dir).as_posix()
            out[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
    return out


def _default_regenerator(repo_root: Path) -> int:
    env = dict(os.environ)
    env["RP_UPDATE_GOLDEN"] = "1"
    proc = subprocess.run(
        ["cargo", "test", "--test", "cli_tests", "golden_", "-q"],
        cwd=repo_root,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if proc.returncode != 0 and proc.stderr:
        print(proc.stderr, file=sys.stderr, end="")
    return proc.returncode


def check_golden(
    repo_root: Path = REPO_ROOT,
    regenerator: Callable[[Path], int] = _default_regenerator,
) -> tuple[int, str]:
    """Run `regenerator(repo_root)` and verify `tests/golden` neither changed nor has untracked files."""
    golden_dir = repo_root / "tests" / "golden"
    before = snapshot_hashes(golden_dir)
    rc = regenerator(repo_root)
    if rc != 0:
        return rc, "error: `RP_UPDATE_GOLDEN=1 cargo test --test cli_tests golden_` failed"
    after = snapshot_hashes(golden_dir)

    if before != after:
        diff_proc = subprocess.run(
            ["git", "-C", str(repo_root), "--no-pager", "diff", "--", "tests/golden"],
            cwd=repo_root,
            env=_git_env(),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        msg = (
            "tests/golden is stale — the argv rusticprofile would run has changed.\n"
            "Review the diff below; if the change is intended, commit it."
        )
        if diff_proc.stdout.strip():
            msg = f"{msg}\n{diff_proc.stdout.rstrip()}"
        return 1, msg

    untracked_proc = subprocess.run(
        [
            "git",
            "-C",
            str(repo_root),
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            "tests/golden",
        ],
        cwd=repo_root,
        env=_git_env(),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    untracked = untracked_proc.stdout.strip()
    if untracked:
        return 1, f"untracked golden files — git add them:\n{untracked}"

    return 0, f"golden snapshots are current ({len(after)} files)"


def self_test() -> int:
    failures: list[str] = []

    def check(label: str, cond: bool, detail: str = "") -> None:
        if not cond:
            failures.append(f"{label}: {detail}")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "repo"
        golden = root / "tests" / "golden"
        golden.mkdir(parents=True)
        f1 = golden / "a.txt"
        f1.write_text("rustic backup\n", encoding="utf-8")

        env = _git_env()
        env["GIT_CONFIG_NOSYSTEM"] = "1"
        env["GIT_CONFIG_GLOBAL"] = os.devnull
        for args in (
            ("init", "-q"),
            ("config", "user.name", "Test"),
            ("config", "user.email", "test@example.com"),
            ("config", "commit.gpgsign", "false"),
            ("add", "tests/golden/a.txt"),
            ("commit", "-q", "-m", "init"),
        ):
            subprocess.run(["git", "-C", str(root), *args], env=env, check=True, capture_output=True)

        rc, msg = check_golden(root, regenerator=lambda _: 0)
        check("unchanged tracked golden passes", rc == 0, msg)

        def _mutating_regen(r: Path) -> int:
            (r / "tests" / "golden" / "a.txt").write_text("rustic backup --dry-run\n", encoding="utf-8")
            return 0

        rc, msg = check_golden(root, regenerator=_mutating_regen)
        check("stale golden is refused", rc == 1 and "tests/golden is stale" in msg, f"rc={rc} msg={msg!r}")

        f1.write_text("rustic backup\n", encoding="utf-8")
        (golden / "untracked.txt").write_text("new\n", encoding="utf-8")
        rc, msg = check_golden(root, regenerator=lambda _: 0)
        check(
            "untracked golden is refused",
            rc == 1 and "untracked golden files" in msg and "untracked.txt" in msg,
            f"rc={rc} msg={msg!r}",
        )

    if failures:
        for f in failures:
            print(f"FAIL: {f}", file=sys.stderr)
        return 1
    print(f"golden_check.py self-test passed (template v{TEMPLATE_VERSION})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true", help="run built-in self-test and exit")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    rc, msg = check_golden()
    stream = sys.stderr if rc != 0 else sys.stdout
    print(msg, file=stream)
    return rc


if __name__ == "__main__":
    sys.exit(main())
