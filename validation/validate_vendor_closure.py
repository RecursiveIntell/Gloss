#!/usr/bin/env python3
"""Fail early on malformed vendored manifests and missing local dependencies.

This is a packaging check, not a compile/test or a runtime dependency-identity claim.
Optional dependencies are checked too: Cargo must be able to resolve their paths.
"""
from pathlib import Path
import argparse
import tomllib


def validate(root: Path) -> list[str]:
    failures: list[str] = []
    vendor = root / 'src-tauri/vendor'
    if not vendor.is_dir():
        return ['missing src-tauri/vendor']
    for path in sorted(vendor.rglob('*')):
        if path.is_symlink() and not path.exists():
            failures.append(f'{path.relative_to(root)}: dangling symlink')
    for manifest in sorted(vendor.rglob('Cargo.toml')):
        label = str(manifest.relative_to(root))
        try:
            data = tomllib.loads(manifest.read_text())
        except (ValueError, OSError) as error:
            failures.append(f'{label}: invalid manifest: {error}')
            continue

        def visit(table: dict) -> None:
            for key, value in table.items():
                if not isinstance(value, dict):
                    continue
                if key in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                    for name, dep in value.items():
                        if isinstance(dep, dict) and 'path' in dep:
                            target = manifest.parent / dep['path'] / 'Cargo.toml'
                            if not target.is_file():
                                failures.append(f'{label}: {name} missing at {dep["path"]}')
                else:
                    visit(value)
        visit(data)
        for member in data.get('workspace', {}).get('members', []):
            if not any((p / 'Cargo.toml').is_file() for p in manifest.parent.glob(member)):
                failures.append(f'{label}: missing workspace member {member}')
    return failures


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', nargs='?', type=Path, default=Path('.'))
    args = parser.parse_args()
    errors = validate(args.root.resolve())
    for error in errors:
        print(f'FAIL: {error}')
    if not errors:
        print('PASS: vendored TOML, local dependency paths and workspace members are present')
    raise SystemExit(bool(errors))
