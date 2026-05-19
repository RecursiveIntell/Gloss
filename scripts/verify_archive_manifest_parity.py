#!/usr/bin/env python3
"""Verify a produced SCR archive against its JSON manifest from actual ZIP bytes."""
import hashlib
import json
import sys
import zipfile
from pathlib import Path


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_manifest(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: verify_archive_manifest_parity.py <archive.zip> <manifest.json>", file=sys.stderr)
        return 2
    zip_path = Path(sys.argv[1])
    manifest_path = Path(sys.argv[2])
    manifest = load_manifest(manifest_path)
    files = manifest.get("files")
    if not isinstance(files, list):
        print("manifest missing files[]", file=sys.stderr)
        return 1
    manifest_by_path = {}
    for item in files:
        path = item.get("path")
        if not path or path in manifest_by_path:
            print(f"bad or duplicate manifest path: {path!r}", file=sys.stderr)
            return 1
        manifest_by_path[path] = item

    with zipfile.ZipFile(zip_path) as zf:
        infos = [i for i in zf.infolist() if not i.is_dir()]
        zip_names = [i.filename for i in infos]
        zip_set = set(zip_names)
        manifest_set = set(manifest_by_path)
        missing = sorted(manifest_set - zip_set)
        extra = sorted(zip_set - manifest_set)
        if missing or extra:
            print("archive/manifest path mismatch", file=sys.stderr)
            if missing:
                print("missing from zip:", file=sys.stderr)
                for p in missing[:200]:
                    print(f"  {p}", file=sys.stderr)
            if extra:
                print("extra in zip:", file=sys.stderr)
                for p in extra[:200]:
                    print(f"  {p}", file=sys.stderr)
            return 1
        for info in infos:
            expected = manifest_by_path[info.filename]
            data = zf.read(info.filename)
            actual_hash = sha256_bytes(data)
            if expected.get("sha256") != actual_hash:
                print(f"hash mismatch: {info.filename}: manifest={expected.get('sha256')} actual={actual_hash}", file=sys.stderr)
                return 1
            if expected.get("bytes") != len(data):
                print(f"byte count mismatch: {info.filename}: manifest={expected.get('bytes')} actual={len(data)}", file=sys.stderr)
                return 1
    archive_hash = sha256_bytes(zip_path.read_bytes())
    manifest_archive_hash = manifest.get("archive_zip_byte_sha256") or manifest.get("report", {}).get("archive_zip_byte_sha256")
    if manifest_archive_hash and manifest_archive_hash != archive_hash:
        print(f"archive sha mismatch: manifest={manifest_archive_hash} actual={archive_hash}", file=sys.stderr)
        return 1
    print(f"ok archive_manifest_parity files={len(files)} archive_sha256={archive_hash}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
