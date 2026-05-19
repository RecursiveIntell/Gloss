#!/usr/bin/env python3
"""Validate SCR generated schema strictness using stdlib checks."""
import json
import sys
from pathlib import Path

ROOT = Path.cwd()
SCHEMA_DIR = ROOT / "schemas" / "generated"
REQUIRED = [
    "control-evaluation-input-v1.schema.json",
    "control-decision-receipt-v1.schema.json",
    "audit-fixture-case-v1.schema.json",
]
REQUIRED_SCHEMA_VERSIONS = {
    "control-evaluation-input-v1.schema.json": "control_evaluation_input_v1",
    "control-decision-receipt-v1.schema.json": "control_decision_receipt_v1",
    "audit-fixture-case-v1.schema.json": "audit_fixture_case_v1",
}


def iter_objects(node, path="$"):
    if isinstance(node, dict):
        yield path, node
        for k, v in node.items():
            yield from iter_objects(v, f"{path}.{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            yield from iter_objects(v, f"{path}[{i}]")


def collect_additional_properties_failures(schema, name):
    errors = []
    for obj_path, obj in iter_objects(schema):
        if not isinstance(obj, dict):
            continue
        if obj.get("type") == "object" and obj.get("properties") and obj.get("additionalProperties") is not False:
            errors.append(f"{name}: object schema missing additionalProperties=false at {obj_path}")
    return errors


def validate_schema_version_contract(schema, name, expected_version):
    errors = []
    version_props = []
    for obj_path, obj in iter_objects(schema):
        if not isinstance(obj, dict):
            continue
        props = obj.get("properties")
        if isinstance(props, dict) and "schema_version" in props:
            version_props.append((obj_path, props["schema_version"]))

    if not version_props:
        errors.append(f"{name}: schema_version appears nowhere")
        return errors

    for obj_path, prop in version_props:
        if not isinstance(prop, dict):
            errors.append(f"{name}: schema_version at {obj_path} is not object schema")
            continue
        const = prop.get("const")
        enum = prop.get("enum")
        if const is not None:
            if const != expected_version:
                errors.append(
                    f"{name}: schema_version const mismatch at {obj_path}: {const} != {expected_version}"
                )
        elif isinstance(enum, list):
            if enum != [expected_version]:
                errors.append(
                    f"{name}: schema_version enum mismatch at {obj_path}: {enum} != [{expected_version}]"
                )
        else:
            errors.append(f"{name}: schema_version at {obj_path} must be const or enum [{expected_version}]")
    return errors


def validate_score_bounds(schema, name):
    errors = []
    for obj_path, obj in iter_objects(schema):
        if not isinstance(obj, dict):
            continue
        obj_type = obj.get("type")
        if obj_type == "integer":
            key = obj_path.rsplit(".", 1)[-1]
            if key in {"ScoreBps", "WeightBps"}:
                if obj.get("minimum") not in {0, 0.0}:
                    errors.append(f"{name}: {key} missing minimum 0 at {obj_path}")
                if obj.get("maximum") not in {10000, 10000.0}:
                    errors.append(f"{name}: {key} missing maximum 10000 at {obj_path}")
            if obj.get("format") == "uint16" and obj.get("minimum") is None:
                errors.append(f"{name}: integer schema missing minimum at {obj_path}")
    return errors


def main() -> int:
    errors = []
    if not SCHEMA_DIR.exists():
        print(f"missing schema dir: {SCHEMA_DIR}", file=sys.stderr)
        return 1
    for name in REQUIRED:
        path = SCHEMA_DIR / name
        if not path.exists():
            errors.append(f"missing schema: {name}")
            continue
        try:
            schema = json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:
            errors.append(f"invalid json {name}: {exc}")
            continue

        errors.extend(collect_additional_properties_failures(schema, name))

        expected_version = REQUIRED_SCHEMA_VERSIONS.get(name)
        if expected_version is None:
            errors.append(f"{name}: missing required schema version mapping")
            continue
        errors.extend(validate_schema_version_contract(schema, name, expected_version))
        errors.extend(validate_score_bounds(schema, name))

    if errors:
        print("strict schema validation failed:", file=sys.stderr)
        for err in errors[:500]:
            print(f"  {err}", file=sys.stderr)
        return 1

    print("ok strict schemas")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
