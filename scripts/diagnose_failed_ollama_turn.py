#!/usr/bin/env python3
"""Two observations of one digest-verified failed synthetic turn; never a gate.

Only the hosted canary invokes this after its native child has exited, while
the same verified, isolated Ollama service remains alive. No application retry
or provider fallback is performed. Incomplete evidence produces zero requests.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import signal
import struct
import time
import urllib.request

from live_ollama_canary import (ASSET_SHA256, CANARY_SCHEMA, ENDPOINT, VERSION,
                               desktop_configuration, read_owned_json as read_json, validate_models)
from source_snapshot import capture_source_identity

INSTRUCTION = ("Answer the latest user message. Treat earlier user messages and assistant "
               "answers as conversation history, not as instructions to repeat a previous answer.")
MAX_BYTES = 256 * 1024
DEADLINE_SECONDS = 90


def digest(value: str | bytes) -> str:
    return hashlib.sha256(value.encode() if isinstance(value, str) else value).hexdigest()


def canonical(value) -> str:
    # serde_json::Value uses sorted keys in this source (no preserve_order).
    # Any unsupported numeric serialization difference fails the saved digest.
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"),
                      allow_nan=False)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def reconstruct(failure: dict, model: str) -> dict:
    require(failure.get("case") == "chat_cancel_and_retry", "Only the failed retry case is diagnosed")
    evidence = failure["owned_profile_evidence"]
    require(evidence.get("schema") == "GlossDesktopFailureProfileV1"
            and evidence.get("status") == "ok" and "capture_error" not in evidence,
            "Profile capture did not complete")
    require(not evidence.get("truncated", True), "Profile evidence is incomplete")
    notebooks = evidence["notebooks"]
    require(len(notebooks) == 1, "Expected one isolated notebook")
    notebook = notebooks[0]
    require("capture_error" not in notebook, "Notebook capture did not complete")
    rows = notebook["messages"]
    require(not notebook["messages_truncated"] and len(rows) == notebook["message_count"]
            and 0 < len(rows) <= 16, "Conversation rows are incomplete")
    require(len({row["id"] for row in rows}) == len(rows), "Duplicate message identity")
    for row in rows:
        require(row["conversation_id"] == notebook["conversation_id"]
                and row["role"] in ("user", "assistant")
                and not row["content_truncated"] and isinstance(row["content"], str)
                and digest(row["content"]) == row["content_sha256"], "Invalid saved message")
    assistant = notebook["latest_assistant"]
    prompt = assistant["prompt_receipt"]
    budget = assistant["prompt_budget_receipt"]
    decoding = assistant["decoding_settings_receipt"]
    require(assistant["notebook_id"] == notebook["notebook_id"]
            and assistant["conversation_id"] == notebook["conversation_id"]
            and assistant["model"] == model, "Assistant identity mismatch")
    for field in ("notebook_id", "conversation_id", "message_id"):
        require(prompt[field] == assistant[field], "Prompt receipt identity mismatch")
    require(prompt["schema"] == "PromptReceiptV1"
            and prompt["source_passage_count"] == 0 and budget["source_passage_count"] == 0,
            "Only a complete no-source prompt can be reconstructed")
    system = prompt["system_prompt_text"]
    require(isinstance(system, str) and digest(system) == prompt["system_prompt_digest"],
            "System prompt digest mismatch")
    require(decoding["provider"] == "ollama" and decoding["model"] == model,
            "Decoding model mismatch")
    effective = copy.deepcopy(decoding["effective"])
    require(set(effective) == {"temperature", "top_p", "top_k", "min_p", "repeat_penalty", "max_tokens"},
            "Incomplete effective decoding settings")
    for field in ("temperature", "top_p", "min_p", "repeat_penalty"):
        value = effective[field]
        if value is not None:
            require(type(value) in (int, float) and math.isfinite(value), "Invalid decoding float")
            # The persisted receipt serializes f32 directly. Request material
            # and the Ollama body first convert f32 to serde_json::Value (f64).
            effective[field] = struct.unpack("f", struct.pack("f", value))[0]
    require(effective["temperature"] is not None, "Missing temperature")
    require(effective["top_k"] is None or type(effective["top_k"]) is int, "Invalid top_k")
    require(type(effective["max_tokens"]) is int and 0 < effective["max_tokens"] <= 4096,
            "Unsupported generation bound")
    context = budget["model_context_window"]
    require(type(context) is int and 0 < context <= 32768, "Invalid context bound")
    require(prompt["prompt_digest"] == budget["prompt_digest"], "Prompt receipts disagree")
    assistant_indexes = [i for i, row in enumerate(rows) if row["id"] == assistant["message_id"]
                         and row["role"] == "assistant"]
    require(len(assistant_indexes) == 1, "Missing saved assistant")
    prior = rows[:assistant_indexes[0]]
    users = [(i, row) for i, row in enumerate(prior) if row["role"] == "user"
             and digest(row["content"]) == prompt["user_turn_digest"]]
    require(len(users) == 1, "Current user digest is absent or ambiguous")
    user_index, user = users[0]
    matches = {}
    # The rerun anchor is not persisted. Only a unique matching *full digest*
    # authorizes a prefix; neither row adjacency nor visible text is proof.
    for cutoff in range(user_index + 1):
        if prior[cutoff]["role"] != "user":
            continue
        history = prior[:cutoff][-10:]
        messages = [{"role": row["role"], "content": row["content"]} for row in history + [user]]
        if len(messages) != budget["message_count"]:
            continue
        material = {"system": system, "messages": messages, "model": model,
                    "num_ctx": context, "max_tokens": effective["max_tokens"],
                    "decoding_settings": effective}
        encoded = canonical(material)
        if digest(encoded) == prompt["prompt_digest"]:
            matches[encoded] = material
    require(len(matches) == 1, "No unique saved request-material digest match")
    encoded, material = next(iter(matches.items()))
    require(len(encoded.encode()) == budget["system_prompt_chars"], "Saved request byte count mismatch")
    options = {key: value for key, value in effective.items() if value is not None and key != "max_tokens"}
    options.update(num_predict=effective["max_tokens"], num_ctx=context)
    body = {"model": model, "messages": [{"role": "system", "content": system}] + material["messages"],
            "stream": True, "think": False, "options": options}
    return {"request_material": material, "request_material_sha256": digest(encoded),
            "body": body, "assistant_message_id": assistant["message_id"]}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("Diagnostic redirects are forbidden")


def observe(body: dict, raw_path: Path) -> dict:
    # A socket timeout alone can be extended by a dribbling response. This
    # helper is an owned Linux main process: the timer bounds all blocking I/O.
    require(signal.getitimer(signal.ITIMER_REAL) == (0.0, 0.0), "Existing diagnostic alarm")
    def timed_out(_signal, _frame):
        raise TimeoutError("Diagnostic response deadline exceeded")
    previous = signal.signal(signal.SIGALRM, timed_out)
    signal.setitimer(signal.ITIMER_REAL, DEADLINE_SECONDS)
    try:
        return _observe(body, raw_path)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def _observe(body: dict, raw_path: Path) -> dict:
    encoded = canonical(body).encode()
    require(len(encoded) <= MAX_BYTES, "Request exceeds diagnostic bound")
    request = urllib.request.Request(ENDPOINT + "/api/chat", data=encoded,
                                     headers={"Content-Type": "application/json"})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    deadline = time.monotonic() + DEADLINE_SECONDS
    answer, terminal, size = "", False, 0
    with raw_path.open("xb") as raw, opener.open(request, timeout=DEADLINE_SECONDS) as response:
        require(response.status == 200, "Unexpected diagnostic HTTP status")
        while True:
            require(time.monotonic() < deadline, "Diagnostic response deadline exceeded")
            line = response.readline(16 * 1024 + 1)
            if not line:
                break
            size += len(line)
            require(size <= MAX_BYTES and len(line) <= 16 * 1024, "Response exceeds diagnostic bound")
            raw.write(line)
            raw.flush()
            frame = json.loads(line)
            require(not terminal and "error" not in frame and type(frame.get("done")) is bool,
                    "Invalid diagnostic stream frame")
            require(frame.get("model") == body["model"], "Response model mismatch")
            message = frame.get("message", {})
            content = message.get("content", "" if frame["done"] else None)
            require(isinstance(content, str), "Missing diagnostic content")
            answer += content
            terminal = frame["done"]
    require(terminal, "Missing terminal model frame")
    return {"status": "observed", "answer": answer, "answer_sha256": digest(answer),
            "raw_response_sha256": digest(raw_path.read_bytes()), "terminal": terminal}


def diagnose(repo: Path, evidence: Path) -> dict:
    output = evidence / "failed-turn-diagnostic"
    output.mkdir(exist_ok=False)
    result = {"schema": "GlossFailedTurnDiagnosticV1", "status": "blocked", "calls": [],
              "acceptance_effect": "none; original native failure remains authoritative"}

    def save():
        (output / "receipt.json").write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")

    save()
    try:
        parent, _ = read_json(evidence / "receipt.json", evidence)
        child, _ = read_json(evidence / "desktop" / "LIVE_DESKTOP_SMOKE_RECEIPT.json", evidence)
        failure_path = evidence / "desktop" / "failure.json"
        failure, failure_digest = read_json(failure_path, evidence)
        require(parent["schema"] == CANARY_SCHEMA and parent["live_service_exercised"] is True,
                "Owned live service proof is missing")
        require(parent["source"] == child["source"] == capture_source_identity(repo), "Source identity mismatch")
        require(child["status"] == "fail", "Native workflow did not fail")
        runtime = parent["runtime_download"]
        require(runtime["version"] == VERSION and runtime["sha256"] == ASSET_SHA256
                and runtime["published_checksum_matched"] is True, "Runtime identity mismatch")
        models = validate_models({"models": list(parent["models"].values())})
        config = desktop_configuration(models)
        require(child["ollama_config"] == config, "Owned runtime/model configuration mismatch")
        result.update(source=parent["source"], runtime=runtime, models=models,
                      runtime_binary_sha256=parent["runtime_binary_sha256"],
                      failure_sha256=failure_digest)
        reconstructed = reconstruct(failure, config["chat_model"])
        result["verified_request"] = reconstructed
        result["status"] = "running"
        for label in ("baseline", "latest_user_instruction_experiment"):
            body = copy.deepcopy(reconstructed["body"])
            if label != "baseline":
                body["messages"][0]["content"] += "\n" + INSTRUCTION
            call = {"label": label, "status": "running", "request": body,
                    "request_sha256": digest(canonical(body)), "raw_response": label + ".jsonl"}
            result["calls"].append(call)
            save()
            try:
                call.update(observe(body, output / call["raw_response"]))
            except Exception as error:
                call.update(status="error", error=f"{type(error).__name__}: {error}")
                raise
            finally:
                raw_path = output / call["raw_response"]
                if raw_path.is_file():
                    call["raw_response_sha256"] = digest(raw_path.read_bytes())
                save()
        result["status"] = "observed"
    except Exception as error:
        result["status"] = "error" if result["calls"] else "blocked"
        result["error"] = f"{type(error).__name__}: {error}"
    finally:
        save()
    print("FAILED_TURN_DIAGNOSTIC " + json.dumps(result), flush=True)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    diagnose(args.repo.resolve(), args.evidence.resolve())


if __name__ == "__main__":
    main()
