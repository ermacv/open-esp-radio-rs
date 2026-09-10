//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use crate::mac_start::{WifiMacReady, WifiMacStartReport};

use oer_esp32s31_hal::{
    owner::{MacInterruptSetup, RadioRuntimeOwner},
    types::MacInterruptEnableState,
};

use oer_esp32s31_phy::{PhyCalibrationCache, RegisteredWifiPhy};

use oer_esp32s31_wifi_mac::sta_ap_registers::disable_all_role_receive_registers;

use oer_ieee80211::channel::WifiChannel;

/// Evidence captured while closing the cold polling interrupt phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiRuntimeTransitionReport {
    /// Mask published by the common MAC initializer before task-side routing.
    pub cold_interrupt_mask: MacInterruptEnableState,
}

/// Common stopped Wi-Fi owner after cold MAC initialization.
///
/// No Wi-Fi role owns DMA or an installed CPU interrupt route in this state.
/// A station, AP or standalone monitor must consume this value to begin its
/// own finite runtime epoch and return the same ownership frontier only after
/// both DMA and interrupt routing have acknowledged their stopped edges.
pub struct WifiStopped<P> {
    platform: P,
    registers: RadioRuntimeOwner,
    interrupt_setup: MacInterruptSetup,
    phy: RegisteredWifiPhy,
    start_report: WifiMacStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl<P> WifiStopped<P> {
    pub const fn start_report(&self) -> WifiMacStartReport {
        self.start_report
    }

    pub const fn transition_report(&self) -> WifiRuntimeTransitionReport {
        self.transition_report
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    pub const fn phy_client_snapshot(&self) -> oer_esp32s31_phy::state::client::PhyClientSnapshot {
        self.phy.client_snapshot()
    }

    /// Borrow the role-neutral radio state for stopped-only operations.
    pub fn radio_mut(&mut self) -> (oer_esp32s31_hal::ieee80211::mac::WifiMacHal<'_>, &mut P) {
        (self.registers.wifi_mac_hal(), &mut self.platform)
    }

    /// Service an elapsed PHY deadline while DMA and IRQ ownership is stopped.
    ///
    /// This consumes the frontier across every await. Failure or cancellation
    /// cannot return a usable stopped owner; reset is required. It does not
    /// stop an active role, wait for a future deadline or restart the MAC.
    pub async fn maintain_phy<
        D: oer_esp32s31_phy::PhyAsyncDelay,
        O: oer_esp32s31_phy::PhyTargetObserver,
    >(
        self,
        clock: &mut impl oer_esp32s31_phy::state::client::PhyPllTrackClock,
        observer: O,
    ) -> Result<
        (
            Self,
            Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
        ),
        WifiMaintenanceFailure<P>,
    > {
        let Self {
            mut platform,
            registers,
            interrupt_setup,
            phy,
            start_report,
            transition_report,
            current_channel,
        } = self;
        let access = match registers.try_into_phy_maintenance(interrupt_setup) {
            Ok(access) => access,
            Err(failure) => {
                return Err(WifiMaintenanceFailure {
                    error: WifiMaintenanceError::Admission(failure.error),
                    _owner: MaintenanceOwner::Stopped {
                        _owner: Self {
                            platform,
                            registers: failure.registers,
                            interrupt_setup: failure.interrupts,
                            phy,
                            start_report,
                            transition_report,
                            current_channel,
                        },
                    },
                });
            }
        };
        let (phy, mut access, outcome) = match phy
            .maintain::<_, D, _, _>(
                &mut platform,
                access,
                oer_esp32s31_phy::WifiPhyMaintenanceRequest::Track,
                clock,
                observer,
            )
            .await
        {
            Ok(success) => success,
            Err(failure) => {
                return Err(WifiMaintenanceFailure {
                    error: WifiMaintenanceError::Phy(failure.error()),
                    _owner: MaintenanceOwner::FailedPhy {
                        _platform: platform,
                        _phy: failure,
                    },
                });
            }
        };
        if outcome.is_some() {
            // Tracking may restore baseband enables. Never resume RX or
            // publish DMA here: the next role owns those transitions.
            disable_all_role_receive_registers(&mut access.wifi_mac_hal());
            access.wifi_mac_hal().request_channel_stop();
        }
        let (registers, interrupt_setup) = match access.try_release() {
            Ok(parts) => parts,
            Err(failure) => {
                return Err(WifiMaintenanceFailure {
                    error: WifiMaintenanceError::Restoration(failure.error),
                    _owner: MaintenanceOwner::FailedRestoration {
                        _platform: platform,
                        _phy: phy,
                        _access: failure,
                    },
                });
            }
        };
        Ok((
            Self {
                platform,
                registers,
                interrupt_setup,
                phy,
                start_report,
                transition_report,
                current_channel,
            },
            outcome,
        ))
    }

    /// Move the exact common ownership frontier into one role runtime.
    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> WifiRuntimeParts<P> {
        WifiRuntimeParts {
            platform: self.platform,
            registers: self.registers,
            interrupt_setup: self.interrupt_setup,
            context: WifiRuntimeContext {
                phy: self.phy,
                start_report: self.start_report,
                transition_report: self.transition_report,
                current_channel: self.current_channel,
            },
        }
    }
}

/// Atomic transfer object between the common stopped owner and one role.
///
/// Keeping PHY, MMIO, interrupt setup and platform ownership together avoids
/// a public constructor which could combine pieces from unrelated epochs.
#[doc(hidden)]
pub struct WifiRuntimeParts<P> {
    pub platform: P,
    pub registers: RadioRuntimeOwner,
    pub interrupt_setup: MacInterruptSetup,
    pub context: WifiRuntimeContext,
}

/// Common Wi-Fi state retained beside one materialized role.
///
/// Register ownership and the interrupt setup token deliberately do not live
/// in this value. A role can reconstruct [`WifiStopped`] only after
/// its DMA/task graph returns the exact runtime owner and its interrupt
/// route returns the exact [`MacInterruptSetup`].
#[doc(hidden)]
pub struct WifiRuntimeContext {
    phy: RegisteredWifiPhy,
    start_report: WifiMacStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl WifiRuntimeContext {
    pub fn phy_mut(&mut self) -> &mut RegisteredWifiPhy {
        &mut self.phy
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.current_channel = channel;
    }

    /// Reconstruct the role-neutral owner from independently proven DMA/task
    /// and interrupt-route return edges.
    pub fn into_stopped<P>(
        self,
        platform: P,
        registers: RadioRuntimeOwner,
        interrupt_setup: MacInterruptSetup,
    ) -> WifiStopped<P> {
        WifiStopped {
            platform,
            registers,
            interrupt_setup,
            phy: self.phy,
            start_report: self.start_report,
            transition_report: self.transition_report,
            current_channel: self.current_channel,
        }
    }
}

/// Unique PHY/platform owner while any finite Wi-Fi role graph is active.
///
/// RX DMA, register and interrupt capabilities move independently into the
/// concrete epoch. The owner is deliberately role-neutral: a same-channel
/// STA+AP graph still has exactly one physical radio and must not manufacture
/// separate station and access-point owners around it.
pub struct WifiRoleOwner<P> {
    platform: P,
    context: WifiRuntimeContext,
}

impl<P> WifiRoleOwner<P> {
    pub fn inspect_phy_tracking(
        &self,
        now_micros: u64,
    ) -> Result<
        oer_esp32s31_phy::tracking::inspection::Inspection,
        oer_esp32s31_phy::state::client::PhyTrackTimeError,
    > {
        self.context.phy.inspect_tracking(now_micros)
    }

    /// Service due PHY tracking while the outer role graph remains paused.
    ///
    /// Consumes the logical radio owner together with admitted physical access.
    /// The caller retains paused RX/TX resources and the detached IRQ route;
    /// neither failure nor cancellation returns a role which can be restarted.
    /// Success still requires MAC restoration and checked access release before
    /// arena publication, RX restoration or IRQ resume. In particular this does
    /// not apply the stopped-role policy which disables both receive interfaces.
    pub async fn maintain_phy<
        D: oer_esp32s31_phy::PhyAsyncDelay,
        O: oer_esp32s31_phy::PhyTargetObserver,
        I: oer_esp32s31_hal::owner::maintenance::InterruptAuthority,
    >(
        self,
        access: oer_esp32s31_hal::owner::maintenance::WifiAccess<I>,
        request: oer_esp32s31_phy::WifiPhyMaintenanceRequest,
        clock: &mut impl oer_esp32s31_phy::state::client::PhyPllTrackClock,
        observer: O,
    ) -> Result<
        (
            Self,
            oer_esp32s31_hal::owner::maintenance::WifiAccess<I>,
            Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
        ),
        WifiRoleMaintenanceFailure<P, I>,
    > {
        let Self {
            mut platform,
            context:
                WifiRuntimeContext {
                    phy,
                    start_report,
                    transition_report,
                    current_channel,
                },
        } = self;
        match phy
            .maintain::<_, D, _, _>(&mut platform, access, request, clock, observer)
            .await
        {
            Ok((phy, access, outcome)) => Ok((
                Self {
                    platform,
                    context: WifiRuntimeContext {
                        phy,
                        start_report,
                        transition_report,
                        current_channel,
                    },
                },
                access,
                outcome,
            )),
            Err(phy) => Err(WifiRoleMaintenanceFailure {
                _platform: platform,
                _start_report: start_report,
                _transition_report: transition_report,
                _current_channel: current_channel,
                phy,
            }),
        }
    }

    pub const fn start_report(&self) -> WifiMacStartReport {
        self.context.start_report
    }

    pub const fn transition_report(&self) -> WifiRuntimeTransitionReport {
        self.context.transition_report
    }

    pub fn radio_mut(&mut self) -> (&mut RegisteredWifiPhy, &mut P) {
        (self.context.phy_mut(), &mut self.platform)
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.context.set_current_channel(channel);
    }

    /// Split the role-neutral logical owner from the platform value while a
    /// concrete role owns the physical register and interrupt epochs.
    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> (P, WifiRuntimeContext) {
        (self.platform, self.context)
    }

    /// Rejoin the exact platform and logical context returned by a role.
    #[doc(hidden)]
    pub fn from_runtime_parts(platform: P, context: WifiRuntimeContext) -> Self {
        Self { platform, context }
    }

    /// Reassemble stopped Wi-Fi only after the exact register and interrupt
    /// setup owners have returned from the active epoch.
    pub fn into_stopped<L>(
        self,
        registers: RadioRuntimeOwner,
        interrupt_setup: MacInterruptSetup,
        resources: L,
    ) -> WifiRoleStopped<P, L> {
        WifiRoleStopped {
            wifi: self
                .context
                .into_stopped(self.platform, registers, interrupt_setup),
            resources,
        }
    }
}

/// Logical role context and failed physical maintenance, retained until reset.
/// No mutable PHY borrow, interrupt capability or role owner can be extracted.
#[must_use = "failed role maintenance retains physical ownership until reset"]
pub struct WifiRoleMaintenanceFailure<
    P,
    I: oer_esp32s31_hal::owner::maintenance::InterruptAuthority,
> {
    _platform: P,
    _start_report: WifiMacStartReport,
    _transition_report: WifiRuntimeTransitionReport,
    _current_channel: WifiChannel,
    phy: oer_esp32s31_phy::WifiPhyMaintenanceFailure<I>,
}

impl<P, I: oer_esp32s31_hal::owner::maintenance::InterruptAuthority>
    WifiRoleMaintenanceFailure<P, I>
{
    pub const fn error(&self) -> oer_esp32s31_phy::WifiPhyMaintenanceError {
        self.phy.error()
    }
}

/// Exact transfer from stopped Wi-Fi into one finite role composition.
pub struct WifiRoleMaterialized<P, L> {
    pub owner: WifiRoleOwner<P>,
    pub registers: RadioRuntimeOwner,
    pub interrupt_setup: MacInterruptSetup,
    pub resources: L,
}

/// Cleanly dematerialized finite role graph.
pub struct WifiRoleStopped<P, L> {
    pub wifi: WifiStopped<P>,
    pub resources: L,
}

/// Consume the common stopped owner before starting any finite role graph.
pub fn materialize_esp32s31_wifi_role<P, L>(
    wifi: WifiStopped<P>,
    resources: L,
) -> WifiRoleMaterialized<P, L> {
    let WifiRuntimeParts {
        platform,
        registers,
        interrupt_setup,
        context,
    } = wifi.into_runtime_parts();
    WifiRoleMaterialized {
        owner: WifiRoleOwner { platform, context },
        registers,
        interrupt_setup,
        resources,
    }
}

/// Close the cold polling phase and enter the reusable stopped-runtime state.
///
/// This is the only normal conversion from [`WifiMacReady`]. It masks
/// and acknowledges cold interrupt state before exposing the setup token used
/// by a finite task-owned interrupt epoch.
pub struct WifiRuntimeStart<P> {
    pub wifi: WifiStopped<P>,
    pub calibration_cache: Option<PhyCalibrationCache>,
}

pub fn enter_esp32s31_wifi_runtime<P>(mut mac: WifiMacReady<P>) -> WifiRuntimeStart<P> {
    let cold_interrupt_mask = { mac.radio_mut().close_cold_interrupt_phase() };
    let (radio, calibration_cache, start_report) = mac.into_parts();
    let (platform, mut registers, interrupt_setup, phy) = radio.into_wifi_runtime_parts();
    // Cold `wifi_set_rx_policy(0)` first publishes both interface addresses,
    // then disables their receive policies. Our cold address transaction is
    // already complete; finish that exact role-neutral suffix before exposing
    // a stopped runtime. Scan/monitor subsequently own only queue three's
    // explicit promiscuous admission, while STA/AP reopen their own context.
    {
        let mut mac = registers.wifi_mac_hal();
        disable_all_role_receive_registers(&mut mac);
        // Complete the role-neutral half of the vendor no-power-save lifecycle.
        // The first role arms its replacement RX ring while this request remains
        // asserted and resumes the frontend only after those credits are live.
        mac.request_channel_stop();
    }
    let current_channel = start_report.wifi.initial_channel;
    WifiRuntimeStart {
        wifi: WifiStopped {
            platform,
            registers,
            interrupt_setup,
            phy,
            start_report,
            transition_report: WifiRuntimeTransitionReport {
                cold_interrupt_mask,
            },
            current_channel,
        },
        calibration_cache,
    }
}

/// Failure reason without exposing a reusable hardware owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiMaintenanceError {
    Admission(oer_esp32s31_hal::owner::maintenance::Error),
    Restoration(oer_esp32s31_hal::owner::maintenance::Error),
    Phy(oer_esp32s31_phy::WifiPhyMaintenanceError),
}

#[allow(
    clippy::large_enum_variant,
    reason = "allocation-free failure retains the physical epoch"
)]
enum MaintenanceOwner<P> {
    Stopped {
        _owner: WifiStopped<P>,
    },
    FailedPhy {
        _platform: P,
        _phy: oer_esp32s31_phy::WifiPhyMaintenanceFailure,
    },
    FailedRestoration {
        _platform: P,
        _phy: RegisteredWifiPhy,
        _access: oer_esp32s31_hal::owner::maintenance::ReleaseFailure,
    },
}

#[must_use = "maintenance failure retains the physical radio and requires reset"]
pub struct WifiMaintenanceFailure<P> {
    _owner: MaintenanceOwner<P>,
    error: WifiMaintenanceError,
}
impl<P> WifiMaintenanceFailure<P> {
    pub const fn error(&self) -> WifiMaintenanceError {
        self.error
    }
}
