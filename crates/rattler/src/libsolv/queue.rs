use crate::libsolv::ffi;
use std::marker::PhantomData;
use std::os::raw::c_int;

/// Wrapper for libsolv queue type. This type is used by libsolv in the solver to solve for
/// different conda matchspecs. This is a type-safe implementation that is coupled to a specific Id
/// type
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

    /// Returns the ffi::Queue as a const pointer
    pub fn as_inner_ptr(&self) -> *const ffi::Queue {
        &self.queue as *const ffi::Queue
    }
}

impl<T: Into<ffi::Id>> Queue<T> {
    /// Pushes a single id to the back of the queue
    pub fn push_id(&mut self, id: T) {
        unsafe {
            ffi::queue_insert(self.as_inner_mut(), self.queue.count, id.into());
        }
    }

    /// Push multiple id's into the queue
    pub fn push_id_and_flags(&mut self, id: T, flags: i32) {
        unsafe {
            ffi::queue_insert2(
                self.as_inner_mut(),
                self.queue.count,
                flags as c_int,
                id.into(),
            );
        }
    }
}

#[cfg(test)]
mod test {
    use crate::libsolv::pool::StringId;
    use crate::libsolv::queue::Queue;

    #[test]
    fn create_queue() {
        let _queue = Queue::<StringId>::default();
    }
}
