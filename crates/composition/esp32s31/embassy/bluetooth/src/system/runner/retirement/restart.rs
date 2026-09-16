//! Rebind the same ISR service after checked physical cold initialization.

use super::*;
use crate::{
    BluetoothInterruptBindError, BluetoothInterruptFault, BluetoothRunners, BluetoothSystem,
    BluetoothSystemReady,
};
use oer_esp32s31_bluetooth::controller::{
    ControllerRestartError, ControllerRestartFailure, ControllerRestarted,
};
use oer_esp32s31_bluetooth_embassy::controller::DtmRecheckStartError;
use oer_esp32s31_radio_platform_esp_hal::EspHalBluetoothInterruptStorageError;

type Restarted<
    P,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> = ControllerRestarted<
    'static,
    P,
    PublishedStorage,
    CriticalSectionRawMutex,
    SC,
    MT,
    H2C,
    C2H,
    PC,
>;

/// Exact rejection of a powered restart or final route activation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHardwareRestartError {
    PriorInterruptFault(BluetoothInterruptFault),
    Controller(ControllerRestartError<EspHalBluetoothInterruptStorageError>),
    Recheck(DtmRecheckStartError),
    Routes(BluetoothInterruptBindError),
}

#[allow(
    clippy::large_enum_variant,
    reason = "complete failed restart remains allocation-free"
)]
enum Retained<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    Cold(BluetoothHardwareColdReleased<P, MT, SC, H2C, C2H, PC>),
    Hardware {
        _owner: ControllerRestartFailure<'static, P, PublishedStorage, SC, MT>,
        _bindings: RetiredBindings<H2C, C2H, PC>,
    },
    Initialized {
        _owner: Restarted<P, MT, SC, H2C, C2H, PC>,
        _bindings: RetiredBindings<H2C, C2H, PC>,
    },
}

/// Complete failed restart, retaining original memory and all hardware owners.
#[must_use = "failed restart is retained until explicit recovery or board reset"]
pub struct BluetoothHardwareRestartFailure<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    error: BluetoothHardwareRestartError,
    _retained: Retained<P, MT, SC, H2C, C2H, PC>,
}
impl<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> BluetoothHardwareRestartFailure<P, MT, SC, H2C, C2H, PC>
{
    pub const fn error(&self) -> BluetoothHardwareRestartError {
        self.error
    }
}

impl<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> BluetoothHardwareColdReleased<P, MT, SC, H2C, C2H, PC>
{
    /// Reinitialize the returned radio using the original allocation graph and
    /// statically borrowed software slots. No allocator or new StaticCell claim
    /// is used. Old Host handles remain closed while a new Host facade receives
    /// the next HCI generation. All three IRQ routes activate only after both
    /// register owners and the complete task/timer/HCI state are restored.
    ///
    /// Once polled, drive to completion; cancellation does not release powered
    /// reservations. A retained ISR fault rejects restart before initialization.
    pub async fn restart(
        self,
        config: oer_esp32s31_bluetooth::phy::PhyInitializationConfig,
    ) -> Result<
        BluetoothSystemReady<P, MT, SC, H2C, C2H, PC>,
        BluetoothHardwareRestartFailure<P, MT, SC, H2C, C2H, PC>,
    > {
        if let Some(fault) = self._bindings.4.fault() {
            return Err(BluetoothHardwareRestartFailure {
                error: BluetoothHardwareRestartError::PriorInterruptFault(fault),
                _retained: Retained::Cold(self),
            });
        }
        let Self {
            owner,
            mut _bindings,
        } = self;
        let mut clock = crate::EmbassyPhyTime;
        let outcome = {
            let mut restart =
                core::pin::pin!(owner.restart::<crate::EmbassyPhyTime, _, H2C, C2H, PC>(
                    &mut _bindings.0,
                    config,
                    &mut clock
                ));
            core::future::poll_fn(|context| poll_physical_release(restart.as_mut(), context)).await
        };
        finish_outcome(outcome, _bindings)
    }
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "failed activation retains complete initialized owners"
)]
fn finish<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    owner: Restarted<P, MT, SC, H2C, C2H, PC>,
    mut bindings: RetiredBindings<H2C, C2H, PC>,
) -> Result<
    BluetoothSystemReady<P, MT, SC, H2C, C2H, PC>,
    BluetoothHardwareRestartFailure<P, MT, SC, H2C, C2H, PC>,
> {
    if let Err(error) = bindings.2.reanchor_after_idle() {
        return Err(BluetoothHardwareRestartFailure {
            error: BluetoothHardwareRestartError::Recheck(error),
            _retained: Retained::Initialized {
                _owner: owner,
                _bindings: bindings,
            },
        });
    }
    let (controller, modem_driver, recheck, wakers, interrupt) = bindings;
    let interrupt = match interrupt.bind() {
        Ok(interrupt) => interrupt,
        Err(failure) => {
            let (error, interrupt) = failure.into_parts();
            return Err(BluetoothHardwareRestartFailure {
                error: BluetoothHardwareRestartError::Routes(error),
                _retained: Retained::Initialized {
                    _owner: owner,
                    _bindings: (controller, modem_driver, recheck, wakers, interrupt),
                },
            });
        }
    };
    let ControllerRestarted {
        task,
        timer,
        platform,
        host,
    } = owner;
    let host_acl_credits = host.acl_credit_sender();
    Ok(BluetoothSystemReady {
        platform,
        system: BluetoothSystem {
            hci: bt_hci::controller::ExternalController::new(host),
            host_acl_credits,
            runners: BluetoothRunners {
                hardware: BluetoothHardwareRunner::new(
                    task, controller, timer, interrupt, recheck, wakers,
                ),
            },
        },
    })
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "keep terminal affine-owner assembly out of the async poll"
)]
fn finish_outcome<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    outcome: Result<
        Restarted<P, MT, SC, H2C, C2H, PC>,
        ControllerRestartFailure<'static, P, PublishedStorage, SC, MT>,
    >,
    bindings: RetiredBindings<H2C, C2H, PC>,
) -> Result<
    BluetoothSystemReady<P, MT, SC, H2C, C2H, PC>,
    BluetoothHardwareRestartFailure<P, MT, SC, H2C, C2H, PC>,
> {
    let restarted = match outcome {
        Ok(owner) => owner,
        Err(owner) => {
            return Err(BluetoothHardwareRestartFailure {
                error: BluetoothHardwareRestartError::Controller(*owner.error()),
                _retained: Retained::Hardware {
                    _owner: owner,
                    _bindings: bindings,
                },
            });
        }
    };
    finish(restarted, bindings)
}
