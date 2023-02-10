use super::InstallError;
use futures::StreamExt;
use std::sync::Arc;
use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedSender},
    sync::{oneshot, Mutex},
    task::JoinHandle,
};

/// Packages can mostly be installed in isolation and therefor in parallel. However, when installing
/// a large number of packages at the same time the different installation tasks start competing for
/// resources. The [`InstallDriver`] helps to assist in making sure that tasks dont starve
/// each other from resource as well as making sure that due to the large number of requests the
/// process doesnt try to acquire more resources than the system has available.
pub struct InstallDriver {
    inner: Arc<Mutex<InstallDriverInner>>,
}

struct InstallDriverInner {
    tx: UnboundedSender<Task>,
    join_handle: JoinHandle<()>,
}

type Task = Box<dyn FnOnce() + Send + 'static>;

impl Default for InstallDriver {
    fn default() -> Self {
        Self::new(100)
    }
}

impl InstallDriver {
    /// Constructs a new [`InstallDriver`] with a given maximum number of concurrent tasks. This is
    /// the number of tasks spawned through the driver that can run concurrently. This is especially
    /// useful to make sure no filesystem limits are encountered.
    pub fn new(concurrency_limit: usize) -> Self {
        let (tx, rx) = unbounded_channel::<Task>();
        let task_stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        let join_handle = tokio::spawn(task_stream.for_each_concurrent(
            Some(concurrency_limit),
            |task| async move {
                if let Err(err) = tokio::task::spawn_blocking(task).await {
                    // If a panic occurred in the blocking task we resume the error here to make sure
                    // its not getting lost.
                    if let Ok(panic) = err.try_into_panic() {
                        std::panic::resume_unwind(panic);
                    }

                    // Note: we dont handle the cancelled error here. This can be handled by a
                    // sender/receiver pair that get closed when the task drops.
                }
            },
        ));
        Self {
            inner: Arc::new(Mutex::new(InstallDriverInner { tx, join_handle })),
        }
    }

    /// Spawns a blocking operation on another thread and waits for it to complete. This is similar
    /// to calling [`tokio::task::spawn_blocking`] except that the number of concurrent tasks is
    /// limited. This is especially useful when performing filesystem operations because most
    /// platforms have a limit on the number of concurrent filesystem operations.
    pub async fn spawn_throttled<
        R: Send + 'static,
        F: FnOnce() -> Result<R, InstallError> + Send + 'static,
    >(
        &self,
        f: F,
    ) -> Result<R, InstallError> {
        let (tx, rx) = oneshot::channel();
        {
            let inner = self.inner.lock().await;
            if inner
                .tx
                .send(Box::new(move || {
                    if !tx.is_closed() {
                        let result = f();
                        let _ = tx.send(result);
                    }
                }))
                .is_err()
            {
                unreachable!(
                    "if a send error occurs here it means the task processor is dropped. \
                Since this only happens when dropping this object there cannot be another call to \
                this function. Therefor this should never happen."
                );
            }
        }

        // Await the result being send back. If an error occurs during receive it means that the
        // sending end of the channel was closed. This can only really happen when the task has been
        // dropped. We assume that that means the task has been cancelled.
        rx.await.map_err(|_| InstallError::Cancelled)?
    }
}

impl Drop for InstallDriverInner {
    fn drop(&mut self) {
        self.join_handle.abort()
    }
}
