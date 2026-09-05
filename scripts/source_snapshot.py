"""Source identity shared by Gloss validation receipts.

Hash the actual checkout, including non-ignored new files. Git HEAD alone cannot
identify a locally edited test run. Symlinks bind their targets without following
them outside the repository. Keep generated receipts in an ignored output path.
"""

from __future__ import annotations

import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path


def _git(root: Path, *args: str) -> bytes:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout


def capture_source_identity(root: Path) -> dict[str, object]:
    root = root.resolve()
    if _git(root, "ls-files", "--unmerged", "-z"):
        raise ValueError("Cannot identify a checkout with unresolved merge entries")
    revision = _git(root, "rev-parse", "HEAD").decode().strip()
    tree = _git(root, "rev-parse", "HEAD^{tree}").decode().strip()
    paths = sorted(set(_git(
        root, "ls-files", "--cached", "--others", "--exclude-standard", "-z"
    ).split(b"\0")) - {b""})
    digest = hashlib.sha256()
    for raw_path in paths:
        relative = os.fsdecode(raw_path)
        path = root / relative
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            record = [relative, "deleted", ""]
        else:
            if stat.S_ISLNK(mode):
                kind, content = "symlink", os.fsencode(os.readlink(path))
            elif stat.S_ISREG(mode):
                kind = "executable" if mode & stat.S_IXUSR else "file"
                content = path.read_bytes()
            else:
                raise ValueError(f"Unsupported source entry: {relative}")
            record = [relative, kind, hashlib.sha256(content).hexdigest()]
        digest.update(json.dumps(record, ensure_ascii=True, separators=(",", ":")).encode())
        digest.update(b"\n")
    return {
        "schema": "GlossSourceSnapshotV1",
        "revision": revision,
        "tree_sha": tree,
        "worktree_clean": not bool(_git(root, "status", "--porcelain", "--untracked-files=all")),
        "source_sha256": digest.hexdigest(),
    }
