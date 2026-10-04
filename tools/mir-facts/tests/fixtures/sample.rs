#![no_std]
pub trait Speak {
    fn speak(&self, x: u32) -> u32;
}
pub struct A;
pub struct B(u32);
impl Speak for A {
    fn speak(&self, x: u32) -> u32 {
        x + 1
    }
}
impl Speak for B {
    fn speak(&self, x: u32) -> u32 {
        x * self.0
    }
}
#[inline(never)]
pub fn call_dyn(s: &dyn Speak, x: u32) -> u32 {
    s.speak(x)
}
pub struct Hook {
    pub release: fn(u32) -> u32,
}
fn double(x: u32) -> u32 {
    x * 2
}
fn triple(x: u32) -> u32 {
    x * 3
}
#[inline(never)]
pub fn call_field(h: &Hook, x: u32) -> u32 {
    (h.release)(x)
}
pub fn roots(on: bool) -> u32 {
    let hook = Hook {
        release: if on { double } else { triple },
    };
    let b = B(3);
    let s: &dyn Speak = if on { &A } else { &b };
    call_dyn(s, 1) + call_field(&hook, 2)
}
