//! Concrete ESP32-S31 active-scan probe TX ownership.
//!
//! The executor-independent scan transaction decides when an active probe is
//! attempted. This module binds that edge to the polling control-TX owner and
//! retains the exact descriptor across successful and passive-fallback paths.

use open_esp_radio_esp32s31_registers::MacInterruptSetup;
use open_esp_radio_esp32s31_wifi_lmac::tx::{TxCompletion, TxHardware};
use open_esp_radio_esp32s31_wifi_sta::scan::Esp32s31ActiveProbeOutcome;
use open_esp_radio_ieee80211::management::ProbeRequest;

use crate::{
    control_tx::{ControlTxError, Esp32s31ControlTx},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
};

/// Complete inputs for one active-scan Probe Request publication.
pub struct Esp32s31ScanProbeRequest<'a> {
    pub source: [u8; 6],
    pub sequence_number: u16,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
    pub current_channel: Option<u8>,
    pub descriptor_capacity: Option<u32>,
}

/// Detailed terminal observation retained for applications and HIL telemetry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanProbeReport {
    Transmitted(TxCompletion),
    PassiveWithoutAttempt,
    PassiveAfterCompletion(TxCompletion),
    PassiveAfterError(ControlTxError),
}

impl Esp32s31ScanProbeReport {
    pub const fn outcome(self) -> Esp32s31ActiveProbeOutcome {
        match self {
            Self::Transmitted(_) => Esp32s31ActiveProbeOutcome::Transmitted,
            Self::PassiveWithoutAttempt
            | Self::PassiveAfterCompletion(_)
            | Self::PassiveAfterError(_) => Esp32s31ActiveProbeOutcome::PassiveFallback,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanTxSummary {
    pub completions: u32,
    pub failures: u32,
}

/// Shared state machine for cold and running active-probe publication.
pub(crate) struct Esp32s31ScanTxState {
    active_probe_available: bool,
    summary: Esp32s31ScanTxSummary,
}

impl Esp32s31ScanTxState {
    pub(crate) const fn new() -> Self {
        Self {
            active_probe_available: true,
            summary: Esp32s31ScanTxSummary {
                completions: 0,
                failures: 0,
            },
        }
    }

    pub(crate) fn begin_scan(&mut self) {
        self.active_probe_available = true;
        self.summary = Esp32s31ScanTxSummary::default();
    }

    pub(crate) const fn active_probe_available(&self) -> bool {
        self.active_probe_available
    }

    pub(crate) fn classify(
        &mut self,
        result: Result<TxCompletion, ControlTxError>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        match result {
            Ok(completion) => {
                self.summary.completions = self.summary.completions.saturating_add(1);
                if completion.status == 0 {
                    Ok(Esp32s31ScanProbeReport::Transmitted(completion))
                } else {
                    self.summary.failures = self.summary.failures.saturating_add(1);
                    self.active_probe_available = false;
                    Ok(Esp32s31ScanProbeReport::PassiveAfterCompletion(completion))
                }
            }
            Err(error) if error.retains_quiescent_owner() => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Ok(Esp32s31ScanProbeReport::PassiveAfterError(error))
            }
            Err(error) => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Err(error)
            }
        }
    }

    pub(crate) const fn summary(&self) -> Esp32s31ScanTxSummary {
        self.summary
    }
}

/// Polling TX owner for a running rescan after the MAC IRQ epoch is quiesced.
///
/// The connected teardown returns the exact ordinary descriptor and disables
/// both CPU and peripheral interrupt routes before this owner can exist. Probe
/// completion may therefore use the same finite polling transaction as the
/// pre-connected path without racing the connected runner. Re-entering a
/// connected epoch consumes the returned control owner and reactivates IRQs.
pub struct Esp32s31RunningScanTx<'slot, 'interrupt, P, E, T, const BUFFER_SIZE: usize> {
    control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
    state: Esp32s31ScanTxState,
    _interrupts: Option<&'interrupt MacInterruptSetup>,
}

impl<'slot, 'interrupt, P, E, T, const BUFFER_SIZE: usize>
    Esp32s31RunningScanTx<'slot, 'interrupt, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub const fn new(
        control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        interrupt_setup: &'interrupt MacInterruptSetup,
    ) -> Self {
        Self {
            control,
            state: Esp32s31ScanTxState::new(),
            _interrupts: Some(interrupt_setup),
        }
    }

    #[cfg(test)]
    const fn new_for_test(control: Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        Self {
            control,
            state: Esp32s31ScanTxState::new(),
            _interrupts: None,
        }
    }

    pub fn begin_scan(&mut self) {
        self.state.begin_scan();
    }

    pub async fn transmit_probe_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        request: Esp32s31ScanProbeRequest<'_>,
    ) -> Result<Esp32s31ScanProbeReport, ControlTxError> {
        if !self.state.active_probe_available() {
            return Ok(Esp32s31ScanProbeReport::PassiveWithoutAttempt);
        }
        let Esp32s31ScanProbeRequest {
            source,
            sequence_number,
            ssid,
            supported_rates,
            current_channel,
            descriptor_capacity,
        } = request;
        let result = self
            .control
            .transmit_probe_request(
                hardware,
                ProbeRequest {
                    source,
                    sequence_number,
                    ssid,
                    supported_rates,
                },
                current_channel,
                descriptor_capacity,
            )
            .await;
        self.state.classify(result)
    }

    pub fn into_parts(
        self,
    ) -> (
        Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        Esp32s31ScanTxSummary,
    ) {
        (self.control, self.state.summary())
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::{Pin, pin},
        task::{Context, Poll},
    };

    use open_esp_radio_esp32s31_registers::{
        MacHeTxProgram, MacHtTxProgram, MacLegacyTxProgram, MacTxCompletionRegisters,
    };
    use open_esp_radio_esp32s31_wifi_lmac::{tx::TxSlot, tx_runtime::StaTxRuntimePolicy};

    use super::*;
    use crate::{
        control_tx::{ControlTxConfig, WifiTxResources},
        ordinary_tx::WifiTxPowerPair,
    };

    #[derive(Default)]
    struct ScanTxHardware {
        publications: u8,
        completion: Option<MacTxCompletionRegisters>,
    }

    impl TxHardware for ScanTxHardware {
        fn tx_descriptor_address(&self, _cpu_address: u32) -> u32 {
            0x2f00_1000
        }

        fn prepare_legacy_tx(&mut self, _queue: u8, _program: MacLegacyTxProgram) -> bool {
            true
        }

        fn start_legacy_tx(&mut self, _queue: u8, _plcp0: u32) {
            self.publications = self.publications.saturating_add(1);
        }

        fn prepare_ht_tx(&mut self, _queue: u8, _program: MacHtTxProgram) -> bool {
            true
        }

        fn start_ht_tx(&mut self, _queue: u8, _plcp0: u32) {
            self.publications = self.publications.saturating_add(1);
        }

        fn prepare_he_tx(&mut self, _queue: u8, _program: MacHeTxProgram) -> bool {
            false
        }

        fn start_he_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
            self.completion.take()
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            false
        }

        fn finish_tx_timeout_abort(&mut self, _queue: u8) -> Option<bool> {
            None
        }

        fn abort_tx_collision(&mut self, _queue: u8) -> bool {
            false
        }

        fn detach_completed_tx(&mut self, _queue: u8) -> bool {
            true
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

    fn scan_tx_completion(status: u8) -> MacTxCompletionRegisters {
        MacTxCompletionRegisters {
            aux_a: 0,
            aux_b: 0,
            aux_c: 0,
            primary: u32::from(status) << 12,
            alternate: 0,
            trigger_flow: false,
        }
    }

    fn running_scan_tx<'a>(
        slot: Pin<&'a mut TxSlot<256>>,
    ) -> Esp32s31RunningScanTx<'a, 'static, ScanTxPower, fn() -> u32, ScanTxTimer, 256> {
        fn entropy() -> u32 {
            0x1234_5678
        }
        Esp32s31RunningScanTx::new_for_test(Esp32s31ControlTx::new(
            WifiTxResources {
                slot,
                policy: StaTxRuntimePolicy::vendor_defaults(),
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
        let mut slot = pin!(TxSlot::<256>::new());
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
                source: [2, 3, 4, 5, 6, 7],
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
        let mut slot = pin!(TxSlot::<256>::new());
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
}
