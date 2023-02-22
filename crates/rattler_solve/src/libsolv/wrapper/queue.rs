use super::ffi;
use super::flags::SolvableFlags;
use std::marker::PhantomData;
use std::os::raw::c_int;

/// Wrapper for queue, the queuing datastructure used by libsolv
///
/// The wrapper functions as an owned pointer, guaranteed to be non-null and freed
/// when the Queue is dropped. It also ensures that you always pass objects of the
/// same Id type to the queue.
pub struct Queue<T> {
    queue: ffi::Queue,
    // Makes this queue typesafe
    _data: PhantomData<T>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        let mut queue = ffi::Queue {
            elements: std::ptr::null_mut(),
            count: 0,
            alloc: std::ptr::null_mut(),
            left: 0,
        };

        // Create the queue
        unsafe { ffi::queue_init(&mut queue as *mut ffi::Queue) };

        Self {
            queue,
            _data: PhantomData,
        }
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        // Safe because we know that the pool is never freed manually
        unsafe {
            ffi::queue_free(self.raw_ptr());
        }
    }
}

impl<T> Queue<T> {
    /// Returns a raw pointer to the wrapped `ffi::Repo`, to be used for calling ffi functions
    /// that require access to the repo (and for nothing else)
    pub(super) fn raw_ptr(&mut self) -> *mut ffi::Queue {
        &mut self.queue as *mut ffi::Queue
    }
}

impl<T: Into<ffi::Id>> Queue<T> {
    /// Pushes a single id to the back of the queue
    pub fn push_id(&mut self, id: T) {
        unsafe {
            ffi::queue_insert(self.raw_ptr(), self.queue.count, id.into());
        }
    }

    /// Push an id and flag into the queue
    pub fn push_id_with_flags(&mut self, id: T, flags: SolvableFlags) {
        unsafe {
            ffi::queue_insert2(
                self.raw_ptr(),
                self.queue.count,
                flags.inner() as c_int,
                id.into(),
            );
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
