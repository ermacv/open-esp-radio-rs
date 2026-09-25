//! One application-owned RAM store around successive physical Controller epochs.
#[path = "secure/retirement.rs"]
mod retirement;
use super::console;
use bluetooth_example::security::{bonds::RamBondStore, epoch};
use core::convert::Infallible;
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use oer_esp32s31_bluetooth_integration::{BluetoothColdStartOutput, entropy::BluetoothEntropy};
use open_esp_radio_hil_protocol::{Command, Event, FeatureCapabilities};
use open_esp_radio_hil_target_core::bluetooth_gatt::secure::{reset_gate, state, store};
use state::State;

type HostExit<'a> = epoch::Exit<
    reset_gate::GatedController<
        'a,
        oer_esp32s31_bluetooth_integration::BluetoothHostController<4, 4, 258>,
    >,
    store::InjectedBondLoadFailure,
>;

impl console::Profile for State {
    fn features(&self) -> FeatureCapabilities {
        FeatureCapabilities {
            bluetooth_secure_gatt: true,
            ..Default::default()
        }
    }
    fn command(&self, command: Command) -> Event {
        let mut event = State::command(self, command);
        if let Event::BluetoothSecureGatt(value) = &mut event {
            value.traffic.cpu0_stack = Some(crate::cpu0_stack_usage_snapshot());
            use oer_esp32s31_bluetooth_integration::diagnostics::{
                self, BluetoothAdvertisingStartRejection as S,
            };
            use open_esp_radio_hil_protocol::BluetoothAdvertisingStartRejection as D;
            value.advertising_start_rejection = diagnostics::snapshot()
                .last_advertising_start_rejection
                .map(|reason| match reason {
                    S::Configuration => D::Configuration,
                    S::GenerationExhausted => D::GenerationExhausted,
                    S::PduFit => D::PduFit,
                    S::AdvertisingEventActive => D::AdvertisingEventActive,
                    S::PeripheralEventActive => D::PeripheralEventActive,
                    S::MemoryPreparation => D::MemoryPreparation,
                    S::TimingWindow => D::TimingWindow,
                    S::Timeline => D::Timeline,
                    S::Sequence => D::Sequence,
                    S::EventFields => D::EventFields,
                    S::EmptyList => D::EmptyList,
                });
        }
        event
    }
}

pub(super) async fn run(
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
    output: BluetoothColdStartOutput<4, 1, 4, 4, 258>,
    entropy: &'static BluetoothEntropy<'static>,
) -> Infallible {
    let state = State::new();
    // Neither is reconstructed when the Controller returns to cold ownership.
    let mut bonds = RamBondStore::<1>::new();
    let resources = super::RESOURCES.init_with(trouble_host::HostResources::new);
    let identity = output.calibration_identity;
    let mut system = Some(output.system);
    // Bind once in stable owner storage, outside the Host epoch loop. The
    // command endpoint retains this independent service across cold restart.
    system
        .as_mut()
        .expect("initial Controller")
        .runners
        .hardware
        .install_entropy(entropy)
        .expect("initial HCI entropy binding");
    let mut platform = output.platform;
    let mut lifecycle = core::pin::pin!(async {
        loop {
            let (exit, hardware) = {
                let mut live =
                    core::pin::pin!(run_host(&mut system, resources, &mut bonds, &state));
                let mut result = None;
                core::future::poll_fn(|cx| poll_into(live.as_mut(), &mut result, cx)).await;
                result.expect("completed Host epoch")
            };
            let ready = {
                let mut restart = core::pin::pin!(retirement::finish(
                    hardware, platform, exit, identity, &state
                ));
                let mut result = None;
                core::future::poll_fn(|cx| poll_into(restart.as_mut(), &mut result, cx)).await;
                result.expect("completed cold restart")
            };
            system = Some(ready.system);
            platform = ready.platform;
            state.restarted();
        }
        #[allow(unreachable_code)]
        core::future::pending::<Infallible>().await
    });
    let mut console = core::pin::pin!(console::run(usb, boot, &state));
    match select(
        core::future::poll_fn(|cx| super::poll_live(lifecycle.as_mut(), cx)),
        console.as_mut(),
    )
    .await
    {
        Either::First(never) | Either::Second(never) => never,
    }
}

// Write large affine results into the caller's future storage. Returning them
// through Poll would materialize another full owner graph on the caller stack.
#[inline(never)]
fn poll_into<F: Future>(
    future: core::pin::Pin<&mut F>,
    output: &mut Option<F::Output>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<()> {
    match super::poll_live(future, cx) {
        core::task::Poll::Pending => core::task::Poll::Pending,
        core::task::Poll::Ready(value) => {
            *output = Some(value);
            core::task::Poll::Ready(())
        }
    }
}

// Host construction and its consuming stop handoff have a separate poll frame
// from the physical close/restart owner transfers. Storage remains caller-owned.
async fn run_host<'a>(
    system: &mut Option<oer_esp32s31_bluetooth_integration::BluetoothSystem<4, 1, 4, 4, 258>>,
    resources: &mut trouble_host::HostResources<trouble_host::prelude::DefaultPacketPool, 1, 3>,
    bonds: &mut RamBondStore<1>,
    state: &'a State,
) -> (
    HostExit<'a>,
    oer_esp32s31_bluetooth_integration::BluetoothHardwareRunner<4, 1, 4, 4, 258>,
) {
    let composed = initialize_host(system.take().expect("fresh Controller"), resources, state);
    let finished = Signal::<NoopRawMutex, ()>::new();
    let mut host = core::pin::pin!(async {
        let mut store = store::Store { ram: bonds, state };
        let exit = {
            let mut epoch = core::pin::pin!(epoch::run(
                composed.stack,
                &mut store,
                &state.comparison,
                state.restart.wait(),
                |event| state.observe(event)
            ));
            core::future::poll_fn(|cx| super::poll_live(epoch.as_mut(), cx)).await
        };
        record_shutdown(state, &exit);
        match exit.action() {
            epoch::ShutdownAction::Retain => {
                state.stopped();
                core::hint::black_box(&exit);
                core::future::pending::<()>().await;
            }
            epoch::ShutdownAction::Close => state.stopped(),
            epoch::ShutdownAction::Restart => {}
        }
        finished.signal(());
        exit
    });
    let mut hardware = core::pin::pin!(composed.hardware.run_until_idle(finished.wait()));
    let mut live = core::pin::pin!(join(host.as_mut(), hardware.as_mut()));
    core::future::poll_fn(|cx| super::poll_live(live.as_mut(), cx)).await
}

fn record_shutdown(state: &State, exit: &HostExit<'_>) {
    if let epoch::Cause::Application(error) = &exit.cause {
        state.application_failure(error);
    }
    use bluetooth_example::security::{bonds::StoreError, gatt::RunError};
    use open_esp_radio_hil_protocol::{
        BluetoothGattResetOutcome as Reset, BluetoothGattShutdown, BluetoothGattStopCause as Cause,
    };
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

// Preserve the construction/codegen boundary while only wrapping the real HCI
// facade. No second Controller, transport queues or hardware owner is created.
#[inline(never)]
fn initialize_host<'r, 's>(
    system: oer_esp32s31_bluetooth_integration::BluetoothSystem<4, 1, 4, 4, 258>,
    resources: &'r mut trouble_host::HostResources<trouble_host::prelude::DefaultPacketPool, 1, 3>,
    state: &'s State,
) -> oer_esp32s31_bluetooth_integration::BluetoothTroubleSystem<
    'r,
    reset_gate::GatedController<
        's,
        oer_esp32s31_bluetooth_integration::BluetoothHostController<4, 4, 258>,
    >,
    trouble_host::prelude::DefaultPacketPool,
    oer_esp32s31_bluetooth_integration::BluetoothHardwareRunner<4, 1, 4, 4, 258>,
> {
    oer_esp32s31_bluetooth_integration::BluetoothTroubleSystem {
        stack: trouble_host::new(
            reset_gate::GatedController {
                inner: system.hci,
                gate: &state.reset_gate,
            },
            resources,
        )
        .build(),
        hardware: system.runners.hardware,
    }
}
