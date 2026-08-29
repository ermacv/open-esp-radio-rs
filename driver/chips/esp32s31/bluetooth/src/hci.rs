//! Source-owned HCI initialization after the scheduler component.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    InProcessHciHostTransport, LeControllerBootstrapConfig, LeControllerHciResources,
    LeControllerHciRuntimeWorker,
};

use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerTaskRuntime,
    BluetoothSchedulerInitialized, BluetoothSchedulerSoftwareConfig,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

/// Why a scheduler epoch could not acquire an HCI resource epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerHciInitializationError {
    /// A packet or successful bootstrap command already entered the supplied
    /// HCI resources.
    ResourcesNotPristine,
}

/// Lossless failure to bind scheduler and HCI resource ownership.
#[must_use = "the scheduler and HCI resources remain owned after a rejected bind"]
pub struct BluetoothControllerHciInitializationFailure<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    error: BluetoothControllerHciInitializationError,
    scheduler: BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY>,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciInitializationFailure<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Exact reason the affine bind was refused.
    pub const fn error(&self) -> BluetoothControllerHciInitializationError {
        self.error
    }

    /// Recover both unchanged owners after a rejected bind.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY>,
        LeControllerHciResources<
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        (self.scheduler, self.hci)
    }
}

/// Powered scheduler ownership with one source-owned HCI bootstrap epoch.
///
/// This state replaces the reviewed vendor HCI environment, packet mempool and
/// generic broker node with one bounded Rust aggregate. It proves only that
/// the Host transport and conservative bootstrap dispatcher belong to the
/// same powered Controller epoch. It exposes no advertising, scanning,
/// connection, ACL dataplane, Link Layer, PHY or on-air readiness.
#[must_use = "the initialized HCI state retains every powered Bluetooth owner"]
pub struct BluetoothControllerHciInitialized<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    scheduler: BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY>,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// All borrowed runtime endpoints from one powered Controller epoch.
///
/// Named fields keep interrupt, task and HCI roles explicit. The mutable task
/// and HCI-worker borrows prevent a second split while this value is alive.
#[must_use = "all runtime endpoints belong to one powered Controller epoch"]
pub struct BluetoothControllerRuntimeEndpoints<
    'runtime,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Interrupt-side scheduler publications.
    pub interrupt: BluetoothControllerInterruptRuntime<'runtime>,
    /// Task-side scheduler workers.
    pub task: BluetoothControllerTaskRuntime<'runtime>,
    /// Host-facing typed HCI transport.
    pub hci_host: InProcessHciHostTransport<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    /// Sole conservative HCI bootstrap worker.
    pub hci_worker: LeControllerHciRuntimeWorker<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Number of scheduler modem-timer slots retained by this exact epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.scheduler.modem_timer_capacity()
    }

    /// Scheduler time scale retained by this exact powered epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.scheduler.controller_time_scale()
    }

    /// Source-owned scheduler policy retained by this exact powered epoch.
    pub const fn scheduler_config(&self) -> BluetoothSchedulerSoftwareConfig {
        self.scheduler.scheduler_config()
    }

    /// Immutable HCI values reported to the Host during bootstrap.
    pub const fn hci_config(&self) -> LeControllerBootstrapConfig {
        self.hci.config()
    }

    /// Whether neither scheduler nor HCI software work entered this epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.scheduler.runtime_is_pristine() && self.hci.is_pristine()
    }

    /// Borrow all matching interrupt, task and HCI endpoints at once.
    pub fn split_runtime(
        &mut self,
    ) -> BluetoothControllerRuntimeEndpoints<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let (interrupt, task) = self.scheduler.split_runtime();
        let (hci_host, hci_worker) = self.hci.split();
        BluetoothControllerRuntimeEndpoints {
            interrupt,
            task,
            hci_host,
            hci_worker,
        }
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
    /// Bind a pristine bounded HCI bootstrap epoch after scheduler init.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return both affine powered owners"
    )]
    pub fn initialize_hci<
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
        BluetoothControllerHciInitialized<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerHciInitializationFailure<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >
    where
        M: RawMutex,
    {
        if !hci.is_pristine() {
            return Err(BluetoothControllerHciInitializationFailure {
                error: BluetoothControllerHciInitializationError::ResourcesNotPristine,
                scheduler: self,
                hci,
            });
        }
        Ok(BluetoothControllerHciInitialized {
            scheduler: self,
            hci,
        })
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        LeControllerBootstrapConfig, LeControllerHciResources,
        bt_hci::{cmd::controller_baseband::Reset, param::BdAddr, transport::Transport},
    };
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use crate::{
        BluetoothClockedResources, BluetoothControllerHciInitializationError,
        BluetoothControllerRuntimeEndpoints, BluetoothControllerRuntimeResources,
        BluetoothSchedulerInitialized, BluetoothStopped,
    };

    type TestHciResources = LeControllerHciResources<NoopRawMutex, 1, 1, 31>;

    fn scheduler() -> BluetoothSchedulerInitialized<(), 4> {
        let stopped = BluetoothStopped::from_hardware((), RadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        BluetoothClockedResources::for_validation(registers, platform)
            .initialize_controller_hal_with(|_, _| {})
            .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::new())
    }

    fn hci() -> TestHciResources {
        let config = LeControllerBootstrapConfig::new(BdAddr::new([2, 3, 5, 7, 11, 13]), 27, 1)
            .expect("nonzero test profile");
        TestHciResources::new(config).expect("profile fits its bounded queues")
    }

    #[test]
    fn scheduler_and_hci_split_as_one_pristine_powered_epoch() {
        let mut controller = scheduler()
            .initialize_hci(hci())
            .unwrap_or_else(|_| panic!("pristine HCI resources must bind"));
        assert!(controller.runtime_is_pristine());
        assert_eq!(controller.modem_timer_capacity(), 4);

        let BluetoothControllerRuntimeEndpoints {
            interrupt,
            task,
            hci_host,
            mut hci_worker,
        } = controller.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        block_on(async {
            let (sent, processed) = embassy_futures::join::join(
                hci_host.write(&Reset::new()),
                hci_worker.process_one(),
            )
            .await;
            sent.expect("Reset enters the matching Host queue");
            processed.expect("the matching bootstrap worker publishes a response");
        });
    }

    #[test]
    fn used_hci_epoch_is_rejected_without_losing_either_owner() {
        let mut used_hci = hci();
        {
            let (host, mut worker) = used_hci.split();
            block_on(async {
                let (sent, processed) =
                    embassy_futures::join::join(host.write(&Reset::new()), worker.process_one())
                        .await;
                sent.expect("Reset enters the test queue");
                processed.expect("the test worker publishes a response");
            });
        }

        let failure = match scheduler().initialize_hci(used_hci) {
            Ok(_) => panic!("a used HCI epoch must not bind"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothControllerHciInitializationError::ResourcesNotPristine
        );
        let (scheduler, hci) = failure.into_parts();
        assert!(scheduler.runtime_is_pristine());
        assert!(!hci.is_pristine());
    }
}
