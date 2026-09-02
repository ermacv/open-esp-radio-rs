//! Post-publication HCI binding for one stable hardware epoch.

#[cfg(any(target_arch = "riscv32", test))]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_hci::LeControllerHciResources;

/// Rejection reason for post-publication HCI binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum BluetoothControllerHciBindError {
    /// The supplied HCI queues already contain state from another runtime epoch.
    ResourcesNotPristine,
}

#[cfg(any(target_arch = "riscv32", test))]
fn validate_hci_bind<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>(
    hci: &LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
) -> Result<(), BluetoothControllerHciBindError>
where
    M: RawMutex,
{
    if hci.is_pristine() {
        Ok(())
    } else {
        Err(BluetoothControllerHciBindError::ResourcesNotPristine)
    }
}

/// Lossless post-publication HCI binding failure.
#[must_use = "failed HCI binding returns both the hardware and protocol owners"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerHciBindFailure<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    hardware: crate::BluetoothControllerInterruptOwnersPublished<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    error: BluetoothControllerHciBindError,
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciBindFailure<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Exact reason why the protocol resources could not join this hardware epoch.
    pub const fn error(&self) -> BluetoothControllerHciBindError {
        self.error
    }

    /// Recover both unchanged affine owners.
    pub fn into_parts(
        self,
    ) -> (
        crate::BluetoothControllerInterruptOwnersPublished<
            P,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
        LeControllerHciResources<
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        (self.hardware, self.hci)
    }
}

/// Final published hardware owner joined to one pristine HCI protocol epoch.
#[must_use = "the final Bluetooth Controller owner must remain in stable storage"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerHciBound<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    hardware: crate::BluetoothControllerInterruptOwnersPublished<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

#[cfg(target_arch = "riscv32")]
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    crate::BluetoothControllerInterruptOwnersPublished<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
{
    /// Bind pristine HCI protocol resources after all interrupt owners reached stable storage.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure returns both complete affine owners"
    )]
    pub fn bind_hci<
        M,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        hci: LeControllerHciResources<
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<
        BluetoothControllerHciBound<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerHciBindFailure<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >
    where
        M: RawMutex,
    {
        match validate_hci_bind(&hci) {
            Ok(()) => Ok(BluetoothControllerHciBound {
                hardware: self,
                hci,
            }),
            Err(error) => Err(BluetoothControllerHciBindFailure {
                hardware: self,
                hci,
                error,
            }),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciBound<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
    S: crate::BluetoothModemLpTimerSoftwareOwnerStorage,
{
    /// Split one stable final owner into routed runtime endpoints.
    pub fn split_runtime<'runtime>(
        &'runtime mut self,
    ) -> crate::BluetoothControllerPublishedRuntimeSplit<
        'runtime,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let Self { hardware, hci } = self;
        hardware.split_hardware_runtime().bind_hci(hci.split())
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciEndpoints,
        LeControllerHciResources,
        bt_hci::{cmd::controller_baseband::Reset, transport::Transport},
    };

    use super::{BluetoothControllerHciBindError, validate_hci_bind};

    fn hci() -> LeControllerHciResources<NoopRawMutex, 1, 1, 45> {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .expect("nonzero test profile");
        LeControllerHciResources::new(config).expect("profile fits its bounded queues")
    }

    #[test]
    fn post_publication_hci_binding_requires_a_pristine_epoch() {
        let pristine = hci();
        assert_eq!(validate_hci_bind(&pristine), Ok(()));

        let mut used = hci();
        {
            let LeControllerHciEndpoints {
                host,
                controller: _,
            } = used.split();
            block_on(async {
                host.write(&Reset::new())
                    .await
                    .expect("Reset enters the test queue");
            });
        }
        assert_eq!(
            validate_hci_bind(&used),
            Err(BluetoothControllerHciBindError::ResourcesNotPristine)
        );
    }
}
