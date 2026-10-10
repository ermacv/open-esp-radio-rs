//! The radio port of an installed runtime, for the portable Controller
//! service loop.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_radio::{
    CancelError, ClockError, ClockInfo, EventId, EventsLost, LeInstant, LeRadio,
    LeRadioCapabilities, LeRadioPort, LifecycleCommand, LifecycleError, Poisoned, PortResult,
    RadioActivity, RadioEpoch, RadioOutcome, RadioPort, RadioRequest, RequestError,
};
use oer_esp32s31_hal::shared_radio::ClientQuiescence;
use oer_time::Timer;

use crate::runtime::Radio;
use crate::{
    BluetoothFault, BluetoothOutcome, BluetoothRadioHardware, BluetoothRuntime,
    BluetoothRuntimeFault,
};

/// The radio port of an installed [`BluetoothRuntime`], for the protocol's
/// one outcome consumer.
///
/// [`BluetoothRuntime::install`] returns it with the radio's
/// [`BluetoothControl`], and [`BluetoothControl::uninstall`] consumes both,
/// so neither meets a runtime without a radio: no call is refused as not
/// installed. It is neither `Copy` nor `Clone`; the consumer borrows it
/// while it serves. The runner ([`BluetoothRuntime::run`]) and the
/// interrupt entry keep reaching the runtime itself.
#[must_use = "an installed radio leaves only through its control's uninstall"]
pub struct BluetoothPort<'r, R> {
    runtime: &'r R,
}

/// The composition's handle to an installed [`BluetoothRuntime`]:
/// shared-PHY maintenance and the uninstall. [`BluetoothRuntime::install`]
/// returns it with the port; neither is `Copy` nor `Clone`.
#[must_use = "an installed radio leaves only through its control's uninstall"]
pub struct BluetoothControl<'r, R> {
    runtime: &'r R,
}

impl<'r, R> BluetoothControl<'r, R> {
    pub(crate) const fn new(runtime: &'r R) -> Self {
        Self { runtime }
    }

    /// The runtime this control belongs to.
    pub const fn runtime(&self) -> &'r R {
        self.runtime
    }
}

impl<'r, R> BluetoothPort<'r, R> {
    pub(crate) const fn new(runtime: &'r R) -> Self {
        Self { runtime }
    }

    /// The runtime this port belongs to.
    pub const fn runtime(&self) -> &'r R {
        self.runtime
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> RadioPort
    for BluetoothPort<
        '_,
        BluetoothRuntime<
            M,
            H,
            T,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
            EVENTS,
        >,
    >
{
    type Event = BluetoothOutcome;
    type Id = EventId;
    type Domain = LeRadio;
    type Fault = BluetoothFault<H::StartError>;

    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<BluetoothOutcome, EventsLost, Self::Fault>> + '_ {
        self.runtime.wait_event()
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, Self::Fault>> + '_ {
        self.runtime.read_now()
    }

    fn cancel(
        &self,
        id: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_ {
        self.runtime.cancel_event(id)
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Self::Fault>> + '_ {
        self.runtime.run_lifecycle(command)
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> LeRadioPort
    for BluetoothPort<
        '_,
        BluetoothRuntime<
            M,
            H,
            T,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
            EVENTS,
        >,
    >
{
    fn capabilities(&self) -> LeRadioCapabilities {
        self.runtime.capabilities()
    }

    /// The radio time is the controller clock, extended from the 32-bit
    /// controller-time latch the radio samples; the runtime keeps no
    /// measured relation between it and the image's monotonic clock.
    fn clock_info(&self) -> ClockInfo {
        ClockInfo {
            resolution: ClockInfo::MONOTONIC_MICROS.resolution,
            epoch: RadioEpoch::Unrelated,
        }
    }

    fn submit(
        &self,
        request: RadioRequest<'_>,
    ) -> impl Future<Output = PortResult<(), RequestError, Self::Fault>> {
        self.runtime.submit_request(request)
    }

    fn view(outcome: &BluetoothOutcome) -> RadioOutcome<'_> {
        outcome.portable()
    }

    fn activity(&self, activity: RadioActivity) -> Result<(), Poisoned<Self::Fault>> {
        self.runtime.publish_activity(activity);
        Ok(())
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
>
    BluetoothControl<
        '_,
        BluetoothRuntime<
            M,
            H,
            T,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
            EVENTS,
        >,
    >
{
    /// Quiesce the port, stop the scheduler, hand the Bluetooth quiescence
    /// proof to `maintenance`, resume the scheduler and enable the port
    /// again: shared-PHY maintenance as a layer over the port's lifecycle.
    /// The port's consumer sees `Quiesced` and `Enabled` and must keep
    /// taking outcomes meanwhile: a full queue refuses `Quiesce` and the
    /// final `Enable` as busy until it frees a slot. A list transaction in
    /// progress finishes before the stop.
    ///
    /// # Errors
    ///
    /// [`Poisoned`] when a hardware fault stopped the runtime.
    pub async fn quiesce<Q>(
        &self,
        maintenance: impl FnOnce(ClientQuiescence<'_>) -> Q,
    ) -> Result<Q, Poisoned<BluetoothFault<H::StartError>>> {
        self.runtime.quiesce(maintenance).await
    }

    /// Stop the scheduler and take the radio and its hardware out of the
    /// runtime, consuming both handles of the installed radio. Events still
    /// listed never end; the next radio starts disabled.
    ///
    /// # Panics
    ///
    /// `port` belongs to another runtime.
    ///
    /// # Errors
    ///
    /// The scheduler could not be stopped: the radio stays installed and
    /// faulted, and both handles are returned with the fault.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the role's pool capacities stay visible in the returned radio"
    )]
    pub async fn uninstall<'p>(
        self,
        port: BluetoothPort<
            'p,
            BluetoothRuntime<
                M,
                H,
                T,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
                EVENTS,
            >,
        >,
    ) -> Result<
        (
            Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
            H,
        ),
        (
            BluetoothRuntimeFault<H::StartError>,
            Self,
            BluetoothPort<
                'p,
                BluetoothRuntime<
                    M,
                    H,
                    T,
                    LEGACY,
                    CONNECTABLE,
                    SCANNERS,
                    CONNECTIONS,
                    SCAN_PACKETS,
                    RX_PACKETS,
                    ITEMS,
                    EVENTS,
                >,
            >,
        ),
    > {
        assert!(
            core::ptr::eq(self.runtime, port.runtime),
            "the port belongs to this control's runtime"
        );
        match self.runtime.uninstall().await {
            Ok(parts) => Ok(parts),
            Err(fault) => Err((fault, self, port)),
        }
    }
}
