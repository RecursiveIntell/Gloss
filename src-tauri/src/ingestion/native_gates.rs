//! Embedding calls use the application's existing GPU and LLM gates.
use crate::error::GlossError;
use tokio::sync::{Semaphore, SemaphorePermit};

pub struct NativeInferenceGuard<'a> {
    _gpu: SemaphorePermit<'a>,
    _llm: SemaphorePermit<'a>,
}

pub async fn acquire<'a>(
    gpu: &'a Semaphore,
    llm: &'a Semaphore,
) -> Result<NativeInferenceGuard<'a>, GlossError> {
    tokio::time::timeout(std::time::Duration::from_secs(300), async {
        let gpu = gpu
            .acquire()
            .await
            .map_err(|_| GlossError::Embedding("GPU inference gate closed".into()))?;
        let llm = llm
            .acquire()
            .await
            .map_err(|_| GlossError::Embedding("LLM inference gate closed".into()))?;
        Ok(NativeInferenceGuard {
            _gpu: gpu,
            _llm: llm,
        })
    })
    .await
    .map_err(|_| {
        GlossError::Embedding(
            "Embedding waited 300 seconds for active inference to finish; retry required".into(),
        )
    })?
}

pub fn acquire_blocking<'a>(
    gpu: &'a Semaphore,
    llm: &'a Semaphore,
) -> Result<NativeInferenceGuard<'a>, GlossError> {
    let future = acquire(gpu, llm);
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Ok(_) => Err(GlossError::Embedding(
            "Blocking ingestion requires a worker thread".into(),
        )),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| GlossError::Embedding(error.to_string()))?
            .block_on(future),
    }
}

pub fn try_acquire<'a>(
    gpu: &'a Semaphore,
    llm: &'a Semaphore,
) -> Result<NativeInferenceGuard<'a>, GlossError> {
    let busy = |_| {
        GlossError::Embedding(
            "Embedding inference is busy; retry when the active request finishes".into(),
        )
    };
    let gpu = gpu.try_acquire().map_err(busy)?;
    let llm = llm.try_acquire().map_err(busy)?;
    Ok(NativeInferenceGuard {
        _gpu: gpu,
        _llm: llm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_inference_cannot_overlap_existing_work_and_releases_on_failure() {
        let gpu = Semaphore::new(1);
        let llm = Semaphore::new(1);
        let active = try_acquire(&gpu, &llm).unwrap();
        assert!(try_acquire(&gpu, &llm).is_err());
        assert!(gpu.try_acquire().is_err());
        assert!(llm.try_acquire().is_err());
        drop(active);
        let other_llm = llm.try_acquire().unwrap();
        assert!(try_acquire(&gpu, &llm).is_err());
        assert!(gpu.try_acquire().is_ok());
        drop(other_llm);
        assert!(try_acquire(&gpu, &llm).is_ok());
    }

    #[tokio::test]
    async fn ingestion_waits_until_active_work_releases_both_gates() {
        let gpu = Semaphore::new(1);
        let llm = Semaphore::new(1);
        let active = try_acquire(&gpu, &llm).unwrap();
        let pending = acquire(&gpu, &llm);
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut pending)
                .await
                .is_err()
        );
        drop(active);
        let next = pending.await.unwrap();
        assert!(try_acquire(&gpu, &llm).is_err());
        drop(next);
        assert!(try_acquire(&gpu, &llm).is_ok());
    }

    #[tokio::test]
    async fn timed_out_retrieval_future_releases_its_owned_permits() {
        let gpu = Semaphore::new(1);
        let llm = Semaphore::new(1);
        let retrieval = async {
            let _guard = acquire(&gpu, &llm).await.unwrap();
            std::future::pending::<()>().await;
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), retrieval)
                .await
                .is_err()
        );
        assert!(try_acquire(&gpu, &llm).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_rebuild_caller_does_not_release_running_worker_permits() {
        let gpu = std::sync::Arc::new(Semaphore::new(1));
        let llm = std::sync::Arc::new(Semaphore::new(1));
        let worker_gpu = gpu.clone();
        let worker_llm = llm.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let guard = acquire_blocking(&worker_gpu, &worker_llm).unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
            finished_tx.send(()).unwrap();
        });
        started_rx.await.unwrap();
        drop(worker);
        assert!(try_acquire(&gpu, &llm).is_err());
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
        assert!(try_acquire(&gpu, &llm).is_ok());
    }
}
