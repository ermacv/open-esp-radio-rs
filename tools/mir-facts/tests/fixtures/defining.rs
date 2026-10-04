#![no_std]
pub mod buf {
    pub struct Header(pub u32);
}
pub mod api {
    pub trait Driver {
        fn send(&self, len: usize) -> usize;
    }
}
pub struct Pool {
    pub release: fn(core::ptr::NonNull<buf::Header>, usize),
}
fn release(_: core::ptr::NonNull<buf::Header>, _: usize) {}
pub fn pool() -> Pool {
    Pool { release }
}
pub struct Loopback;
impl api::Driver for Loopback {
    fn send(&self, len: usize) -> usize {
        len
    }
}
pub fn driver() -> &'static dyn api::Driver {
    &Loopback
}
