use crate::error::GlossError;
use crate::state::AppState;
use std::time::{Duration, Instant};
use tokio::sync::TryAcquireError;

use super::emit::emit_chat_status;

pub(crate) fn gate_owner_for(
    state: &AppState,
    gate_name: &str,
) -> Option<crate::state::RuntimeGateOwner> {
    state
        .gate_owners_snapshot()
        .into_iter()
        .find(|owner| owner.gate == gate_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn gate_wait_preserves_notebook_timeout_closed_and_pre_cancelled_reasons() {
        let data_dir = tempfile::tempdir().expect("temporary chat gate state");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        state.set_active_notebook(Some("notebook".to_string()), Some(7));
        let app = tauri::test::mock_app();
        app.manage(state);
        let state = app.state::<AppState>();
        let gate = tokio::sync::Semaphore::new(1);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = acquire_gate_with_epoch(
            app.handle(),
            &state,
            "notebook",
            "conversation",
            "pre-cancelled",
            7,
            &cancellation,
            &gate,
            Duration::from_secs(120),
            "GPU gate",
        )
        .await
        .expect("pre-cancelled gate wait has a cancellation result");
        assert!(result.is_none());
        assert_eq!(gate.available_permits(), 1);
        assert!(state.gate_owners_snapshot().is_empty());
        assert!(state
            .chat_events_since("notebook", "conversation", None)
            .iter()
            .any(|event| event.payload["message"] == "Chat cancelled by user request"));

        let cancellation = CancellationToken::new();
        let result = acquire_gate_with_epoch(
            app.handle(),
            &state,
            "notebook",
            "conversation",
            "notebook-switched",
            6,
            &cancellation,
            &gate,
            Duration::from_secs(120),
            "GPU gate",
        )
        .await
        .expect("stale notebook epoch has a cancellation result");
        assert!(result.is_none());
        assert!(state
            .chat_events_since("notebook", "conversation", None)
            .iter()
            .any(|event| event.payload["message"] == "Chat cancelled by notebook switch"));

        let held = gate.acquire().await.expect("held fixture permit");
        let error = acquire_gate_with_epoch(
            app.handle(),
            &state,
            "notebook",
            "conversation",
            "timeout",
            7,
            &cancellation,
            &gate,
            Duration::ZERO,
            "GPU gate",
        )
        .await
        .expect_err("held gate must time out at its deadline");
        assert!(error
            .to_string()
            .contains("Timed out waiting for GPU gate."));
        assert!(state
            .chat_events_since("notebook", "conversation", None)
            .iter()
            .any(|event| event.payload["phase"] == "gate_timeout"));
        gate.close();
        let error = acquire_gate_with_epoch(
            app.handle(),
            &state,
            "notebook",
            "conversation",
            "closed",
            7,
            &cancellation,
            &gate,
            Duration::from_secs(120),
            "GPU gate",
        )
        .await
        .expect_err("closed gate must retain its closed error");
        assert!(error.to_string().contains("GPU gate closed"));
        assert!(!cancellation.is_cancelled());
        drop(held);
        assert!(state.gate_owners_snapshot().is_empty());
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn acquire_gate_with_epoch<'a, R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    state: &'a AppState,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    epoch: u64,
    cancellation: &tokio_util::sync::CancellationToken,
    gate: &'a tokio::sync::Semaphore,
    timeout: Duration,
    gate_name: &str,
) -> Result<Option<tokio::sync::SemaphorePermit<'a>>, GlossError> {
    let started = Instant::now();
    let mut last_wait_status = Instant::now() - Duration::from_secs(2);
    let owner = gate_owner_for(state, gate_name);
    emit_chat_status(
        app_handle,
        notebook_id,
        conversation_id,
        message_id,
        "waiting_gate",
        "Waiting for runtime gate",
        None,
        None,
        Some(gate_name),
        owner.as_ref().map(|owner| owner.owner.as_str()),
        owner.as_ref().map(|owner| owner.detail.as_str()),
        started.elapsed(),
        Some(timeout),
        false,
        None,
    );

    loop {
        let notebook_changed = !state.is_active_notebook_epoch(notebook_id, epoch);
        if notebook_changed || cancellation.is_cancelled() {
            emit_chat_status(
                app_handle,
                notebook_id,
                conversation_id,
                message_id,
                "cancelled",
                if notebook_changed {
                    "Chat cancelled by notebook switch"
                } else {
                    "Chat cancelled by user request"
                },
                None,
                None,
                Some(gate_name),
                None,
                None,
                started.elapsed(),
                Some(timeout),
                false,
                None,
            );
            return Ok(None);
        }

        match gate.try_acquire() {
            Ok(permit) => {
                state.set_gate_owner(gate_name, "chat", message_id);
                emit_chat_status(
                    app_handle,
                    notebook_id,
                    conversation_id,
                    message_id,
                    "gate_acquired",
                    "Runtime gate acquired",
                    None,
                    None,
                    Some(gate_name),
                    Some("chat"),
                    Some(message_id),
                    started.elapsed(),
                    Some(timeout),
                    false,
                    None,
                );
                return Ok(Some(permit));
            }
            Err(TryAcquireError::NoPermits) => {
                if started.elapsed() >= timeout {
                    let error = format!("Timed out waiting for {gate_name}.");
                    let owner = gate_owner_for(state, gate_name);
                    emit_chat_status(
                        app_handle,
                        notebook_id,
                        conversation_id,
                        message_id,
                        "gate_timeout",
                        "Timed out waiting for runtime gate",
                        None,
                        None,
                        Some(gate_name),
                        owner.as_ref().map(|owner| owner.owner.as_str()),
                        owner.as_ref().map(|owner| owner.detail.as_str()),
                        started.elapsed(),
                        Some(timeout),
                        false,
                        Some(&error),
                    );
                    return Err(GlossError::Other(error));
                }
                if last_wait_status.elapsed() >= Duration::from_secs(1) {
                    let owner = gate_owner_for(state, gate_name);
                    emit_chat_status(
                        app_handle,
                        notebook_id,
                        conversation_id,
                        message_id,
                        "waiting_gate",
                        "Waiting for runtime gate",
                        None,
                        None,
                        Some(gate_name),
                        owner.as_ref().map(|owner| owner.owner.as_str()),
                        owner.as_ref().map(|owner| owner.detail.as_str()),
                        started.elapsed(),
                        Some(timeout),
                        false,
                        None,
                    );
                    last_wait_status = Instant::now();
                }
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {},
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                }
            }
            Err(TryAcquireError::Closed) => {
                return Err(GlossError::Other(format!("{gate_name} closed")));
            }
        }
    }
}
