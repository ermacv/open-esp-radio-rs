//! Diagnostic storage is outside the nested PHY future and borrowed per event.
use oer_esp32s31_phy::PhyTargetObserver;
#[cfg(feature = "diagnostics")]
use oer_esp32s31_phy::tracking::observation::Report;

#[derive(Default)]
pub(super) struct Storage {
    #[cfg(feature = "diagnostics")]
    recorder: core::cell::RefCell<oer_esp32s31_phy::tracking::observation::Recorder>,
}

impl Storage {
    /// Claim once with the station checkpoint. Separate static storage avoids
    /// moving the recorder together with the complete paused runner at startup.
    pub fn initialize() -> &'static Self {
        static STORAGE: static_cell::StaticCell<Storage> = static_cell::StaticCell::new();
        STORAGE.init_with(Self::default)
    }

    // Initialize before entering the hardware call chain; do not reserve this
    // temporary recorder in the enclosing async poll frame during calibration.
    #[inline(never)]
    pub fn reset(&self) {
        #[cfg(feature = "diagnostics")]
        {
            *self.recorder.borrow_mut() = Default::default();
        }
    }

    #[cfg(feature = "diagnostics")]
    pub fn report(&self) -> Option<Report> {
        Some(self.recorder.borrow().report())
    }

    pub fn observer(&self) -> impl PhyTargetObserver + '_ {
        #[cfg(feature = "diagnostics")]
        {
            Observer(&self.recorder)
        }
        #[cfg(not(feature = "diagnostics"))]
        {
            oer_esp32s31_phy::NoopPhyTargetObserver
        }
    }
}

#[cfg(feature = "diagnostics")]
struct Observer<'a>(&'a core::cell::RefCell<oer_esp32s31_phy::tracking::observation::Recorder>);

#[cfg(feature = "diagnostics")]
impl PhyTargetObserver for Observer<'_> {
    const OBSERVE_RFPLL_AGE: bool = true;
    const OBSERVE_DELAYS: bool = true;
    #[inline(never)]
    fn rfpll_completed(&mut self, observation: oer_esp32s31_phy::tracking::rfpll::Observation) {
        self.0.borrow_mut().observe_rfpll(observation);
    }
    #[inline(never)]
    fn tx_wait(
        &mut self,
        scope: oer_esp32s31_phy::executor::wait::tx::Scope,
        kind: oer_esp32s31_phy::executor::wait::Kind,
        event: oer_esp32s31_phy::executor::wait::Event,
    ) {
        self.0.borrow_mut().observe_tx_wait(scope, kind, event);
    }

    #[inline(never)]
    fn tx_sar_ready(&mut self, ready: bool) {
        self.0.borrow_mut().observe_tx_sar_ready(ready);
    }

    #[inline(never)]
    fn dcode_wait(
        &mut self,
        scope: oer_esp32s31_phy::executor::wait::Scope,
        kind: oer_esp32s31_phy::executor::wait::Kind,
        event: oer_esp32s31_phy::executor::wait::Event,
    ) {
        self.0.borrow_mut().observe_dcode_wait(scope, kind, event);
    }

    #[inline(never)]
    fn dcode_pll_lock(&mut self, locked: bool) {
        self.0.borrow_mut().observe_dcode_pll_lock(locked);
    }

    #[inline(never)]
    fn rx_gain_execution(
        &mut self,
        execution: oer_esp32s31_phy::tracking::observation::RxGainExecution,
    ) {
        self.0.borrow_mut().observe_rx_gain_execution(execution);
    }

    // Keep diagnostic clock/accounting locals out of the large PHY poll frame.
    #[inline(never)]
    fn tracking_operation(
        &mut self,
        operation: oer_esp32s31_phy::tracking::observation::Operation,
        event: oer_esp32s31_phy::tracking::observation::Event,
    ) {
        self.0
            .borrow_mut()
            .observe(operation, event, embassy_time::Instant::now().as_micros());
    }
}
