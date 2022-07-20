pub mod ffi {
    #![allow(warnings)]
    include!(concat!(env!("OUT_DIR"), "/libsolv_bindings.rs"));
}
