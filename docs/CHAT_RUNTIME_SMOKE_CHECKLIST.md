# Chat Runtime Smoke Checklist

Use this after chat gate scheduling changes.

1. Start Ollama and confirm the target model is available:
   - `ollama pull qwen3.5:4b` or `ollama pull cogito:3b`
   - `ollama run qwen3.5:4b "Reply with only: ok"` or `ollama run cogito:3b "Reply with only: ok"`
2. Launch the Gloss desktop app.
3. Open or create a notebook.
4. Open settings and select the Ollama provider.
5. Select `qwen3.5:4b` or `cogito:3b` as the chat model.
6. Send a short prompt in chat, for example: `In one sentence, explain what this notebook is for.`
7. Verify `chat:status` progresses through context build, GPU gate acquisition, LLM gate acquisition, provider request start, first-token wait, streaming, and complete.
8. If a background summary is running, verify status identifies the blocking gate and owner.
9. Verify tokens stream into the assistant message.
10. Verify the assistant message persists after navigating away and back to the conversation.
11. Repeat once with Ollama cold-started so model load exceeds 45 seconds; verify it does not fail at the old first-token timeout.
