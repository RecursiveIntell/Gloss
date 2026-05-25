#!/usr/bin/env node
import { readFileSync } from "node:fs";

const checks = [];

function check(name, condition, detail) {
  checks.push({ name, pass: Boolean(condition), detail });
}

const types = readFileSync("src/lib/types.ts", "utf8");
const chatPanel = readFileSync("src/components/chat/ChatPanel.tsx", "utf8");
const context = readFileSync("src-tauri/src/retrieval/context.rs", "utf8");

check(
  "message citations use evidence envelope",
  /citations\?: ChatEvidencePayload;/.test(types),
  "Message.citations must be a single ChatEvidencePayload shape."
);
check(
  "note citations are citation array only",
  /citations\?: Citation\[\];/.test(types),
  "Note.citations must not accept JSON strings or evidence envelopes."
);
check(
  "no citation union types",
  !/citations\?:[^\n|]*\|/.test(types),
  "citations fields must not be polymorphic unions."
);
check(
  "unknown evidence defaults are not marked preserved",
  /source_scope_preserved: false,/.test(chatPanel),
  "Null evidence must not claim source scope was preserved."
);
check(
  "system prompt excludes quoted passage payloads",
  !/Retrieved passages|lives in the system message/.test(context),
  "Source passage text must be assembled outside the system prompt."
);
check(
  "user turn wraps source data",
  /SOURCE_DATA/.test(context) && /Treat the following blocks as quoted source data/.test(context),
  "Source passages must be wrapped as quoted data in the current user turn."
);

const failed = checks.filter((item) => !item.pass);
if (failed.length) {
  console.error(JSON.stringify({ status: "fail", failed }, null, 2));
  process.exit(1);
}

console.log(JSON.stringify({ status: "pass", checks: checks.map(({ name }) => name) }, null, 2));
