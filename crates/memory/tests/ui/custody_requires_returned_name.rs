#![no_std]
use oer_memory::{CustodyDomain, IndexedCustody, RetainedName};

#[cfg(not(ui_reject))]
pub fn finish<T, D: CustodyDomain<N>, const N: usize>(
    storage: &IndexedCustody<T, D, N>,
    name: RetainedName<D::Name, N>,
) -> T {
    storage.take(name).1
}

#[cfg(ui_reject)]
pub fn finish<T, D: CustodyDomain<N>, const N: usize>(
    storage: &IndexedCustody<T, D, N>,
    name: D::Name,
) -> T {
    storage.take(name).1 // ui-error: E0308
}
