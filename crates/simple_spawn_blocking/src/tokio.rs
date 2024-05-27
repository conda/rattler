use crate::Cancelled;
use tokio::task::JoinError;

/// Run a blocking task to completion. If the task is cancelled, the function
/// will return an error converted from `Error`.
///
/// Any panic that occurs in the blocking task will be propagated.
pub async fn run_blocking_task<T, E, F>(f: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<Cancelled> + Send + 'static,
{
    match tokio::task::spawn_blocking(f)
        .await
        .map_err(JoinError::try_into_panic)
    {
        Ok(result) => result,
        Err(Err(_err)) => Err(E::from(Cancelled)),
        Err(Ok(payload)) => std::panic::resume_unwind(payload),
    }
}
