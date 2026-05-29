# Gloss Release-Grade Hardening Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Eliminate all CRITICAL and HIGH findings from the hostile audit; harden MEDIUM findings; achieve release-grade quality.

**Architecture:** Six phases ordered by severity and dependency. Phase 1 (critical safety) must land first because Ollama expect() can crash the process and chat:done loss causes permanent spinner. Phase 2 (data integrity) must land before Phase 6 (architecture) because making `conn` private requires all transaction-unsafe paths to be fixed first. Phases 3-5 can partially overlap but are sequenced for clarity.

**Tech Stack:** Rust (Tauri backend), TypeScript/React (frontend), SQLite (rusqlite), Zustand (state management)

---

## Phase 1: Critical Safety Fixes (3 tasks)

### Task 1.1: Replace Ollama SSE expect() with error handling

**Objective:** Prevent process panic on malformed Ollama responses.

**Files:**
- Modify: `src-tauri/src/providers/ollama.rs:325`

**Step 1:** Change the `expect()` in `ollama_chat_token_from_value` test usage and the production call site.

In production code at ollama.rs, find every call to `ollama_chat_token_from_value(&value)` that uses `.expect()` and replace with proper `match` or `?` propagation. The production call site is in the SSE streaming loop — change from:
```rust
let token = ollama_chat_token_from_value(&value).expect("normal frame should parse");
```
To:
```rust
let token = match ollama_chat_token_from_value(&value) {
    Ok(t) => t,
    Err(e) => {
        tracing::warn!(?value, error = %e, "Skipping malformed Ollama SSE frame");
        continue;
    }
};
```

**Step 2:** Verify test still compiles. The test at line 325 uses `.expect()` on a known-good frame — change test to use `assert!(result.is_ok())` pattern:
```rust
let result = ollama_chat_token_from_value(&value);
assert!(result.is_ok(), "normal frame should parse: {:?}", result);
let token = result.unwrap();
```

**Step 3:** Run: `cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::ollama::tests`
**Step 4:** Commit: `fix(ollama): replace expect() with graceful SSE frame skip on parse failure`

---

### Task 1.2: Emit chat:done (or chat:cancelled) on assistant persistence failure

**Objective:** Prevent permanent spinner when assistant message DB write fails.

**Files:**
- Modify: `src-tauri/src/commands/chat/mod.rs:2576-2600`

**Step 1:** After `emit_chat_error` at line 2589-2595, add `emit_chat_done` before the `return`:
```rust
if let Err(e) = app_state.with_notebook_db(&nb_id, |db| db.insert_message(&assistant_msg)) {
    tracing::error!(message_id = %msg_id, "Failed to persist assistant message: {}", e);
    record_chat_attempt_trace(/* ... existing args ... */);
    emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &format!("Assistant message persistence failed: {e}"));
    emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);  // <-- ADD THIS
    app_state.clear_gate_owner("GPU gate", "chat");
    app_state.clear_gate_owner("LLM gate", "chat");
    drop(gpu_permit);
    drop(permit);
    return;
}
```

**Step 2:** Frontend must handle receiving `chat:done` after `chat:error` gracefully. Check `chatStore.ts` — `finalizeMessage` already guards against empty `streamingContent` with an error. Verify that `chat:error` + `chat:done` in sequence doesn't cause double state mutation. The current flow: `onChatError` sets `streamingError` + `isStreaming: false`, then `onChatDone` calls `finalizeMessage` which checks `streamingContent.trim()` and bails if empty. This is safe.

**Step 3:** Commit: `fix(chat): emit chat:done on assistant persistence failure to unblock frontend`

---

### Task 1.3: Wrap replace_models and set_selected_sources in SQL transactions

**Objective:** Prevent data loss on crash between DELETE and INSERT.

**Files:**
- Modify: `src-tauri/src/db/app_db.rs:244-273` (replace_models)
- Modify: `src-tauri/src/db/notebook_db/mod.rs:492-512` (set_selected_sources)

**Step 1:** Wrap `replace_models` in a transaction:
```rust
pub fn replace_models(&self, provider_id: &str, models: &[ModelRecord]) -> Result<(), GlossError> {
    let tx = self.conn.unchecked_transaction()?;
    tx.execute("DELETE FROM models WHERE provider_id = ?1", [provider_id])?;
    let mut stmt = tx.prepare(
        "INSERT INTO models (id, provider_id, display_name, parameter_size, context_window, capabilities, available, stale, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for m in models {
        stmt.execute(rusqlite::params![m.id, m.provider_id, m.display_name, m.parameter_size, m.context_window, m.capabilities, m.available, m.stale, m.last_error])?;
    }
    tx.execute("UPDATE providers SET last_refreshed = datetime('now') WHERE id = ?1", [provider_id])?;
    tx.commit()?;
    Ok(())
}
```

**Step 2:** Wrap `set_selected_sources` in a transaction:
```rust
pub fn set_selected_sources(&self, selected_ids: &[String]) -> Result<(), GlossError> {
    let tx = self.conn.unchecked_transaction()?;
    tx.execute("UPDATE sources SET selected = 0", [])?;
    if !selected_ids.is_empty() {
        let placeholders: Vec<String> = (0..selected_ids.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!("UPDATE sources SET selected = 1 WHERE id IN ({})", placeholders.join(", "));
        let params: Vec<&dyn rusqlite::types::ToSql> = selected_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
        tx.execute(&sql, params.as_slice())?;
    }
    tx.commit()?;
    Ok(())
}
```

**Step 3:** Add `use rusqlite::Transaction;` or relevant import if not already present.
**Step 4:** Run: `cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant`
**Step 5:** Commit: `fix(db): wrap replace_models and set_selected_sources in SQL transactions`

---

## Phase 2: Data Integrity & Error Handling (4 tasks)

### Task 2.1: Make AppDb and NotebookDb `conn` field private

**Objective:** Prevent direct SQLite access that bypasses transaction safety.

**Files:**
- Modify: `src-tauri/src/db/app_db.rs:9`
- Modify: `src-tauri/src/db/notebook_db/mod.rs:11`
- Modify: all files that access `.conn` directly

**Step 1:** Change `pub conn: Connection` to `conn: Connection` in both structs.
**Step 2:** Find all external accesses to `.conn` (the `with_notebook_db` closure pattern passes `&NotebookDb`, so any `db.conn.` call needs to go through a method): `grep -rn '\.conn\.' src-tauri/src/`
**Step 3:** For any external caller that needs raw conn access, add a `with_conn()` method that takes a closure:
```rust
pub fn with_conn<F, T>(&self, f: F) -> Result<T, GlossError>
where F: FnOnce(&Connection) -> Result<T, GlossError> {
    f(&self.conn)
}
```
But prefer adding proper typed methods for each operation instead.

**Step 4:** Run: `cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant`
**Step 5:** Commit: `refactor(db): make conn private, enforce access through methods`

---

### Task 2.2: Remove dead api_key parameter from update_provider

**Objective:** Eliminate confusing dead code.

**Files:**
- Modify: `src-tauri/src/db/app_db.rs:180-194`

**Step 1:** Remove `api_key: Option<&str>` parameter and the `let _ = api_key;` line:
```rust
pub fn update_provider(&self, id: &str, enabled: bool, base_url: Option<&str>) -> Result<(), GlossError> {
    self.conn.execute(
        "INSERT OR REPLACE INTO providers (id, enabled, base_url) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, enabled, base_url],
    )?;
    Ok(())
}
```

**Step 2:** Update all callers of `update_provider` to remove the `api_key` argument.
**Step 3:** Commit: `refactor(db): remove dead api_key parameter from update_provider`

---

### Task 2.3: Handle empty-string API keys explicitly in provider creation

**Objective:** Prevent confusing 401 errors from empty-but-present API keys.

**Files:**
- Modify: `src-tauri/src/providers/mod.rs:368,383`

**Step 1:** Replace `unwrap_or_default()` with explicit None handling:
```rust
let key = match secret_store.get("openai_api_key")? {
    Some(k) if !k.is_empty() => k,
    Some(_) => {
        tracing::warn!("OpenAI API key exists but is empty — treating as absent");
        String::new()
    }
    None => String::new(),
};
```
**Step 2:** Commit: `fix(providers): handle empty-string API keys explicitly`

---

### Task 2.4: Replace silent error swallowing in TypeScript stores and StatusBar

**Objective:** Make backend errors visible to users instead of invisible stale state.

**Files:**
- Modify: `src/stores/chatStore.ts` (multiple catch blocks)
- Modify: `src/stores/settingsStore.ts` (5+ catch blocks)
- Modify: `src/stores/sourceStore.ts` (15+ catch blocks)
- Modify: `src/components/layout/StatusBar.tsx:96-97`

**Step 1:** In each store's catch blocks, use the toast system to surface errors:
```typescript
} catch (e) {
    console.error('Failed to load conversations:', e);
    useToastStore.getState().addToast('Failed to load conversations', 'error');
}
```

**Step 2:** In StatusBar.tsx, replace empty catches:
```typescript
api.getQueueStatus().then(setQueueStatus).catch((e) => {
    console.warn('Queue status unavailable:', e);
    setQueueStatus(null);
});
api.memoryBackendStatus(activeNotebookId).then(setMemoryStatus).catch((e) => {
    console.warn('Memory backend status unavailable:', e);
    setMemoryStatus({ available: false, reason: 'status check failed' });
});
```

**Step 3:** Commit: `fix(ui): surface backend errors instead of silently swallowing`

---

## Phase 3: Security Hardening (3 tasks)

### Task 3.1: Set restrictive file permissions on Windows for secret store

**Objective:** Prevent any-process-can-read on Windows.

**Files:**
- Modify: `src-tauri/src/provider_config_store.rs:204-211`

**Step 1:** Implement Windows file permissions using `std::os::windows::fs::OpenOptionsExt` and `winapi` or use the `windows` crate:
```rust
#[cfg(windows)]
fn set_owner_only_dir_permissions(path: &Path) -> Result<(), GlossError> {
    // On Windows, set the file to be readable only by the current user
    // using Windows ACLs. Use icacls or the windows crate.
    use std::process::Command;
    let output = Command::new("icacls")
        .arg(path.to_str().unwrap_or(""))
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{}:(R)", whoami::username()))
        .output()
        .map_err(|e| GlossError::Io(e.to_string()))?;
    if !output.status.success() {
        tracing::warn!("Failed to set restrictive permissions on {:?}: {:?}", path, output.stderr);
    }
    Ok(())
}
```
Note: This adds a dependency on `whoami` crate or uses `std::env::var("USERNAME")`. If adding a crate is too heavy, use the `USERNAME` env var.

**Step 2:** Commit: `fix(security): restrict secret store file permissions on Windows`

---

### Task 3.2: Zero API key strings on Drop for provider structs

**Objective:** Prevent API key leakage in core dumps.

**Files:**
- Modify: `src-tauri/src/providers/openai.rs`
- Modify: `src-tauri/src/providers/anthropic.rs`

**Step 1:** Add Drop impls that zero the api_key field:
```rust
impl Drop for OpenAIProvider {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}
```
This requires the `zeroize` crate. Add to Cargo.toml:
```toml
zeroize = { version = "1", features = ["derive"] }
```

**Step 2:** Same for `AnthropicProvider`.
**Step 3:** Commit: `fix(security): zero API key strings on provider Drop`

---

### Task 3.3: Fix token redaction for keys embedded in JSON

**Objective:** Close the redaction gap where `{"key":"sk-..."}` isn't caught.

**Files:**
- Modify: `src-tauri/src/redaction.rs`
- Modify: `src-tauri/src/providers/mod.rs:216-250`

**Step 1:** Add JSON-aware redaction that matches `sk-` (and other prefixes) inside quoted strings:
```rust
fn redact_embedded_secrets(text: &str) -> String {
    let re = regex::Regex::new(r#""[^"]*\b(sk-|key-|gl-)[A-Za-z0-9_-]{20,}[^"]*""#).unwrap();
    re.replace_all(text, r#""[REDACTED]""#).to_string()
}
```

**Step 2:** Call this after the existing whitespace-split redaction in `sanitize_provider_error_body`.
**Step 3:** Commit: `fix(security): redact API keys embedded in JSON strings`

---

## Phase 4: Chat Lifecycle & Frontend Quality (4 tasks)

### Task 4.1: Fix createConversation race condition

**Objective:** Prevent stale conversation list when switching notebooks mid-await.

**Files:**
- Modify: `src/stores/chatStore.ts:71-78`

**Step 1:** Guard the full sequence, not just the final state set:
```typescript
createConversation: async (notebookId) => {
    const id = await api.createConversation(notebookId);
    if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) return id;
    await get().loadConversations(notebookId);
    if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) return id;
    set({ activeConversationId: id, messages: [] });
    return id;
},
```

**Step 2:** Commit: `fix(chat): guard createConversation against notebook switch race`

---

### Task 4.2: Add Tauri event listener cleanup on unmount

**Objective:** Prevent memory leaks from leaked event listeners.

**Files:**
- Modify: `src/App.tsx`

**Step 1:** Store unlisten functions and call them in the useEffect cleanup:
```typescript
useEffect(() => {
    const unlisteners: Promise<VoidFunction>[] = [];
    
    unlisteners.push(listen('chat:token', (e) => { /* ... */ }));
    // ... all other listen() calls ...
    
    return () => {
        unlisteners.forEach(p => p.then(fn => fn()));
    };
}, []);
```

**Step 2:** Commit: `fix(ui): clean up Tauri event listeners on App unmount`

---

### Task 4.3: Notify user when retrieval degrades silently during ingestion

**Objective:** Stop silent degradation of vector search quality.

**Files:**
- Modify: `src-tauri/src/state.rs:621-626` (backend)
- Modify: `src/components/chat/ChatPanel.tsx` (frontend indicator)

**Step 1:** When `embedder.try_lock().ok()` returns None, emit a status event:
```rust
let embedder_guard = self.embedder.try_lock().ok();
let embedder = embedder_guard.as_ref().and_then(|guard| guard.as_ref());
if embedder.is_none() {
    let _ = handle.emit("retrieval:degraded", serde_json::json!({
        "reason": "embedder_busy",
        "fallback": "bm25_only"
    }));
}
```

**Step 2:** In ChatPanel, listen for this event and show a subtle indicator.
**Step 3:** Commit: `feat(chat): notify user when retrieval degrades to BM25-only`

---

### Task 4.4: Fix scroll-to-bottom performance during streaming

**Objective:** Prevent animation frame queue buildup.

**Files:**
- Modify: `src/components/chat/ChatPanel.tsx:67-69`

**Step 1:** Throttle scroll via ref timestamp:
```typescript
const lastScrollRef = useRef(0);
useEffect(() => {
    const now = Date.now();
    if (now - lastScrollRef.current < 100) return;
    lastScrollRef.current = now;
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
}, [messages, streamingContent]);
```

**Step 2:** Commit: `perf(chat): throttle scroll-to-bottom during streaming`

---

## Phase 5: Test Coverage (3 tasks)

### Task 5.1: Add frontend contract tests for chatStore

**Objective:** Cover the critical chat lifecycle (send -> stream -> finalize -> error -> stop).

**Files:**
- Create: `src/stores/__tests__/chatStore.test.ts`
- Modify: `package.json` (add vitest devDep if needed)

**Step 1:** Install vitest: `npm install --save-dev vitest`
**Step 2:** Add test script to package.json: `"test:unit": "vitest run"`
**Step 3:** Write tests for:
- `createConversation` guard behavior
- `finalizeMessage` with empty content
- `stopStreaming` sets error state
- `onChatToken` accumulates streaming content
- `onChatError` sets streamingError
- notebook switch during active stream

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useChatStore } from '../chatStore';

describe('chatStore', () => {
    beforeEach(() => {
        useChatStore.setState({
            messages: [],
            streamingContent: '',
            isStreaming: false,
            streamingError: null,
            conversations: [],
            activeConversationId: null,
        });
    });

    it('finalizeMessage bails on empty streamingContent', () => {
        useChatStore.getState().finalizeMessage('nb1', 'conv1', 'msg1');
        expect(useChatStore.getState().messages).toHaveLength(0);
    });

    it('stopStreaming sets streamingError with partial content', () => {
        useChatStore.setState({ streamingContent: 'partial text', isStreaming: true });
        // simulate stop
        useChatStore.getState().stopStreaming('nb1');
        expect(useChatStore.getState().isStreaming).toBe(false);
        expect(useChatStream.getState().streamingError).toContain('Partial output');
    });
});
```

**Step 4:** Run: `npm run test:unit`
**Step 5:** Commit: `test(chat): add chatStore contract tests`

---

### Task 5.2: Add frontend contract tests for sourceStore

**Objective:** Cover source degradation and selection logic.

**Files:**
- Create: `src/stores/__tests__/sourceStore.test.ts`

**Step 1:** Write tests for:
- Source list loading states (loading, partial, error, loaded)
- Selected source tracking
- Error state propagation

**Step 2:** Commit: `test(sources): add sourceStore contract tests`

---

### Task 5.3: Add backend unit tests for transaction safety

**Objective:** Prove replace_models and set_selected_sources are atomic.

**Files:**
- Modify: `src-tauri/src/db/app_db.rs` (add test module)
- Modify: `src-tauri/src/db/notebook_db/mod.rs` (add test module)

**Step 1:** Add tests that verify rollback on failure:
```rust
#[cfg(test)]
mod transaction_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn replace_models_rollback_on_bad_insert() {
        // Create a model with invalid data that will fail on insert
        // Verify the original models are still present after the error
    }

    #[test]
    fn set_selected_sources_atomicity() {
        // Set some sources selected, then call set_selected_sources with data that fails
        // Verify original selection state is preserved
    }
}
```

**Step 2:** Commit: `test(db): add transaction atomicity tests`

---

## Phase 6: Architecture Cleanup (3 tasks)

### Task 6.1: Decompose commands/chat/mod.rs (3,662 lines)

**Objective:** Make the chat command file maintainable by extracting submodules.

**Files:**
- Create: `src-tauri/src/commands/chat/streaming.rs` (streaming pipeline)
- Create: `src-tauri/src/commands/chat/gates.rs` (LLM/GPU gate acquisition)
- Create: `src-tauri/src/commands/chat/receipts.rs` (receipt persistence)
- Create: `src-tauri/src/commands/chat/emit.rs` (all emit_* functions)
- Modify: `src-tauri/src/commands/chat/mod.rs` (re-export from submodules)

**Step 1:** Move `emit_chat_done`, `emit_chat_error`, `emit_chat_status`, `emit_chat_evidence` to `emit.rs`
**Step 2:** Move gate acquisition logic (acquire_gpu_gate, acquire_llm_gate, gate_owner_for) to `gates.rs`
**Step 3:** Move receipt recording functions to `receipts.rs`
**Step 4:** Move the main streaming closure to `streaming.rs`
**Step 5:** Re-export from `mod.rs`: `mod emit; mod gates; mod receipts; mod streaming;`
**Step 6:** Run: `cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant`
**Step 7:** Commit: `refactor(chat): decompose 3.6K-line mod.rs into submodules`

---

### Task 6.2: Add NotebookDb connection pooling

**Objective:** Enable prepared statement caching and reduce per-call overhead.

**Files:**
- Modify: `src-tauri/src/state.rs` (with_notebook_db method)
- Create: `src-tauri/src/db/notebook_pool.rs`

**Step 1:** Create a simple connection pool:
```rust
use std::collections::HashMap;
use std::sync::Mutex;
use crate::db::notebook_db::NotebookDb;

pub struct NotebookDbPool {
    connections: Mutex<HashMap<String, NotebookDb>>,
}

impl NotebookDbPool {
    pub fn new() -> Self {
        Self { connections: Mutex::new(HashMap::new()) }
    }

    pub fn with_db<F, T>(&self, notebook_id: &str, db_path: &Path, f: F) -> Result<T, GlossError>
    where F: FnOnce(&NotebookDb) -> Result<T, GlossError> {
        let mut map = self.connections.lock().map_err(|e| GlossError::Io(e.to_string()))?;
        let db = map.entry(notebook_id.to_string())
            .or_insert_with(|| NotebookDb::connect(db_path).expect("db connect"))?;
        f(db)
    }
}
```

**Step 2:** Replace `with_notebook_db` in state.rs with pool-based version.
**Step 3:** Commit: `perf(db): add NotebookDb connection pooling`

---

### Task 6.3: Remove stale documentation artifacts from repo root

**Objective:** Clean up the 25+ numbered audit/policy/gate documents cluttering the repo root.

**Files:**
- Move: All `0x_*.md`, `0x_*.csv` files to `docs/archive/audit-v29/`
- Move: `PHASES/`, `PHASE_PROMPTS/` to `docs/archive/audit-v29/`
- Keep: `README.md`, `AGENTS.md`, `CLAUDE.md`, `SPEC-gloss.md`

**Step 1:** `mkdir -p docs/archive/audit-v29`
**Step 2:** Move numbered docs: `git mv 0* docs/archive/audit-v29/`
**Step 3:** Move phases: `git mv PHASES docs/archive/audit-v29/ && git mv 06_PHASE_PROMPTS docs/archive/audit-v29/`
**Step 4:** Commit: `chore: archive v29 audit documents to docs/archive/`

---

## Verification Gates

After all phases complete, run:

```bash
# Build
npm run build

# Frontend tests
npm run test:unit

# Rust tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant

# Clippy
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant -- -D warnings

# Format
cargo fmt --all -- --check

# Validation scripts
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 validation/validate_release_receipt_consistency.py .
```

All must pass with zero errors for release-grade.