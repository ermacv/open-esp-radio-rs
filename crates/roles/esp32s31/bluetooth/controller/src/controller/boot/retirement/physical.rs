//! Consuming last-client RF close and final physical resource reunion.

use super::*;
use oer_esp32s31_bluetooth::resources::{
    BluetoothRadioHardware, BluetoothStopped, TaskResources,
    platform_retirement::ControllerRetiredPlatform,
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothPhysicalReleaseError, BluetoothPhysicalReleaseFailure, InterruptOutputReleasedOwner,
    ModemLpTimerInterruptReadyOwner,
};

/// Software borrows and actual SRAM allocations retained after physical release.
/// These borrows do not authorize reopening the closed HCI epoch or ISR storage.
#[must_use = "retain the old software epoch until its storage can be safely reused"]
pub struct ControllerRetiredStorage<'runtime, S, const SC: usize, const MT: usize> {
    _software: ControllerPublishedTaskService<'runtime, S, SC>,
    _roles: super::super::role_retirement::ControllerRoleResources,
    _memory: oer_esp32s31_bluetooth::ble_phy::BlePhyRetiredMemory,
    _hci: oer_bluetooth_hci::LeControllerHciRetired<'runtime, ()>,
    _timer_runtime:
        oer_esp32s31_bluetooth::runtime_resources::ControllerModemTimerRuntime<'runtime, MT>,
    _timer_storage: &'runtime S,
}

/// Physical cold radio, final calibration state and the retired software epoch.
#[must_use = "the cold radio and retired software storage have separate lifetimes"]
pub struct ControllerColdReleased<'runtime, P, S, const SC: usize, const MT: usize> {
    radio: BluetoothStopped<P>,
    platform_slot: oer_esp32s31_bluetooth::resources::runtime_owner::RuntimeOwnerLease<
        'runtime,
        oer_esp32s31_bluetooth::resources::TeardownPendingPlatform<P>,
    >,
    final_state: oer_esp32s31_phy::PhyState,
    storage: ControllerRetiredStorage<'runtime, S, SC, MT>,
}

impl<'runtime, P, S, const SC: usize, const MT: usize>
    ControllerColdReleased<'runtime, P, S, SC, MT>
{
    /// Return actual cold ownership without reconstructing a static reference.
    /// The old HCI transport stays closed and ISR publication remains reserved.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothStopped<P>,
        oer_esp32s31_phy::PhyState,
        ControllerRetiredStorage<'runtime, S, SC, MT>,
    ) {
        let _ = self.platform_slot;
        (self.radio, self.final_state, self.storage)
    }
}

/// Platform, retained PHY and the retired software epoch after a Controller
/// handed its powered, registered PHY to another protocol route.
#[must_use = "the retained PHY and retired software storage have separate lifetimes"]
pub struct ControllerRetainedReleased<'runtime, P, S, const SC: usize, const MT: usize> {
    platform: P,
    platform_slot: oer_esp32s31_bluetooth::resources::runtime_owner::RuntimeOwnerLease<
        'runtime,
        oer_esp32s31_bluetooth::resources::TeardownPendingPlatform<P>,
    >,
    retained: oer_esp32s31_phy::RetainedPhy,
    storage: ControllerRetiredStorage<'runtime, S, SC, MT>,
}

impl<'runtime, P, S, const SC: usize, const MT: usize>
    ControllerRetainedReleased<'runtime, P, S, SC, MT>
{
    /// Return the platform and the retained PHY for the next route. The old
    /// HCI transport stays closed and ISR publication remains reserved.
    pub fn into_parts(
        self,
    ) -> (
        P,
        oer_esp32s31_phy::RetainedPhy,
        ControllerRetiredStorage<'runtime, S, SC, MT>,
    ) {
        let _ = self.platform_slot;
        (self.platform, self.retained, self.storage)
    }
}

/// First failed edge of physical Controller shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerPhysicalShutdownError {
    Client(oer_esp32s31_phy::state::client::PhyClientReleaseError),
    Rf(oer_esp32s31_phy::PhyTargetPortError),
    Hardware(BluetoothPhysicalReleaseError),
    /// The released root is not described by the closed PHY registration.
    RegistrationMismatch,
}

enum Stage {
    Client {
        _task: TaskResources,
        _output: InterruptOutputReleasedOwner,
        _timer: ModemLpTimerInterruptReadyOwner,
        _failure: oer_esp32s31_phy::RegisteredBluetoothPhyClientReleaseFailure,
    },
    Rf {
        _task: TaskResources,
        _output: InterruptOutputReleasedOwner,
        _timer: ModemLpTimerInterruptReadyOwner,
        _failure: oer_esp32s31_phy::BluetoothPhyRfCloseFailure,
    },
    Hardware {
        _failure: BluetoothPhysicalReleaseFailure,
        _final_state: oer_esp32s31_phy::PhyState,
    },
    RetainedHardware {
        _failure: BluetoothPhysicalReleaseFailure,
        _closed: oer_esp32s31_phy::RegisteredBluetoothPhyRfClosed,
    },
    RetainedPairing {
        _mismatch: oer_esp32s31_phy::RetainedPhyMismatch,
    },
}

/// A failed shutdown retains platform reservation, hardware and all SRAM owners.
#[must_use = "a failed physical shutdown never releases the platform reservation"]
pub struct ControllerPhysicalShutdownFailure<'runtime, P, S, const SC: usize, const MT: usize> {
    error: ControllerPhysicalShutdownError,
    _platform: ControllerRetiredPlatform<'runtime, P>,
    _storage: ControllerRetiredStorage<'runtime, S, SC, MT>,
    _stage: Stage,
}

impl<P, S, const SC: usize, const MT: usize> ControllerPhysicalShutdownFailure<'_, P, S, SC, MT> {
    /// Whether the retained failure has an ambiguous shared-PHY hardware state.
    /// Client rejection precedes RF close; hardware reunion follows proven close.
    /// This fact selects no platform reset or watchdog mechanism.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        match &self._stage {
            Stage::Rf { _failure, .. } => _failure.hardware_ambiguous(),
            Stage::Client { .. }
            | Stage::Hardware { .. }
            | Stage::RetainedHardware { .. }
            | Stage::RetainedPairing { .. } => false,
        }
    }

    pub const fn error(&self) -> ControllerPhysicalShutdownError {
        self.error
    }
}

impl<'runtime, S, const SC: usize> ControllerTaskHciRetired<'runtime, S, SC> {
    /// Close the last PHY client, power down temperature, reset Bluetooth and
    /// release clocks into the original cold radio owner. Output release and
    /// drained timer ownership are mandatory inputs. The platform reservation
    /// must have joined this exact HCI epoch before this call.
    ///
    /// # Cancellation
    /// Drive to a terminal result once polled. Every failure retains the platform
    /// without running its Drop; cancellation never manufactures cold ownership.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete no-allocation physical frontier"
    )]
    pub async fn release_physical<P, D: oer_esp32s31_phy::PhyAsyncDelay, const MT: usize>(
        self,
        timer: super::super::ControllerModemTimerRetired<'runtime, S, MT>,
        output: InterruptOutputReleasedOwner,
        platform: ControllerRetiredPlatform<'runtime, P>,
    ) -> Result<
        ControllerColdReleased<'runtime, P, S, SC, MT>,
        ControllerPhysicalShutdownFailure<'runtime, P, S, SC, MT>,
    > {
        let closed = self.close_rf::<P, D, MT>(timer, output, platform).await?;
        finish_shutdown(closed)
    }

    /// Close the last PHY client like [`Self::release_physical`], then hand
    /// the powered, registered PHY to another protocol route instead of
    /// returning the cold radio.
    ///
    /// Bluetooth resets its Controller domain and releases its own clock
    /// leases; the common PHY power, calibration and registration epoch stay
    /// in effect, so the next route wakes RF without registering again.
    ///
    /// # Cancellation
    /// Drive to a terminal result once polled. Every failure retains the platform
    /// without running its Drop; cancellation never manufactures a retained PHY.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete no-allocation physical frontier"
    )]
    pub async fn release_retained<P, D: oer_esp32s31_phy::PhyAsyncDelay, const MT: usize>(
        self,
        timer: super::super::ControllerModemTimerRetired<'runtime, S, MT>,
        output: InterruptOutputReleasedOwner,
        platform: ControllerRetiredPlatform<'runtime, P>,
    ) -> Result<
        ControllerRetainedReleased<'runtime, P, S, SC, MT>,
        ControllerPhysicalShutdownFailure<'runtime, P, S, SC, MT>,
    > {
        let closed = self.close_rf::<P, D, MT>(timer, output, platform).await?;
        finish_retained_handoff(closed)
    }

    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete no-allocation physical frontier"
    )]
    async fn close_rf<P, D: oer_esp32s31_phy::PhyAsyncDelay, const MT: usize>(
        self,
        timer: super::super::ControllerModemTimerRetired<'runtime, S, MT>,
        output: InterruptOutputReleasedOwner,
        mut platform: ControllerRetiredPlatform<'runtime, P>,
    ) -> Result<
        ClosedController<'runtime, P, S, SC, MT>,
        ControllerPhysicalShutdownFailure<'runtime, P, S, SC, MT>,
    > {
        let Self {
            _software,
            mut _hardware,
            _phy,
            _roles,
            hci,
        } = self;
        let (phy, memory) = _phy.into_shutdown_parts();
        let (timer, timer_runtime, timer_storage) = timer.into_shutdown_parts();
        let storage = ControllerRetiredStorage {
            _software,
            _roles,
            _memory: memory,
            _hci: hci,
            _timer_runtime: timer_runtime,
            _timer_storage: timer_storage,
        };
        let released = match phy.release_phy_client() {
            Ok(phy) => phy,
            Err(failure) => {
                return Err(ControllerPhysicalShutdownFailure {
                    error: ControllerPhysicalShutdownError::Client(failure.error()),
                    _platform: platform,
                    _storage: storage,
                    _stage: Stage::Client {
                        _task: _hardware,
                        _output: output,
                        _timer: timer,
                        _failure: failure,
                    },
                });
            }
        };
        let closed = {
            let mut registers = _hardware.shared_phy_hal();
            released
                .close_rf::<P, D>(platform.platform_mut(), &mut registers)
                .await
        };
        Ok(ClosedController {
            hardware: _hardware,
            output,
            timer,
            platform,
            storage,
            closed,
        })
    }
}

/// Physical owners after the RF-close attempt of the last Bluetooth client.
struct ClosedController<'runtime, P, S, const SC: usize, const MT: usize> {
    hardware: TaskResources,
    output: InterruptOutputReleasedOwner,
    timer: ModemLpTimerInterruptReadyOwner,
    platform: ControllerRetiredPlatform<'runtime, P>,
    storage: ControllerRetiredStorage<'runtime, S, SC, MT>,
    closed: Result<
        oer_esp32s31_phy::RegisteredBluetoothPhyRfClosed,
        oer_esp32s31_phy::BluetoothPhyRfCloseFailure,
    >,
}

// Keep terminal owner assembly and reset/clock release out of the async poll
// frame; both success and failure retain the original allocation-free graph.
#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "terminal failure retains the complete physical frontier"
)]
fn finish_shutdown<'runtime, P, S, const SC: usize, const MT: usize>(
    closed: ClosedController<'runtime, P, S, SC, MT>,
) -> Result<
    ControllerColdReleased<'runtime, P, S, SC, MT>,
    ControllerPhysicalShutdownFailure<'runtime, P, S, SC, MT>,
> {
    let ClosedController {
        hardware: mut _hardware,
        output,
        timer,
        platform,
        storage,
        closed,
    } = closed;
    let closed = match closed {
        Ok(closed) => closed,
        Err(failure) => {
            return Err(ControllerPhysicalShutdownFailure {
                error: ControllerPhysicalShutdownError::Rf(failure.error()),
                _platform: platform,
                _storage: storage,
                _stage: Stage::Rf {
                    _task: _hardware,
                    _output: output,
                    _timer: timer,
                    _failure: failure,
                },
            });
        }
    };
    oer_esp32s31_hal::phy::temperature::power_down(&mut _hardware.shared_phy_hal());
    let final_state = closed.into_retired_state();
    let hardware = match _hardware.release_after_phy_close(output, timer) {
        Ok(hardware) => hardware,
        Err(failure) => {
            return Err(ControllerPhysicalShutdownFailure {
                error: ControllerPhysicalShutdownError::Hardware(failure.error()),
                _platform: platform,
                _storage: storage,
                _stage: Stage::Hardware {
                    _failure: failure,
                    _final_state: final_state,
                },
            });
        }
    };
    let (platform, platform_slot) = platform.into_platform_after_shutdown();
    Ok(ControllerColdReleased {
        platform_slot,
        radio: BluetoothStopped::from_hardware(
            platform,
            BluetoothRadioHardware::from_released(hardware),
        ),
        final_state,
        storage,
    })
}

// Like `finish_shutdown`, keep terminal owner assembly out of the async frame.
#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "terminal failure retains the complete physical frontier"
)]
fn finish_retained_handoff<'runtime, P, S, const SC: usize, const MT: usize>(
    closed: ClosedController<'runtime, P, S, SC, MT>,
) -> Result<
    ControllerRetainedReleased<'runtime, P, S, SC, MT>,
    ControllerPhysicalShutdownFailure<'runtime, P, S, SC, MT>,
> {
    let ClosedController {
        mut hardware,
        output,
        timer,
        platform,
        storage,
        closed,
    } = closed;
    let closed = match closed {
        Ok(closed) => closed,
        Err(failure) => {
            return Err(ControllerPhysicalShutdownFailure {
                error: ControllerPhysicalShutdownError::Rf(failure.error()),
                _platform: platform,
                _storage: storage,
                _stage: Stage::Rf {
                    _task: hardware,
                    _output: output,
                    _timer: timer,
                    _failure: failure,
                },
            });
        }
    };
    oer_esp32s31_hal::phy::temperature::power_down(&mut hardware.shared_phy_hal());
    let root = match hardware.release_retained_after_phy_close(output, timer) {
        Ok(root) => root,
        Err(failure) => {
            return Err(ControllerPhysicalShutdownFailure {
                error: ControllerPhysicalShutdownError::Hardware(failure.error()),
                _platform: platform,
                _storage: storage,
                _stage: Stage::RetainedHardware {
                    _failure: failure,
                    _closed: closed,
                },
            });
        }
    };
    let retained = match oer_esp32s31_phy::RetainedPhy::from_bluetooth(root, closed) {
        Ok(retained) => retained,
        Err(mismatch) => {
            return Err(ControllerPhysicalShutdownFailure {
                error: ControllerPhysicalShutdownError::RegistrationMismatch,
                _platform: platform,
                _storage: storage,
                _stage: Stage::RetainedPairing {
                    _mismatch: mismatch,
                },
            });
        }
    };
    let (platform, platform_slot) = platform.into_platform_after_shutdown();
    Ok(ControllerRetainedReleased {
        platform,
        platform_slot,
        retained,
        storage,
    })
}

mod restart;
pub use restart::{ControllerRestartError, ControllerRestartFailure, ControllerRestarted};
