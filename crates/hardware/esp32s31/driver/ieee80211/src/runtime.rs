//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use crate::mac_start::{WifiMacReady, WifiMacStartReport};

use oer_esp32s31_hal::{
    owner::{MacInterruptSetup, RadioRuntimeOwner},
    types::MacInterruptEnableState,
};

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::owner::{Radio, state as radio_state};

use oer_esp32s31_phy::{
    PhyCalibrationCache, RegisteredPhyClientReleaseDisposition, RegisteredWifiPhy,
};

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_phy::{
    RegisteredPhyClientAcquireFailure, RegisteredPhyColdReleaseFailure, RegisteredPhyColdReleased,
    RegisteredPhyPendingTrack, RegisteredPhyRadio, RegisteredPhyRetainedReleaseFailure,
    RegisteredPhyRfCloseFailure, RegisteredPhyRfWakePoisoned, RetainedPhy,
};

use oer_esp32s31_ieee80211_mac::sta_ap_registers::disable_all_role_receive_registers;

use oer_ieee80211_mac::channel::WifiChannel;

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

    /// Retire the Wi-Fi PHY client from this fully stopped runtime frontier.
    ///
    /// The runtime register and inactive IRQ owners are reunited before the
    /// client result is attached to the physical radio. Wi-Fi RX is disabled
    /// at the vendor-shaped per-client edge. The returned last-client owner is
    /// still physically powered; RF close and shared-clock release are later
    /// whole-radio lifecycle transactions.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "failure must return the complete stopped Wi-Fi frontier"
    )]
    pub fn release_phy_client(
        self,
    ) -> Result<RegisteredPhyClientReleaseDisposition<P>, WifiPhyClientReleaseFailure<P>> {
        let Self {
            platform,
            registers,
            interrupt_setup,
            phy,
            start_report,
            transition_report,
            current_channel,
        } = self;
        let (registers, interrupt_setup) = match registers.try_confirm_phy_stopped(interrupt_setup)
        {
            Ok(frontier) => frontier,
            Err(failure) => {
                return Err(WifiPhyClientReleaseFailure {
                    error: WifiPhyClientReleaseError::Hardware(failure.error),
                    owner: Self {
                        platform,
                        registers: failure.registers,
                        interrupt_setup: failure.interrupts,
                        phy,
                        start_report,
                        transition_report,
                        current_channel,
                    },
                });
            }
        };
        let mut radio = Radio::<P, radio_state::Running>::from_runtime_parts(
            platform,
            registers,
            interrupt_setup,
        )
        .reunite_powered();
        radio.disable_wifi_rx();
        match phy.release_wifi_client(radio) {
            Ok(release) => Ok(release.into_disposition()),
            Err(failure) => {
                let error = WifiPhyClientReleaseError::Phy(failure.error());
                let (phy, mut radio) = failure.into_parts();
                radio.enable_wifi_rx();
                let (platform, registers, interrupt_setup) =
                    radio.into_running().into_runtime_parts();
                Err(WifiPhyClientReleaseFailure {
                    error,
                    owner: Self {
                        platform,
                        registers,
                        interrupt_setup,
                        phy,
                        start_report,
                        transition_report,
                        current_channel,
                    },
                })
            }
        }
    }

    /// Release the stopped Wi-Fi client and close the shared RF domain when it
    /// was the final protocol client.
    ///
    /// A non-final release returns the still-registered shared radio without
    /// touching its physical RF state. A final release executes the complete
    /// source-owned RF close and cold-owner reunion, returning a cache captured
    /// from the final post-maintenance calibration state.
    ///
    /// # Cancellation
    ///
    /// Once the final-client close future is polled, it must be driven to a
    /// terminal result. Dropping it can strand a temperature transaction or a
    /// partially closed RF epoch.
    #[cfg(target_arch = "riscv32")]
    #[must_use = "radio release must reach an owned shared or cold disposition"]
    pub async fn release_radio<D: oer_esp32s31_phy::PhyAsyncDelay>(
        self,
    ) -> Result<WifiRadioReleaseDisposition<P>, WifiRadioReleaseFailure<P>> {
        match self
            .release_phy_client()
            .map_err(WifiRadioReleaseFailure::Client)?
        {
            RegisteredPhyClientReleaseDisposition::Remaining(radio) => {
                Ok(WifiRadioReleaseDisposition::Shared(radio))
            }
            RegisteredPhyClientReleaseDisposition::Last(idle) => {
                let closed = idle
                    .close_rf::<D>()
                    .await
                    .map_err(WifiRadioReleaseFailure::RfClose)?;
                let cold = closed
                    .release_to_cold()
                    .map_err(WifiRadioReleaseFailure::ColdRelease)?;
                Ok(WifiRadioReleaseDisposition::Cold(cold))
            }
        }
    }

    /// Release the stopped last Wi-Fi client, close RF and hand the powered,
    /// registered PHY to another protocol route.
    ///
    /// This is the protocol-switch counterpart of [`Self::release_radio`]:
    /// the registration epoch, calibration state and common PHY power stay in
    /// effect, so the next route wakes RF instead of registering again. A
    /// release that leaves another client returns the shared radio unchanged.
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result. Dropping it can
    /// strand a temperature transaction or a partially closed RF epoch.
    #[cfg(target_arch = "riscv32")]
    #[must_use = "retained release must reach an owned retained PHY or failure frontier"]
    pub async fn release_retained<D: oer_esp32s31_phy::PhyAsyncDelay>(
        self,
    ) -> Result<(P, RetainedPhy), WifiRadioRetainedReleaseFailure<P>> {
        let idle = match self
            .release_phy_client()
            .map_err(WifiRadioRetainedReleaseFailure::Client)?
        {
            RegisteredPhyClientReleaseDisposition::Last(idle) => idle,
            RegisteredPhyClientReleaseDisposition::Remaining(radio) => {
                return Err(WifiRadioRetainedReleaseFailure::Shared(radio));
            }
        };
        let closed = idle
            .close_rf::<D>()
            .await
            .map_err(WifiRadioRetainedReleaseFailure::RfClose)?;
        closed
            .release_retained()
            .map_err(WifiRadioRetainedReleaseFailure::Retained)
    }

    /// Close and immediately restore the RF domain without retiring the
    /// registered calibration epoch.
    ///
    /// This is the basic retained-wake lifecycle transaction. It is admitted
    /// only from the fully stopped Wi-Fi frontier, releases the final Wi-Fi
    /// client, executes physical close and wake, reacquires that client and
    /// reconstructs the same stopped runtime ownership boundary.
    #[cfg(target_arch = "riscv32")]
    #[must_use = "retained RF cycle must return a stopped owner or a fail-stop frontier"]
    pub async fn cycle_retained_rf<
        D: oer_esp32s31_phy::PhyAsyncDelay,
        C: oer_esp32s31_phy::state::client::PhyPllTrackClock,
    >(
        self,
        clock: &mut C,
    ) -> Result<Self, WifiRadioRetainedCycleFailure<P>> {
        let start_report = self.start_report;
        let transition_report = self.transition_report;
        let current_channel = self.current_channel;
        let idle = match self
            .release_phy_client()
            .map_err(WifiRadioRetainedCycleFailure::Client)?
        {
            RegisteredPhyClientReleaseDisposition::Last(idle) => idle,
            RegisteredPhyClientReleaseDisposition::Remaining(radio) => {
                return Err(WifiRadioRetainedCycleFailure::Shared(radio));
            }
        };
        let closed = idle
            .close_rf::<D>()
            .await
            .map_err(WifiRadioRetainedCycleFailure::RfClose)?;
        let idle = closed
            .wake_rf::<D>()
            .await
            .map_err(WifiRadioRetainedCycleFailure::RfWake)?;
        let acquire = idle
            .retain_powered()
            .acquire_client(clock)
            .map_err(WifiRadioRetainedCycleFailure::Acquire)?;
        let mut radio = acquire
            .into_owner()
            .map_err(WifiRadioRetainedCycleFailure::Tracking)?;
        radio.enable_wifi_rx_after_retained_wake();
        let (platform, mut registers, interrupt_setup, phy) = radio.into_wifi_runtime_parts();
        {
            let mut mac = registers.wifi_mac_hal();
            disable_all_role_receive_registers(&mut mac);
            mac.request_channel_stop();
        }
        Ok(Self {
            platform,
            registers,
            interrupt_setup,
            phy,
            start_report,
            transition_report,
            current_channel,
        })
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
            Option<oer_esp32s31_phy::tracking::PhyParamTrackingOutcome>,
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

/// Whole-radio disposition after a stopped Wi-Fi client is released.
#[cfg(target_arch = "riscv32")]
#[must_use = "the returned owner is the only authority for the next radio epoch"]
pub enum WifiRadioReleaseDisposition<P> {
    /// Another protocol client keeps the shared registered PHY powered.
    Shared(RegisteredPhyRadio<P>),
    /// Wi-Fi was the last client and the RF domain reached the cold frontier.
    Cold(RegisteredPhyColdReleased<P>),
}

/// Fail-stop frontier for an immediate retained RF close/wake cycle.
#[cfg(target_arch = "riscv32")]
#[must_use = "retained lifecycle failure owns the only recoverable or poisoned frontier"]
pub enum WifiRadioRetainedCycleFailure<P> {
    Client(WifiPhyClientReleaseFailure<P>),
    Shared(RegisteredPhyRadio<P>),
    RfClose(RegisteredPhyRfCloseFailure<P>),
    RfWake(RegisteredPhyRfWakePoisoned<P>),
    Acquire(RegisteredPhyClientAcquireFailure<P>),
    Tracking(RegisteredPhyPendingTrack<P>),
}

/// Frontier of a stopped-Wi-Fi handoff that did not reach the retained PHY.
#[cfg(target_arch = "riscv32")]
#[must_use = "retained release failure retains the exact recoverable or poisoned owner"]
pub enum WifiRadioRetainedReleaseFailure<P> {
    /// Wi-Fi client release failed before physical RF close was selected.
    Client(WifiPhyClientReleaseFailure<P>),
    /// Another protocol client keeps the shared registered PHY powered.
    Shared(RegisteredPhyRadio<P>),
    /// RF-close preparation failed recoverably or physical close was poisoned.
    RfClose(RegisteredPhyRfCloseFailure<P>),
    /// RF was closed, but the retained root rejected the handoff before MMIO.
    Retained(RegisteredPhyRetainedReleaseFailure<P>),
}

#[cfg(target_arch = "riscv32")]
impl<P> WifiRadioRetainedReleaseFailure<P> {
    /// Whether RF close left an ambiguous PHY epoch.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        match self {
            Self::RfClose(RegisteredPhyRfCloseFailure::Started(_)) => true,
            Self::Client(_)
            | Self::Shared(_)
            | Self::RfClose(RegisteredPhyRfCloseFailure::Preparation(_))
            | Self::Retained(_) => false,
        }
    }
}

/// Fail-stop frontier for the composed stopped-Wi-Fi release transaction.
#[cfg(target_arch = "riscv32")]
#[must_use = "release failure retains the exact recoverable or poisoned owner"]
pub enum WifiRadioReleaseFailure<P> {
    /// Wi-Fi client release failed before physical RF close was selected.
    Client(WifiPhyClientReleaseFailure<P>),
    /// RF-close preparation failed recoverably or physical close was poisoned.
    RfClose(RegisteredPhyRfCloseFailure<P>),
    /// RF was closed, but the cold PAC ownership reunion failed.
    ColdRelease(RegisteredPhyColdReleaseFailure<P>),
}

#[cfg(target_arch = "riscv32")]
impl<P> WifiRadioReleaseFailure<P> {
    /// Whether RF close left an ambiguous PHY epoch. Client preflight preserves
    /// its stopped owner; cold reunion failure retains already-closed RF.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        match self {
            Self::RfClose(RegisteredPhyRfCloseFailure::Started(_)) => true,
            Self::Client(_)
            | Self::RfClose(RegisteredPhyRfCloseFailure::Preparation(_))
            | Self::ColdRelease(_) => false,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> WifiRadioRetainedCycleFailure<P> {
    /// Whether close or wake poisoned the shared PHY. A pending tracking owner
    /// is unexecuted work, not a failed tracking transaction. No branch grants
    /// runnable Wi-Fi ownership merely because this observation is false.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        match self {
            Self::RfClose(RegisteredPhyRfCloseFailure::Started(_)) | Self::RfWake(_) => true,
            Self::Client(_)
            | Self::Shared(_)
            | Self::RfClose(RegisteredPhyRfCloseFailure::Preparation(_))
            | Self::Acquire(_)
            | Self::Tracking(_) => false,
        }
    }
}

/// Failed stopped-frontier PHY client release retaining all Wi-Fi owners.
#[must_use = "failed PHY client release retains the complete stopped Wi-Fi frontier"]
pub struct WifiPhyClientReleaseFailure<P> {
    error: WifiPhyClientReleaseError,
    owner: WifiStopped<P>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPhyClientReleaseError {
    Hardware(oer_esp32s31_hal::owner::maintenance::Error),
    Phy(oer_esp32s31_phy::RegisteredWifiPhyClientReleaseError),
}

impl<P> WifiPhyClientReleaseFailure<P> {
    pub const fn error(&self) -> WifiPhyClientReleaseError {
        self.error
    }

    pub fn into_owner(self) -> WifiStopped<P> {
        self.owner
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
            Option<oer_esp32s31_phy::tracking::PhyParamTrackingOutcome>,
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
