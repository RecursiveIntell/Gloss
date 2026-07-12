#!/usr/bin/env python3
"""Verify the source-derived Gloss Tauri command and event contract.

The JSON file is an explicit, reviewable contract, but it is not a hand-kept
inventory: this gate re-derives registrations, command definitions, frontend
invokes/wrappers/callers, and Tauri event emit/listen names from the source.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


TS_GLOBS = ("*.ts", "*.tsx")


def rel(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def source_paths(root: Path, base: str, globs: tuple[str, ...]) -> list[Path]:
    directory = root / base
    if not directory.exists():
        return []
    return sorted({path for glob in globs for path in directory.rglob(glob) if path.is_file()})


def balanced(
    text: str,
    open_at: int,
    opener: str = "{",
    closer: str = "}",
    *,
    track_quotes: bool = True,
) -> tuple[str, int]:
    """Return the balanced segment beginning at *open_at*, without parsing TS."""
    if open_at < 0 or open_at >= len(text) or text[open_at] != opener:
        return "", open_at
    depth = 0
    quote: str | None = None
    escaped = False
    for index in range(open_at, len(text)):
        char = text[index]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if track_quotes and char in "'\"`":
            quote = char
        elif char == opener:
            depth += 1
        elif char == closer:
            depth -= 1
            if depth == 0:
                return text[open_at : index + 1], index + 1
    return "", open_at


def registered_commands(root: Path) -> set[str]:
    path = root / "src-tauri/src/lib.rs"
    if not path.exists():
        return set()
    text = path.read_text(encoding="utf-8")
    marker = text.find("tauri::generate_handler![")
    if marker == -1:
        return set()
    open_at = text.find("[", marker)
    block, _ = balanced(text, open_at, "[", "]")
    return set(re.findall(r"commands::(?:[A-Za-z_]\w*::)*(\w+)", block))


def command_definitions(root: Path) -> dict[str, str]:
    definitions: dict[str, str] = {}
    pattern = re.compile(
        r"#\[tauri::command\][\s\S]{0,240}?\bpub\s+async\s+fn\s+(\w+)\s*\("
    )
    for path in source_paths(root, "src-tauri/src", ("*.rs",)):
        for match in pattern.finditer(path.read_text(encoding="utf-8")):
            name = match.group(1)
            previous = definitions.get(name)
            if previous:
                raise ValueError(f"duplicate #[tauri::command] definition for {name}: {previous}, {rel(root, path)}")
            definitions[name] = rel(root, path)
    return definitions


def camel_case(field: str) -> str:
    head, *tail = field.split("_")
    return head + "".join(part.capitalize() for part in tail)


def rust_response_family(value: str) -> str:
    value = normalize_type(value)
    if value == "()":
        return "void"
    vector = re.fullmatch(r"Vec<(.+)>", value)
    return f"{normalize_type(vector.group(1))}[]" if vector else value


def command_signatures(root: Path) -> dict[str, dict[str, Any]]:
    """Extract operator-only request/response shapes from public Rust commands."""
    signatures: dict[str, dict[str, Any]] = {}
    start = re.compile(r"#\[tauri::command\]\s*pub\s+async\s+fn\s+(\w+)\s*\(")
    for path in source_paths(root, "src-tauri/src", ("*.rs",)):
        text = path.read_text(encoding="utf-8")
        for match in start.finditer(text):
            name = match.group(1)
            params_open = text.find("(", match.start())
            params, params_end = balanced(text, params_open, "(", ")", track_quotes=False)
            header = text[params_end : text.find("{", params_end)]
            response = re.search(r"->\s*Result\s*<\s*(.+?)\s*,\s*([^>]+)\s*>", header)
            fields = [
                camel_case(field)
                for field in re.findall(r"(?m)^\s*(\w+)\s*:", params)
                if field not in {"state", "queue", "app_handle"}
            ]
            signatures[name] = {
                "fields": fields,
                "response_family": rust_response_family(response.group(1)) if response else "unknown",
                "error_family": normalize_type(response.group(2)) if response else "unknown",
            }
    return signatures


def normalize_type(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip()


def request_fields(object_text: str) -> list[str]:
    if not object_text:
        return []
    fields: list[str] = []
    for item in object_text[1:-1].split(","):
        match = re.match(r"\s*([A-Za-z_$][\w$]*)\s*(?=[:},]|$)", item)
        if match:
            fields.append(match.group(1))
    return fields


def wrapper_inventory(root: Path) -> dict[str, dict[str, Any]]:
    path = root / "src/lib/tauri.ts"
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8")
    start = re.compile(r"export\s+async\s+function\s+(\w+)\s*\(")
    wrappers: dict[str, dict[str, Any]] = {}
    for match in start.finditer(text):
        name = match.group(1)
        args_open = text.find("(", match.start())
        _, args_end = balanced(text, args_open, "(", ")")
        body_open = text.find("{", args_end)
        if body_open == -1:
            continue
        header = text[args_end:body_open]
        response = re.search(r":\s*Promise\s*<([\s\S]+)>\s*$", header)
        body, _ = balanced(text, body_open)
        invoke_match = re.search(r"\binvoke\s*\(\s*['\"]([^'\"]+)['\"]", body)
        if not invoke_match:
            continue
        command = invoke_match.group(1)
        after_name = invoke_match.end()
        object_open = body.find("{", after_name)
        object_text = ""
        if object_open != -1:
            object_text, _ = balanced(body, object_open)
        if command in wrappers:
            raise ValueError(f"multiple frontend wrappers invoke {command}")
        wrappers[command] = {
            "name": name,
            "request_fields": request_fields(object_text),
            "response_family": normalize_type(response.group(1)) if response else "unknown",
        }
    return wrappers


def frontend_invokes(root: Path) -> set[str]:
    result: set[str] = set()
    for path in source_paths(root, "src", TS_GLOBS):
        text = path.read_text(encoding="utf-8")
        result.update(re.findall(r"\binvoke\s*\(\s*['\"]([^'\"]+)['\"]", text))
    return result


def wrapper_callers(root: Path, wrapper_names: set[str]) -> dict[str, list[str]]:
    callers: dict[str, list[str]] = {name: [] for name in wrapper_names}
    for path in source_paths(root, "src", TS_GLOBS):
        path_text = rel(root, path)
        if path_text == "src/lib/tauri.ts" or "/__tests__/" in path_text or ".test." in path_text:
            continue
        text = path.read_text(encoding="utf-8")
        for name in wrapper_names:
            if re.search(rf"\b{re.escape(name)}\s*\(", text):
                callers[name].append(path_text)
    return callers


def event_inventory(root: Path) -> tuple[dict[str, list[str]], dict[str, list[str]], dict[str, list[str]]]:
    emitters: dict[str, list[str]] = defaultdict(list)
    listeners: dict[str, list[str]] = defaultdict(list)
    listener_types: dict[str, list[str]] = defaultdict(list)
    rust_paths = source_paths(root, "src-tauri/src", ("*.rs",))
    rust_paths += source_paths(root, "src-tauri/vendor/tauri-queue/src", ("*.rs",))
    for path in rust_paths:
        text = path.read_text(encoding="utf-8")
        for event in re.findall(r"\.emit\s*\(\s*['\"]([^'\"]+)['\"]", text):
            emitters[event].append(rel(root, path))
    for path in source_paths(root, "src", TS_GLOBS):
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(
            r"\blisten\s*(?:<\s*([^>]+?)\s*>)?\s*\(\s*['\"]([^'\"]+)['\"]",
            text,
            re.DOTALL,
        ):
            payload_type, event = match.groups()
            listeners[event].append(rel(root, path))
            if payload_type:
                listener_types[event].append(normalize_type(payload_type))
    return (
        {event: sorted(set(paths)) for event, paths in emitters.items()},
        {event: sorted(set(paths)) for event, paths in listeners.items()},
        {event: sorted(set(types)) for event, types in listener_types.items()},
    )


def is_camel_case(field: str) -> bool:
    return "_" not in field and bool(re.fullmatch(r"[a-z][A-Za-z0-9]*", field))


def compare_set(label: str, actual: set[str], expected: set[str], failures: list[str]) -> None:
    if actual != expected:
        failures.append(
            f"{label} drift: missing={sorted(actual - expected)}, stale={sorted(expected - actual)}"
        )


def verify_contract(root: Path) -> list[str]:
    root = root.resolve()
    schema_path = root / "schemas/tauri-contract-v1.json"
    if not schema_path.exists():
        return [f"missing contract schema: {schema_path}"]
    try:
        contract = json.loads(schema_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        return [f"invalid contract JSON: {error}"]
    if contract.get("version") != "tauri-contract-v1":
        return ["contract version must be tauri-contract-v1"]

    failures: list[str] = []
    commands = contract.get("commands")
    events = contract.get("events")
    if not isinstance(commands, (list, dict)) or not isinstance(events, (list, dict)):
        return ["contract must contain commands and events collections"]
    if isinstance(commands, dict):
        command_entries = {name: {"name": name, **entry} for name, entry in commands.items() if isinstance(entry, dict)}
    else:
        command_entries = {entry.get("name"): entry for entry in commands if isinstance(entry, dict) and entry.get("name")}
    if isinstance(events, dict):
        event_entries = {name: {"name": name, **entry} for name, entry in events.items() if isinstance(entry, dict)}
    else:
        event_entries = {entry.get("name"): entry for entry in events if isinstance(entry, dict) and entry.get("name")}
    if len(command_entries) != len(commands):
        failures.append("contract command names must be present and unique")
    if len(event_entries) != len(events):
        failures.append("contract event names must be present and unique")

    try:
        registered = registered_commands(root)
        definitions = command_definitions(root)
        signatures = command_signatures(root)
        wrappers = wrapper_inventory(root)
        invokes = frontend_invokes(root)
        callers = wrapper_callers(root, {entry["name"] for entry in wrappers.values()})
        emitters, listeners, listener_types = event_inventory(root)
    except ValueError as error:
        return [str(error)]

    operator_commands = {
        name for name, entry in command_entries.items() if entry.get("operator_only") is True
    }
    compare_set("registered commands", registered, set(command_entries), failures)
    compare_set("tauri command definitions", set(definitions), registered, failures)
    compare_set("frontend invokes", invokes, registered - operator_commands, failures)
    compare_set("frontend wrapper commands", set(wrappers), invokes, failures)

    for command in sorted(registered):
        entry = command_entries.get(command, {})
        wrapper = wrappers.get(command)
        if entry.get("registered") is not True:
            failures.append(f"{command}: registered must be true")
        if entry.get("definition") != definitions.get(command):
            failures.append(f"{command}: definition mismatch (expected {definitions.get(command)})")
        operator_only = entry.get("operator_only") is True
        if operator_only:
            if command in wrappers or command in invokes:
                failures.append(f"{command}: operator-only command must not expose a frontend invoke")
            if entry.get("wrapper") is not None or entry.get("callers") not in ([], None):
                failures.append(f"{command}: operator-only command cannot declare frontend wrapper or callers")
            if not isinstance(entry.get("rationale"), str) or not entry["rationale"].strip():
                failures.append(f"{command}: operator-only command requires rationale")
            signature = signatures.get(command)
            expected_request = entry.get("request", {})
            if not signature:
                failures.append(f"{command}: operator-only command signature was not found")
            else:
                expected_casing = "none" if not signature["fields"] else "camelCase"
                if expected_request.get("fields") != signature["fields"] or expected_request.get("casing") != expected_casing:
                    failures.append(f"{command}: operator-only request mismatch (expected {expected_casing} {signature['fields']})")
                if entry.get("response_family") != signature["response_family"]:
                    failures.append(f"{command}: operator-only response family mismatch (expected {signature['response_family']})")
                if entry.get("error_family") != signature["error_family"]:
                    failures.append(f"{command}: operator-only error family mismatch (expected {signature['error_family']})")
            continue
        if not wrapper:
            failures.append(f"{command}: registered public command needs a frontend wrapper")
            continue
        if entry.get("wrapper") != wrapper["name"]:
            failures.append(f"{command}: wrapper mismatch (expected {wrapper['name']})")
        actual_callers = callers.get(wrapper["name"], [])
        if entry.get("caller_count") != len(actual_callers) or not actual_callers:
            failures.append(f"{command}: caller_count mismatch or empty (expected {len(actual_callers)})")
        expected_request = entry.get("request", {})
        actual_fields = wrapper["request_fields"]
        actual_casing = "none" if not actual_fields else ("camelCase" if all(is_camel_case(field) for field in actual_fields) else "other")
        if expected_request.get("fields") != actual_fields or expected_request.get("casing") != actual_casing:
            failures.append(
                f"{command}: request mismatch (expected {actual_casing} {actual_fields})"
            )
        if entry.get("response_family") != wrapper["response_family"]:
            failures.append(
                f"{command}: response family mismatch (expected {wrapper['response_family']})"
            )
        if not isinstance(entry.get("error_family"), str) or not entry["error_family"]:
            failures.append(f"{command}: error_family is required")

    actual_event_names = set(emitters) | set(listeners)
    compare_set("event emit/listen names", actual_event_names, set(event_entries), failures)
    for event in sorted(actual_event_names):
        entry = event_entries.get(event, {})
        actual_emitters = emitters.get(event, [])
        actual_listeners = listeners.get(event, [])
        if entry.get("emitters") != actual_emitters:
            failures.append(f"{event}: emitters mismatch (expected {actual_emitters})")
        if entry.get("listeners") != actual_listeners:
            failures.append(f"{event}: listeners mismatch (expected {actual_listeners})")
        payload_ref = entry.get("payload_schema_ref")
        if not isinstance(payload_ref, str) or not payload_ref:
            failures.append(f"{event}: payload_schema_ref is required")
        elif listener_types.get(event) and payload_ref not in listener_types[event]:
            failures.append(f"{event}: payload_schema_ref mismatch (listener uses {listener_types[event]})")
        if not isinstance(entry.get("sequence_scope"), str) or not entry["sequence_scope"]:
            failures.append(f"{event}: sequence_scope is required")
        if entry.get("terminal_status") not in {"terminal", "nonterminal", "conditional"}:
            failures.append(f"{event}: terminal_status must be terminal, nonterminal, or conditional")
        if not actual_listeners and not isinstance(entry.get("rationale"), str):
            failures.append(f"{event}: unlistened event requires rationale")

    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", nargs="?", default=".")
    args = parser.parse_args()
    failures = verify_contract(Path(args.repo))
    if failures:
        print("FAIL: Tauri IPC/event contract drift:")
        print("\\n".join(f"- {failure}" for failure in failures))
        return 1
    print("PASS: Tauri IPC/event contract matches current source")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
