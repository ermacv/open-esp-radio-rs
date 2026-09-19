//! One application-owned RAM store around successive physical Controller epochs.
#[path = "secure/retirement.rs"]
mod retirement;
#[path = "secure/state.rs"]
mod state;
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
use state::State;

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
            let (old_hci, hardware) = {
                let mut live =
                    core::pin::pin!(run_host(&mut system, resources, &mut bonds, &state));
                let mut result = None;
                core::future::poll_fn(|cx| poll_into(live.as_mut(), &mut result, cx)).await;
                result.expect("completed Host epoch")
            };
            let ready = {
                let mut restart = core::pin::pin!(retirement::cold_restart(
                    hardware, platform, old_hci, identity, &state
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
async fn run_host(
    system: &mut Option<oer_esp32s31_bluetooth_integration::BluetoothSystem<4, 1, 4, 4, 258>>,
    resources: &mut trouble_host::HostResources<trouble_host::prelude::DefaultPacketPool, 1, 3>,
    bonds: &mut RamBondStore<1>,
    state: &State,
) -> (
    oer_esp32s31_bluetooth_integration::BluetoothHostController<4, 4, 258>,
    oer_esp32s31_bluetooth_integration::BluetoothHardwareRunner<4, 1, 4, 4, 258>,
) {
    let composed = super::initialize_host(system.take().expect("fresh Controller"), resources);
    let finished = Signal::<NoopRawMutex, ()>::new();
    let mut host = core::pin::pin!(async {
        let exit = {
            let mut epoch = core::pin::pin!(epoch::run(
                composed.stack,
                bonds,
                &state.comparison,
                state.restart.wait(),
                |event| state.observe(event)
            ));
            core::future::poll_fn(|cx| super::poll_live(epoch.as_mut(), cx)).await
        };
        // A failed Host or Reset never authorizes physical retirement/restart.
        if !matches!(exit.cause, epoch::Cause::Requested) || exit.reset.is_err() {
            state.stopped();
            core::hint::black_box(&exit);
            core::future::pending::<()>().await;
        }
        finished.signal(());
        exit.controller
    });
    let mut hardware = core::pin::pin!(composed.hardware.run_until_idle(finished.wait()));
    let mut live = core::pin::pin!(join(host.as_mut(), hardware.as_mut()));
    core::future::poll_fn(|cx| super::poll_live(live.as_mut(), cx)).await
}
