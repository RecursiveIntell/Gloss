#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

pattern='reqwest|ureq|hyper::|tokio::net|std::net|TcpStream|UdpSocket|OpenAI|embedding|chat_completion|model_call'

if grep -RInE "$pattern" crates --exclude-dir=target; then
  echo "LLM/model/network call surface found in production code" >&2
  exit 1
fi

echo "no LLM/model/network call surface found"
