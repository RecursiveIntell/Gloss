//! A deadline requests cancellation; it never detaches work from its owner.
use std::time::Duration;
use tokio::task::{JoinError, JoinHandle};

/// The caller must keep inference permits alive until this returns. Some jobs
/// perform blocking work that cannot be aborted, so wait for actual completion
/// after requesting cooperative cancellation and let the queue persist status.
pub async fn join_with_cancellation<T>(
    mut task: JoinHandle<T>,
    deadline: Duration,
    cancel: impl FnOnce(),
) -> Result<T, JoinError> {
    match tokio::time::timeout(deadline, &mut task).await {
        Ok(result) => result,
        Err(_) => {
            cancel();
            task.await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{oneshot, Semaphore};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deadline_cannot_release_permits_while_blocking_work_continues() {
        let gpu = Arc::new(Semaphore::new(1));
        let llm = Arc::new(Semaphore::new(1));
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let worker = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                42
            })
            .await
            .unwrap()
        });
        started_rx.await.unwrap();
        let gpu_owner = gpu.clone();
        let llm_owner = llm.clone();
        let supervisor = tokio::spawn(async move {
            let _gpu = gpu_owner.acquire().await.unwrap();
            let _llm = llm_owner.acquire().await.unwrap();
            join_with_cancellation(worker, Duration::from_millis(10), || {
                cancel_tx.send(()).unwrap();
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), cancel_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(gpu.try_acquire().is_err());
        assert!(llm.try_acquire().is_err());
        assert!(!supervisor.is_finished());
        finish_tx.send(()).unwrap();
        assert_eq!(supervisor.await.unwrap().unwrap(), 42);
        assert_eq!(gpu.available_permits(), 1);
        assert_eq!(llm.available_permits(), 1);
    }

    #[tokio::test]
    async fn panic_is_reported_without_killing_the_owner() {
        let task = tokio::spawn(async { panic!("fixture job panic") });
        let failure = join_with_cancellation(task, Duration::from_secs(1), || {
            panic!("a completed panic must not request cancellation")
        })
        .await;
        assert!(failure.unwrap_err().is_panic());
        assert_eq!(
            join_with_cancellation(tokio::spawn(async { 7 }), Duration::from_secs(1), || {})
                .await
                .unwrap(),
            7
        );
    }
}
