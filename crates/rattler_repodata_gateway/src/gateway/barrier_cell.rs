use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};
use thiserror::Error;
use tokio::sync::Notify;

/// A synchronization primitive that can be used to wait for a value to become available.
///
/// The [`BarrierCell`] is initially empty, requesters can wait for a value to become available
/// using the `wait` method. Once a value is available, the `set` method can be used to set the
/// value in the cell. The `set` method can only be called once. If the `set` method is called
/// multiple times, it will return an error. When `set` is called all waiters will be notified.
pub struct BarrierCell<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
    notify: Notify,
}

impl<T> Drop for BarrierCell<T> {
    fn drop(&mut self) {
        if self.state.load(Ordering::Acquire) == BarrierCellState::Initialized as u8 {
            unsafe { self.value.get_mut().assume_init_drop() }
        }
    }
}

unsafe impl<T: Sync> Sync for BarrierCell<T> {}

unsafe impl<T: Send> Send for BarrierCell<T> {}

#[repr(u8)]
enum BarrierCellState {
    Uninitialized,
    Initializing,
    Initialized,
}

impl<T> Default for BarrierCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Error)]
pub enum SetError {
    #[error("cannot assign a BarrierCell twice")]
    AlreadySet,
}

impl<T> BarrierCell<T> {
    /// Constructs a new instance.
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(BarrierCellState::Uninitialized as u8),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            notify: Notify::new(),
        }
    }

    /// Wait for a value to become available in the cell
    pub async fn wait(&self) -> &T {
        let notified = self.notify.notified();
        if self.state.load(Ordering::Acquire) != BarrierCellState::Initialized as u8 {
            notified.await;
        }
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    /// Set the value in the cell, if the cell was already initialized this will return an error.
    pub fn set(&self, value: T) -> Result<(), SetError> {
        let state = self
            .state
            .fetch_max(BarrierCellState::Initializing as u8, Ordering::SeqCst);

        // If the state is larger than started writing, then either there is an active writer or
        // the cell has already been initialized.
        if state == BarrierCellState::Initialized as u8 {
            return Err(SetError::AlreadySet);
        } else {
            unsafe { *self.value.get() = MaybeUninit::new(value) };
            self.state
                .store(BarrierCellState::Initialized as u8, Ordering::Release);

            self.notify.notify_waiters();
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::BarrierCell;
    use std::sync::Arc;

    /// Test that setting the barrier cell works, and we can wait on the value
    #[tokio::test]
    pub async fn test_barrier_cell() {
        let barrier = Arc::new(BarrierCell::new());
        let barrier_clone = barrier.clone();

        let handle = tokio::spawn(async move {
            let value = barrier_clone.wait().await;
            assert_eq!(*value, 42);
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        barrier.set(42).unwrap();
        handle.await.unwrap();
    }

    /// Test that we cannot set the barrier cell twice
    #[tokio::test]
    pub async fn test_barrier_cell_set_twice() {
        let barrier = Arc::new(BarrierCell::new());
        barrier.set(42).unwrap();
        assert!(barrier.set(42).is_err());
    }

    #[test]
    pub fn test_drop() {
        let barrier = BarrierCell::new();
        let arc = Arc::new(42);
        barrier.set(arc.clone()).unwrap();
        assert_eq!(Arc::strong_count(&arc), 2);
        drop(barrier);
        assert_eq!(Arc::strong_count(&arc), 1);
    }
}
