//! Automatic PHY handoff joins the actor, timer, IRQ bank and matching platform.
//! Active DTM has no authority variant. Hard deferral failure closes HCI and
//! invalidates the shared PHY epoch; the current backend escalates to full SoC
//! reset because local active-RF shutdown is unproven. It never pauses a test.

use super::*;
use oer_esp32s31_bluetooth::{
    controller::{ControllerIdleCommandTask, SchedulerRunInterruptStorage},
    le::peripheral::PeripheralPhyMaintenanceReady,
    resources::platform_retirement::ControllerRuntimePlatform,
};
use oer_esp32s31_bluetooth_runtime::controller::maintenance::PhyMaintenancePolicy;
use oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome;

#[allow(
    clippy::large_enum_variant,
    reason = "one exact no-alloc physical authority"
)]
enum Authority<const SC: usize> {
    Idle(ControllerIdleCommandTask<'static, PublishedStorage, SC>),
    Peripheral(PeripheralPhyMaintenanceReady<'static, PublishedStorage, SC>),
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>
{
    /// Service automatic maintenance forever with explicit, measured time budgets.
    /// The runner must start idle. HCI stays open across successful maintenance;
    /// Hard deadline, clock, physical or restoration failures enter typed shared-RF
    /// fail-stop. The current ESP32-S31 backend escalates to full SoC reset.
    /// Keep this consuming future pinned through completion, including PHY work.
    pub async fn run_with_phy_maintenance<P: 'static>(
        self,
        platform: &mut ControllerRuntimePlatform<'static, P>,
        policy: PhyMaintenancePolicy,
    ) -> ! {
        let _runner = self
            .run_with_phy_maintenance_until_idle(platform, policy, core::future::pending())
            .await;
        unreachable!("the permanent maintenance runner has no stop request")
    }

    /// Automatic maintenance until a stop request and normal idle/drain admission.
    /// The stop request neither ends DTM nor disconnects ACL. It stays latched
    /// across maintenance. Returned live runners must continue through a
    /// maintenance-aware entry to keep this configured policy enforced.
    pub async fn run_with_phy_maintenance_until_idle<P: 'static>(
        mut self,
        platform: &mut ControllerRuntimePlatform<'static, P>,
        policy: PhyMaintenancePolicy,
        request: impl Future<Output = ()>,
    ) -> Self {
        if let Err(error) = self
            .command
            .as_mut()
            .expect("live actor")
            .enable_phy_maintenance(policy)
        {
            quarantine(self, error);
        }
        let mut request = pin!(request);
        let mut requested = false;
        loop {
            let maintenance = {
                let stop = async {
                    if !requested {
                        request.as_mut().await;
                        requested = true;
                    }
                };
                let mut running = pin!(self.drive_until_idle(stop, true));
                poll_fn(|cx| poll_idle_handoff(running.as_mut(), cx)).await
            };
            if !maintenance {
                return self;
            }
            self = self.maintain_due_phy(platform).await;
        }
    }

    async fn maintain_due_phy<P: 'static>(
        mut self,
        platform: &mut ControllerRuntimePlatform<'static, P>,
    ) -> Self {
        let deadline = self
            .command
            .as_ref()
            .expect("live actor")
            .phy_maintenance_idle_deadline();
        let command = self
            .command
            .take()
            .expect("maintenance retains the original actor");
        let (continuation, authority) = if command.peripheral_phy_maintenance_ready() {
            match command.take_peripheral_phy_maintenance() {
                Ok((continuation, ready)) => (continuation, Authority::Peripheral(ready)),
                Err(failure) => quarantine(self, failure),
            }
        } else {
            if deadline.is_none() {
                quarantine(self, command);
            }
            match command.take_idle_phy_maintenance() {
                Ok((continuation, idle)) => (continuation, Authority::Idle(idle)),
                Err(failure) => quarantine(self, failure),
            }
        };
        let (admitted, execution, restoration) = match &authority {
            Authority::Idle(_) => (
                deadline.unwrap().started_at_micros(),
                Some(deadline.unwrap().expires_at_micros()),
                None,
            ),
            Authority::Peripheral(ready) => (
                ready.execution_deadline().started_at_micros(),
                Some(ready.execution_deadline().expires_at_micros()),
                Some(ready.restoration_deadline().expires_at_micros()),
            ),
        };
        crate::maintenance_observation::begin(admitted, execution, restoration);
        let protection = self.watchdog.maintenance(
            restoration
                .or(execution)
                .map(|at| at.saturating_sub(embassy_time::Instant::now().as_micros())),
        );
        let interrupt = match self.interrupt.take().expect("live IRQ owner").disable() {
            Ok(interrupt) => interrupt,
            Err(failure) => quarantine(self, (continuation, authority, failure)),
        };
        if let Some(fault) = interrupt.fault() {
            quarantine(self, (continuation, authority, interrupt, fault));
        }
        let timer = match self
            .modem_timer
            .take()
            .expect("live timer owner")
            .try_retire()
        {
            Ok(timer) => timer,
            Err(failure) => quarantine(self, (continuation, authority, interrupt, failure)),
        };
        let registers = match interrupt.retire_registers() {
            Ok(registers) => registers,
            Err(error) => quarantine(self, (continuation, authority, interrupt, timer, error)),
        };
        crate::maintenance_observation::quiesced();
        let peripheral = matches!(authority, Authority::Peripheral(_));
        let mut clock = crate::EmbassyPhyTime;
        let outcome: Option<PhyParamTrackingOutcome>;
        let actor = match authority {
            Authority::Idle(idle) => {
                let result = {
                    let mut work = pin!(
                        registers
                            .maintain_phy::<P, _, crate::EmbassyPhyTime, _, SC, MT, H2C, C2H, PC>(
                                idle,
                                timer,
                                platform,
                                &mut self.controller,
                                &mut clock,
                                crate::maintenance_observation::Observer,
                                deadline
                            )
                    );
                    poll_fn(|cx| poll_maintenance(work.as_mut(), cx)).await
                };
                let maintained = match result {
                    Ok(maintained) => maintained,
                    Err(failure) => quarantine(self, (continuation, interrupt, failure)),
                };
                self.modem_timer = Some(maintained.timer);
                outcome = maintained.outcome;
                crate::maintenance_observation::physical(outcome);
                continuation.resume_idle(maintained.task)
            }
            Authority::Peripheral(ready) => {
                let result = {
                    let mut work = pin!(registers.maintain_peripheral_phy::<P, _, crate::EmbassyPhyTime, _, SC, MT, H2C, C2H, PC>(
                    ready, timer, platform, &mut self.controller, &mut clock, crate::maintenance_observation::Observer
                ));
                    poll_fn(|cx| poll_maintenance(work.as_mut(), cx)).await
                };
                let maintained = match result {
                    Ok(maintained) => maintained,
                    Err(failure) => quarantine(self, (continuation, interrupt, failure)),
                };
                self.modem_timer = Some(maintained.timer);
                outcome = maintained.outcome;
                crate::maintenance_observation::physical(outcome);
                continuation.resume_peripheral(maintained.task)
            }
        };
        self.command = Some(match actor {
            Ok(actor) => actor,
            Err(failure) => quarantine(self, (interrupt, failure)),
        });
        if let Err(error) = self.recheck.reanchor_after_idle() {
            quarantine(self, (interrupt, error));
        }
        self.interrupt = Some(match interrupt.bind() {
            Ok(interrupt) => interrupt,
            Err(failure) => quarantine(self, failure),
        });
        if peripheral {
            self.restoration_protection = Some(protection);
        } else {
            crate::WatchdogConfig::complete(protection);
        }
        crate::diagnostics::record(
            crate::diagnostics::BluetoothExecutionEvent::PhyMaintenance { peripheral },
            format_args!(
                "automatic PHY maintenance: {:?}; at={}",
                outcome,
                PublishedStorage::monotonic_micros()
            ),
        );
        self
    }
}

#[inline(never)]
fn poll_maintenance<F: Future>(
    future: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> Poll<F::Output> {
    future.poll(cx)
}

#[inline(never)]
fn quarantine<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
    T,
>(
    mut runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    owners: T,
) -> ! {
    crate::diagnostics::record(
        crate::diagnostics::BluetoothExecutionEvent::Terminal,
        format_args!("automatic PHY maintenance quarantine"),
    );
    runner.controller.close_transport();
    super::fail_stop_shared_phy(
        oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop::MaintenanceFailed,
        (runner, owners),
    )
}
