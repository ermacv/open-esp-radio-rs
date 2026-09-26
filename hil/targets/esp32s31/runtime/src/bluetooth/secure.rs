//! One application-owned RAM bond store around successive Controller epochs.
//!
//! Each Host epoch runs the Trouble Host and the secure GATT application over
//! a fresh Host end of the HCI transport, beside the radio runner and the HCI
//! service. A requested restart resets the Host through HCI, retires the
//! drained Host end, stops the Controller on the shared radio, checks that the
//! old Host end stays closed and starts the next Controller epoch, whose Host
//! restores the retained bonds. A failed application closes the Controller
//! without restarting; an unconfirmed Reset retains every owner as it is.

use core::{convert::Infallible, future::pending, pin::pin};

use embassy_futures::select::{Either, select};
use gatt_application::security::{
    bonds::RamBondStore,
    epoch::{self, ShutdownAction},
};
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress,
    bt_hci::{
        cmd::{SyncCmd, controller_baseband::Reset},
        controller::{Controller as _, ExternalController},
    },
};
use oer_esp32s31_bluetooth_system::{
    BluetoothHci, BluetoothHciService, BluetoothHostTransport, BluetoothSystem, start,
};
use oer_hil_protocol::{Command, Event, FeatureCapabilities};
use oer_hil_target_core::bluetooth_gatt::secure::{
    reset_gate::{self, GatedController},
    state::State,
    store::{self, Store},
};
use static_cell::StaticCell;
use trouble_host::prelude::*;

use super::{Radio, console};

/// Command slots the Host keeps in flight.
const COMMAND_SLOTS: usize = 4;

type Controller = ExternalController<BluetoothHostTransport, COMMAND_SLOTS>;
type Gated = GatedController<'static, Controller>;
type Resources = HostResources<DefaultPacketPool, 1, 3>;
type HostExit = epoch::Exit<Gated, store::InjectedBondLoadFailure>;

static RESOURCES: StaticCell<Resources> = StaticCell::new();
static STATE: StaticCell<State> = StaticCell::new();

impl console::Profile for State {
    fn features(&self) -> FeatureCapabilities {
        FeatureCapabilities {
            bluetooth_secure_gatt: true,
            ..FeatureCapabilities::default()
        }
    }

    fn maximum_payload_bytes(&self) -> u16 {
        0
    }

    async fn command(&self, command: Command) -> Event {
        let mut event = State::command(self, command);
        if let Event::BluetoothSecureGatt(value) = &mut event {
            value.traffic.cpu0_stack = Some(crate::cpu0_stack_usage_snapshot());
        }
        event
    }
}

/// Serve the console and run Host epochs until one ends terminally.
pub(super) async fn run(
    radio: &'static Radio,
    system: BluetoothSystem,
    hci: BluetoothHci,
    public_address: BluetoothPublicDeviceAddress,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) -> ! {
    let state: &'static State = STATE.init_with(State::new);
    let lifecycle = pin!(lifecycle(radio, system, hci, public_address, state));
    let console = pin!(console::run(usb, boot, state));
    match select(lifecycle, console).await {
        Either::First(never) => match never {},
        Either::Second(never) => never,
    }
}

async fn lifecycle(
    radio: &'static Radio,
    mut system: BluetoothSystem,
    hci: BluetoothHci,
    public_address: BluetoothPublicDeviceAddress,
    state: &'static State,
) -> Infallible {
    // Neither is reconstructed when the Controller restarts.
    let mut bonds = RamBondStore::<1>::new();
    let resources = RESOURCES.init_with(HostResources::new);
    let BluetoothHci { host, mut service } = hci;
    let mut host = Some(host);
    loop {
        let transport = match host.take() {
            Some(transport) => transport,
            None => match service.restart() {
                Ok(transport) => transport,
                Err(_) => {
                    crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-restart\r\n")
                }
            },
        };
        let exit = {
            let mut epoch = pin!(run_epoch(
                &mut system,
                &mut service,
                transport,
                resources,
                &mut bonds,
                state,
            ));
            epoch.as_mut().await
        };
        record_shutdown(state, &exit);
        let action = exit.action();
        if action == ShutdownAction::Retain {
            state.stopped();
            retain((exit, system, service)).await;
        }

        // Retire the old Host end once its undelivered packets are drained.
        match select(service.wait_retirement_ready(), drain(&exit.controller)).await {
            Either::First(Ok(())) => {}
            Either::First(Err(_)) => {
                crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-retirement\r\n")
            }
            Either::Second(never) => match never {},
        }
        if service.retire().is_err() {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-retirement\r\n");
        }
        let parked = {
            let mut stop = pin!(system.stop(radio));
            match stop.as_mut().await {
                Ok(parked) => parked,
                Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-stop\r\n"),
            }
        };
        let closed = old_hci_closed(&exit.controller).await;
        state.cold(closed);
        if !closed || action == ShutdownAction::Close {
            // A reset failed Host may close the Controller, but never erase
            // its failure and restart as if a fresh epoch had been requested.
            state.stopped();
            retain((exit, parked, service)).await;
        }
        system = {
            let mut restart = pin!(start(radio, parked, public_address));
            match restart.as_mut().await {
                Ok(system) => system,
                Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-restart\r\n"),
            }
        };
        // Retain the old Host facade until the new Controller epoch runs.
        drop(exit);
        state.restarted();
    }
}

/// Run one Host epoch beside the radio runner and the HCI service.
async fn run_epoch(
    system: &mut BluetoothSystem,
    service: &mut BluetoothHciService,
    transport: BluetoothHostTransport,
    resources: &mut Resources,
    bonds: &mut RamBondStore<1>,
    state: &'static State,
) -> HostExit {
    let stack = build(
        GatedController {
            inner: Controller::new(transport),
            gate: &state.reset_gate,
        },
        resources,
    );
    let mut store = Store { ram: bonds, state };
    let host = pin!(epoch::run(
        stack,
        &mut store,
        &state.comparison,
        state.restart.wait(),
        |event| state.observe(event),
    ));
    let hardware = pin!(select(system.run(), service.run()));
    match select(host, hardware).await {
        Either::First(exit) => exit,
        Either::Second(Either::First(_)) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-runner-fault\r\n")
        }
        Either::Second(Either::Second(_)) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-service\r\n")
        }
    }
}

// Host construction materializes security-manager and connection state
// beyond the caller-owned resource arrays; keep it out of the epoch frame.
#[inline(never)]
fn build(controller: Gated, resources: &mut Resources) -> Stack<'_, Gated, DefaultPacketPool> {
    trouble_host::new(controller, resources).build()
}

fn record_shutdown(state: &State, exit: &HostExit) {
    use gatt_application::security::{bonds::StoreError, gatt::RunError};
    use oer_hil_protocol::{
        BluetoothGattResetOutcome as Reset, BluetoothGattShutdown, BluetoothGattStopCause as Cause,
    };
    if let epoch::Cause::Application(error) = &exit.cause {
        state.application_failure(error);
    }
    let cause = match &exit.cause {
        epoch::Cause::Requested => Cause::Requested,
        epoch::Cause::Application(RunError::Store(StoreError::Backend(
            store::InjectedBondLoadFailure,
        ))) => Cause::InjectedBondLoadFailure,
        epoch::Cause::Application(_) => Cause::Application,
        epoch::Cause::Host(_) => Cause::Host,
    };
    let reset = match &exit.reset {
        Ok(()) => Reset::Completed,
        Err(epoch::ResetError::BootstrapUnacknowledged) => Reset::BootstrapUnacknowledged,
        Err(epoch::ResetError::HostDuringBootstrapDrain(_)) => Reset::BootstrapDrainFailed,
        Err(epoch::ResetError::Command(_)) => Reset::CommandFailed,
        Err(epoch::ResetError::Receive(reset_gate::ReadError::Injected)) => {
            Reset::InjectedReceiveFailure
        }
        Err(epoch::ResetError::Receive(_)) => Reset::ReceiveFailed,
    };
    state.shutdown(BluetoothGattShutdown { cause, reset });
}

/// Read and discard what the old Host end still receives; ends only when the
/// transport closes.
async fn drain(controller: &Gated) -> Infallible {
    loop {
        let Ok(mut buffer) = controller.alloc_buf() else {
            return pending().await;
        };
        if controller.read(&mut buffer).await.is_err() {
            return pending().await;
        }
    }
}

/// Whether both directions of the old Host end report the transport closed.
async fn old_hci_closed(controller: &Gated) -> bool {
    use embedded_io_async::{Error, ErrorKind};
    let commands = matches!(
        Reset::new().exec(controller).await,
        Err(oer_bluetooth_hci::bt_hci::cmd::Error::Io(error)) if error.kind() == ErrorKind::BrokenPipe
    );
    let Ok(mut buffer) = controller.alloc_buf() else {
        return false;
    };
    let events = matches!(
        controller.read(&mut buffer).await,
        Err(error) if error.kind() == ErrorKind::BrokenPipe
    );
    commands && events
}

/// Keep terminal owners alive and never resume them.
async fn retain<T>(owners: T) -> ! {
    pending::<()>().await;
    drop(owners);
    unreachable!("terminal owners are never resumed")
}
