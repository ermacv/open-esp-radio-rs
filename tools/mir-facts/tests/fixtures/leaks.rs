#![no_std]
//! One function-pointer type per way out of it, and one kept.
use core::sync::atomic::{AtomicPtr, Ordering};

pub fn kept() -> i8 {
    1
}
pub fn in_union() -> u8 {
    2
}
pub fn cast() -> u16 {
    3
}
pub fn transmuted() -> u32 {
    4
}
pub fn through_bytes(x: u32) -> u32 {
    x
}
pub fn atomic() -> u64 {
    6
}
pub fn widened() -> i16 {
    7
}

pub union Either {
    pub function: fn() -> u8,
    pub word: usize,
}

pub static KEPT: fn() -> i8 = kept;
pub static ATOMIC: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

#[inline(never)]
pub fn by_union() -> usize {
    let either = Either { function: in_union };
    unsafe { either.word }
}

#[inline(never)]
pub fn by_cast() -> *const () {
    let f: fn() -> u16 = cast;
    f as *const ()
}

#[inline(never)]
pub fn by_transmute() -> usize {
    let f: fn() -> u32 = transmuted;
    unsafe { core::mem::transmute::<fn() -> u32, usize>(f) }
}

#[inline(never)]
pub fn by_bytes(buffer: &mut [u8; 4]) -> fn(u32) -> u32 {
    let f: fn(u32) -> u32 = through_bytes;
    unsafe {
        core::ptr::write_unaligned(buffer.as_mut_ptr() as *mut fn(u32) -> u32, f);
        core::ptr::read_unaligned(buffer.as_ptr() as *const fn(u32) -> u32)
    }
}

#[inline(never)]
pub fn by_atomic() {
    let f: fn() -> u64 = atomic;
    ATOMIC.store(f as *mut (), Ordering::Relaxed);
}

#[inline(never)]
pub fn by_edge() -> fn() -> u16 {
    let f: fn() -> i16 = widened;
    unsafe { core::mem::transmute::<fn() -> i16, fn() -> u16>(f) }
}

#[inline(never)]
pub fn by_lifetime(f: fn(&'static u8) -> u32) -> fn(&u8) -> u32 {
    unsafe { core::mem::transmute::<fn(&'static u8) -> u32, fn(&u8) -> u32>(f) }
}

pub trait Hidden {
    fn hidden(&self) -> u32;
}
pub struct Carrier {
    pub function: fn(u8) -> u8,
}
impl Hidden for Carrier {
    fn hidden(&self) -> u32 {
        (self.function)(1) as u32
    }
}
fn doubled(x: u8) -> u8 {
    x * 2
}

/// A `dyn` behind erased bytes leaks its trait; the trait's implementor
/// carries `fn(u8) -> u8`.
#[inline(never)]
pub fn by_dyn(slot: &mut [usize; 2]) {
    let carrier: &'static Carrier = &Carrier { function: doubled };
    let hidden: &dyn Hidden = carrier;
    unsafe { core::ptr::write(slot.as_mut_ptr() as *mut &dyn Hidden, hidden) };
}

/// Views of one shape reinterpret nothing.
#[inline(never)]
pub fn by_wrapper(cell: &core::cell::UnsafeCell<fn() -> i8>) -> *mut fn() -> i8 {
    cell.get()
}

/// `NonNull` is a view of its pointer: no leak.
#[inline(never)]
pub fn by_non_null(slot: &mut fn() -> i8) -> core::ptr::NonNull<fn() -> i8> {
    core::ptr::NonNull::from(slot)
}

/// An address without provenance accesses nothing: no leak.
#[inline(never)]
pub fn by_address(slot: &fn() -> i8) -> usize {
    (slot as *const fn() -> i8).addr()
}

/// An exposed address joins the exposed contents; a pointer made from an
/// integer leaks its own type.
#[inline(never)]
pub fn by_exposure(slot: &fn() -> u128, address: usize) -> *const fn() -> i128 {
    let _ = slot as *const fn() -> u128 as usize;
    address as *const fn() -> i128
}
