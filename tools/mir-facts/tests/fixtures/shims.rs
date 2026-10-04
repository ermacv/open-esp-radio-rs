#![no_std]
extern crate alloc;
use alloc::boxed::Box;

pub trait Job {
    fn run(&self) -> u32;
}
pub struct A(u32);
impl Job for A {
    fn run(&self) -> u32 {
        self.0
    }
}
impl Drop for A {
    fn drop(&mut self) {
        core::hint::black_box(self.0);
    }
}

fn seven() -> u32 {
    7
}

#[inline(never)]
fn call_twice<F: FnMut() -> u32>(mut f: F) -> u32 {
    f() + f()
}

#[inline(never)]
pub fn through_shim() -> u32 {
    let pointer: fn() -> u32 = seven;
    call_twice(pointer)
}

#[inline(never)]
pub fn drop_boxed(job: Box<dyn Job>) {
    drop(job)
}

pub fn roots() {
    drop_boxed(Box::new(A(3)));
}
