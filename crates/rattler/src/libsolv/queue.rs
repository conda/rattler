use crate::libsolv::ffi;

/// Wrapper for libsolv queue type. This type is used by libsolv in the solver
/// to solve for different conda matchspecs
pub struct Queue(ffi::Queue);

impl Default for Queue {
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
            Self(queue)
        }
    }
}

impl Queue {
    /// Returns the ffi::Queue as a mutable pointer
    pub fn as_inner_mut(&mut self) -> *mut ffi::Queue {
        &mut self.0 as *mut ffi::Queue
    }

    /// Returns the ffi::Queue as a const pointer
    pub fn as_inner_ptr(&self) -> *const ffi::Queue {
        &self.0 as *const ffi::Queue
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        // Safe because this pointer exists
        unsafe {
            ffi::queue_free(self.as_inner_mut());
        }
    }
}

#[cfg(test)]
mod test {
    use crate::libsolv::queue::Queue;

    #[test]
    fn create_queue() {
        let queue = Queue::default();
    }
}
