#![no_std]
use oer_memory::{CustodyDomain, IndexedCustody, RetainedName};

pub fn finish<T, D: CustodyDomain<N>, const N: usize>(
    storage: &IndexedCustody<T, D, N>,
    name: RetainedName<D::Name, N>,
) -> T {
    #[cfg(ui_reject)]
    let _ = storage.take(name);
    storage.take(name).1 // ui-error: E0382
}
