---
name: p32r3-runtime-status
description: Use for Gloss chat/Ollama runtime status, gate ownership, timeouts, and dynamic context sizing.
---

Implement typed `chat:status` events from Rust to frontend, first-token timeout, stream-idle timeout, dynamic context sizing, and stream truncation disclosure. Gate owner/wait state must be visible to the user. Do not synthesize clean completion for incomplete provider streams.
