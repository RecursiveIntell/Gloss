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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn acquire_gate_with_epoch<'a>(
    app_handle: &tauri::AppHandle,
    state: &'a AppState,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    epoch: u64,
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
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            emit_chat_status(
                app_handle,
                notebook_id,
                conversation_id,
                message_id,
                "cancelled",
                "Chat cancelled by notebook switch",
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
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(TryAcquireError::Closed) => {
                return Err(GlossError::Other(format!("{gate_name} closed")));
            }
        }
    }
}
