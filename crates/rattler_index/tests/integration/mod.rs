// Integration test modules
mod atomic_write;
mod basic_indexing;
mod cache_tests;
mod concurrent_indexing;
#[allow(dead_code)] // Unit and integration targets exercise different backend modes.
pub mod etag_memory_backend;
mod etag_memory_backend_tests;
mod indexed_timestamp;
