use super::ffi;
use std::marker::PhantomData;

/// Wrapper for libsolv queue type. This type is used by to gather items of a specific type. This
/// is a type-safe implementation that is coupled to a specific Id type.
pub struct Queue<T> {
    queue: ffi::Queue,
    // Makes this queue typesafe
    _data: PhantomData<T>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        // Safe because we know for a fact that the queue exists
        unsafe {
            // Create a queue pointer and initialize it
            let mut queue = ffi::Queue {
                elements: std::ptr::null_mut(),
                count: 0,
                alloc: std::ptr::null_mut(),
                left: 0,
            };
            // This initializes some internal libsolv stuff
            ffi::queue_init(&mut queue as *mut ffi::Queue);
            Self {
                queue,
                _data: PhantomData,
            }
        }
    }
}

/// This drop implementation drops the internal libsolv queue
impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        // Safe because this pointer exists
        unsafe {
            ffi::queue_free(self.as_inner_mut());
        }
    }
}

impl<T> Queue<T> {
    /// Returns the ffi::Queue as a mutable pointer
    pub fn as_inner_mut(&mut self) -> *mut ffi::Queue {
        &mut self.queue as *mut ffi::Queue
    }
}

impl<T: Into<ffi::Id>> Queue<T> {
    /// Pushes a single id to the back of the queue
    pub fn push_id(&mut self, id: T) {
        unsafe {
            ffi::queue_insert(self.as_inner_mut(), self.queue.count, id.into());
        }
    }
}

#[cfg(test)]
mod test {
    use super::{super::pool::StringId, Queue};

    #[test]
    fn create_queue() {
        let _queue = Queue::<StringId>::default();
    }
}
