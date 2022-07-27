use crate::libsolv::ffi;
use std::ptr::NonNull;

/// Wrapper for libsolv queue type. This type is used by libsolv in the solver
/// to solve for different conda matchspecs
pub struct Queue(NonNull<ffi::Queue>);

impl Default for Queue {
    fn default() -> Self {
        unsafe {
            // Create a queue pointer and initialize it
            let queue = Box::new(ffi::Queue {
                elements: std::ptr::null_mut(),
                count: 0,
                alloc: std::ptr::null_mut(),
                left: 0,
            });
            let queue = Box::into_raw(queue);
            ffi::queue_init(queue);
            Self(NonNull::new(queue).expect("returned pointer is null"))
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        // Safe because this pointer exists
        unsafe {
            ffi::queue_free(self.0.as_mut());
            drop(Box::from_raw(self.0.as_mut()));
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
