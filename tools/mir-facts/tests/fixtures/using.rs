#![no_std]
pub use defining::api as reexported;
#[inline(never)]
pub fn drop_packet(pool: &defining::Pool, header: core::ptr::NonNull<defining::buf::Header>) {
    (pool.release)(header, 1)
}
#[inline(never)]
pub fn send(driver: &dyn reexported::Driver) -> usize {
    driver.send(4)
}
