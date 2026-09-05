use core::{
    future::{Future, ready},
    pin::{Pin, pin},
    task::{Context, Poll},
};

use open_esp_radio_esp32s31_hal::types::{
    MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi_mac::{
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot},
    tx_runtime::WifiTxRuntimePolicy,
};

use super::*;
use crate::control_tx::{ControlTxConfig, WifiTxResources};
use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;

#[derive(Default)]
struct ScanTxHardware {
    publications: u8,
    completion: Option<MacTxCompletionObservation>,
}

impl TxHardware for ScanTxHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications = self.publications.saturating_add(1);
    }

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
            MacTxDetachReason::Collision | MacTxDetachReason::Timeout => {
                MacTxDetachOutcome::NoEvent
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ScanTxPower;

impl WifiTxPowerProfile for ScanTxPower {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

#[derive(Default)]
struct ScanTxTimer {
    now: u64,
}

impl WifiTxTimer for ScanTxTimer {
    fn now_micros(&self) -> u64 {
        self.now
    }

    fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.now = deadline_micros;
        ready(())
    }

    fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
        self.now = self.now.saturating_add(micros);
        ready(())
    }
}

fn scan_tx_completion(status: u8) -> MacTxCompletionObservation {
    MacTxCompletionObservation::new_model(status, 0)
}

fn running_scan_tx<'a>(
    slot: Pin<&'a mut TxSlot<256>>,
) -> Esp32s31RunningScanTx<'a, ScanTxPower, fn() -> u32, ScanTxTimer, 256> {
    fn entropy() -> u32 {
        0x1234_5678
    }
    Esp32s31RunningScanTx::new_for_test(Esp32s31ControlTx::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: ScanTxPower,
            entropy,
            timer: ScanTxTimer::default(),
        },
        ControlTxConfig {
            unicast_attempt_limit: 2,
            completion_timeout_us: 10,
            poll_interval_us: 1,
        },
    ))
}

fn scan_probe_request() -> Esp32s31ScanProbeRequest<'static> {
    Esp32s31ScanProbeRequest {
        source: [2, 3, 4, 5, 6, 7],
        sequence_number: 9,
        ssid: b"",
        supported_rates: &[0x82, 0x84],
        current_channel: Some(6),
        descriptor_capacity: Some(256),
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(core::task::Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn running_scan_tx_returns_the_control_owner_after_a_probe() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let mut hardware = ScanTxHardware {
        completion: Some(scan_tx_completion(0)),
        ..ScanTxHardware::default()
    };
    let mut tx = running_scan_tx(slot.as_mut());
    tx.begin_scan();

    let report = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request()))
        .expect("completed running probe");
    assert!(matches!(report, Esp32s31ScanProbeReport::Transmitted(_)));
    let (mut control, summary) = tx.into_parts();
    assert_eq!(summary.completions, 1);
    assert_eq!(summary.failures, 0);

    hardware.completion = Some(scan_tx_completion(0));
    block_on(control.transmit_probe_request(
        &mut hardware,
        ProbeRequest {
            destination: open_esp_radio_ieee80211::management::BROADCAST_ADDRESS,
            source: [2, 3, 4, 5, 6, 7],
            bssid: open_esp_radio_ieee80211::management::BROADCAST_ADDRESS,
            sequence_number: 10,
            ssid: b"",
            supported_rates: &[0x82, 0x84],
        },
        Some(6),
        Some(256),
    ))
    .expect("returned control owner remains usable");
    assert_eq!(hardware.publications, 2);
}

#[test]
fn failed_running_probe_disables_further_active_attempts() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let mut hardware = ScanTxHardware {
        completion: Some(scan_tx_completion(1)),
        ..ScanTxHardware::default()
    };
    let mut tx = running_scan_tx(slot.as_mut());
    tx.begin_scan();

    let first = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request()))
        .expect("nonzero completion is a safe passive fallback");
    assert!(matches!(
        first,
        Esp32s31ScanProbeReport::PassiveAfterCompletion(_)
    ));
    let second = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request()))
        .expect("disabled active probe remains passive");
    assert_eq!(second, Esp32s31ScanProbeReport::PassiveWithoutAttempt);
    assert_eq!(hardware.publications, 1);

    let (_control, summary) = tx.into_parts();
    assert_eq!(summary.completions, 1);
    assert_eq!(summary.failures, 1);
}
