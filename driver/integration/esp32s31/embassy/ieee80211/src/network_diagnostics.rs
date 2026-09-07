//! Interface selection for independent STA/AP resource observations.

/// Logical Wi-Fi endpoint whose network resources are being observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkInterface {
    Station,
    AccessPoint,
}

#[cfg(any(
    test,
    all(
        target_arch = "riscv32",
        any(feature = "upstream-network", feature = "compat-network")
    )
))]
pub(crate) struct Monitors<T> {
    endpoints: embassy_sync::blocking_mutex::Mutex<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        core::cell::RefCell<Option<(T, T)>>,
    >,
}

#[cfg(any(
    test,
    all(
        target_arch = "riscv32",
        any(feature = "upstream-network", feature = "compat-network")
    )
))]
impl<T> Monitors<T> {
    pub(crate) const fn new() -> Self {
        Self {
            endpoints: embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(None)),
        }
    }

    pub(crate) fn initialize(&self, station: T, access_point: T) {
        self.endpoints
            .lock(|endpoints| *endpoints.borrow_mut() = Some((station, access_point)));
    }

    pub(crate) fn snapshot<S>(
        &self,
        interface: NetworkInterface,
        read: impl FnOnce(&T) -> S,
    ) -> Option<S> {
        self.endpoints.lock(|endpoints| {
            let endpoints = endpoints.borrow();
            let (station, access_point) = endpoints.as_ref()?;
            Some(read(match interface {
                NetworkInterface::Station => station,
                NetworkInterface::AccessPoint => access_point,
            }))
        })
    }
}

#[cfg(test)]
mod tests;
