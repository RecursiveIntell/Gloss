#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path


PNG = b"\x89PNG\r\n\x1a\n"
JPEG = b"\xff\xd8\xff"


def signature(path: Path) -> str:
    data = path.read_bytes()[:8]
    if data.startswith(PNG):
        return "png"
    if data.startswith(JPEG):
        return "jpeg"
    return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser(description="P33 GUI asset normalization gate.")
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    findings: list[dict[str, str]] = []

    manifest = repo / "src/assets/gloss-gui/ASSET_MANIFEST.json"
    if not manifest.exists():
        findings.append(
            {
                "severity": "error",
                "code": "missing-production-asset-manifest",
                "path": "src/assets/gloss-gui/ASSET_MANIFEST.json",
            }
        )

    required_assets = [
        "src/assets/gloss-gui/canvas-overview.jpg",
        "src/assets/gloss-gui/pasted-1779172952776-0.png",
        "src/assets/gloss-gui/profile pic.jpg",
    ]
    for rel in required_assets:
        path = repo / rel
        if not path.exists():
            findings.append({"severity": "error", "code": "missing-production-asset", "path": rel})
            continue
        ext = path.suffix.lower()
        sig = signature(path)
        if ext == ".png" and sig != "png":
            findings.append({"severity": "error", "code": "asset-extension-signature-mismatch", "path": rel})
        if ext in {".jpg", ".jpeg"} and sig != "jpeg":
            findings.append({"severity": "error", "code": "asset-extension-signature-mismatch", "path": rel})

    for rel in [
        "docs/design/GLOSS_GUI_REFERENCE_20260519/screenshots/canvas-overview.png",
        "docs/design/GLOSS_GUI_REFERENCE_20260519/uploads/profile pic",
    ]:
        path = repo / rel
        if path.exists():
            sig = signature(path)
            if path.suffix.lower() == ".png" and sig != "png":
                findings.append(
                    {
                        "severity": "warning",
                        "code": "reference-asset-extension-signature-mismatch",
                        "path": rel,
                        "detail": f"reference-only asset has {sig} bytes",
                    }
                )
            if not path.suffix:
                findings.append(
                    {
                        "severity": "warning",
                        "code": "reference-asset-has-no-extension",
                        "path": rel,
                        "detail": f"reference-only asset has {sig} bytes",
                    }
                )

    src_html = list((repo / "src").rglob("*.html")) if (repo / "src").exists() else []
    if src_html:
        findings.append(
            {
                "severity": "error",
                "code": "prototype-html-in-production-src",
                "path": ", ".join(str(path.relative_to(repo)) for path in src_html),
            }
        )

    errors = [finding for finding in findings if finding.get("severity") == "error"]
    result = {
        "ok": not errors,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
