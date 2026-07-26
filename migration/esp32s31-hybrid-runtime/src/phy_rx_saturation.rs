//! Event-driven replacement for `phy_check_rx_sat`.
//!
//! The pinned archive body performs eleven PBus commands, calls
//! `ets_delay_us(5)`, then reads `0x2010_08d0[21:20]` exactly 100 times.
//! No dedicated completion interrupt is evidenced in the available S31
//! PAC/SVD or ROM symbols. Rust therefore retains the required polling as 100
//! separately completed one-shot samples. The executor may yield or arm an
//! async timer between samples; neither this transition nor its MMIO leaf
//! contains a spin loop.

use crate::phy_pbus::PhyPbusForceTest;

pub const PHY_RX_SATURATION_DELAY_MICROS: u32 = 5;
pub const PHY_RX_SATURATION_SAMPLE_COUNT: u8 = 100;
pub const PHY_RX_SATURATION_STATUS_ADDRESS: usize = 0x2010_08d0;
pub const PHY_RX_SATURATION_STATUS_MASK: u32 = 0x0030_0000;

const PHY_RX_SATURATION_PBUS_COUNT: u8 = 11;

const fn pbus_transaction(index: u8, parameter_002: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, parameter_002 as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        6 => PhyPbusForceTest::new(1, 2, 0),
        7 => PhyPbusForceTest::new(2, 1, 0x100),
        8 => PhyPbusForceTest::new(3, 1, 0x100),
        9 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationOutcome {
    Measured { saturated_samples: u8, samples: u8 },
    PbusTimedOut(PhyPbusForceTest),
    CaptureTimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationAction {
    ConfigureDebugMode,
    ForcePbus(PhyPbusForceTest),
    DelayMicros {
        micros: u32,
    },
    SampleStatus {
        address: usize,
        activity_mask: u32,
        sample_index: u8,
        samples: u8,
    },
    ConfigureWorkMode,
    Complete(PhyRxSaturationOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationCompletion {
    DebugModeConfigured,
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    DelayElapsed {
        micros: u32,
    },
    StatusSampled {
        address: usize,
        sample_index: u8,
        register_value: u32,
    },
    CaptureTimedOut,
    WorkModeConfigured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationTransitionError {
    WrongCompletion,
    InvalidCapture,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxSaturationStep {
    DebugMode,
    Pbus {
        index: u8,
    },
    Delay,
    Sample {
        sample_index: u8,
        saturated_samples: u8,
    },
    WorkMode(PhyRxSaturationOutcome),
    Complete(PhyRxSaturationOutcome),
}

/// Caller-driven `phy_check_rx_sat` state machine.
///
/// Each status read is a separate action/completion pair. This preserves the
/// reference's bounded 100-sample policy while allowing the Rust executor to
/// yield between samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxSaturationTransition {
    parameter_002: u8,
    step: PhyRxSaturationStep,
}

impl PhyRxSaturationTransition {
    pub const fn new(parameter_002: u8) -> Self {
        Self {
            parameter_002,
            step: PhyRxSaturationStep::DebugMode,
        }
    }

    pub const fn action(self) -> PhyRxSaturationAction {
        match self.step {
            PhyRxSaturationStep::DebugMode => PhyRxSaturationAction::ConfigureDebugMode,
            PhyRxSaturationStep::Pbus { index } => {
                PhyRxSaturationAction::ForcePbus(pbus_transaction(index, self.parameter_002))
            }
            PhyRxSaturationStep::Delay => PhyRxSaturationAction::DelayMicros {
                micros: PHY_RX_SATURATION_DELAY_MICROS,
            },
            PhyRxSaturationStep::Sample { sample_index, .. } => {
                PhyRxSaturationAction::SampleStatus {
                    address: PHY_RX_SATURATION_STATUS_ADDRESS,
                    activity_mask: PHY_RX_SATURATION_STATUS_MASK,
                    sample_index,
                    samples: PHY_RX_SATURATION_SAMPLE_COUNT,
                }
            }
            PhyRxSaturationStep::WorkMode(_) => PhyRxSaturationAction::ConfigureWorkMode,
            PhyRxSaturationStep::Complete(outcome) => PhyRxSaturationAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxSaturationCompletion,
    ) -> Result<(), PhyRxSaturationTransitionError> {
        self.step = match (self.step, completion) {
            (PhyRxSaturationStep::DebugMode, PhyRxSaturationCompletion::DebugModeConfigured) => {
                PhyRxSaturationStep::Pbus { index: 0 }
            }
            (
                PhyRxSaturationStep::Pbus { index },
                PhyRxSaturationCompletion::PbusCompleted(completed),
            ) if completed == pbus_transaction(index, self.parameter_002) => {
                let next = index + 1;
                if next == PHY_RX_SATURATION_PBUS_COUNT {
                    PhyRxSaturationStep::Delay
                } else {
                    PhyRxSaturationStep::Pbus { index: next }
                }
            }
            (
                PhyRxSaturationStep::Pbus { index },
                PhyRxSaturationCompletion::PbusTimedOut(completed),
            ) if completed == pbus_transaction(index, self.parameter_002) => {
                PhyRxSaturationStep::WorkMode(PhyRxSaturationOutcome::PbusTimedOut(completed))
            }
            (
                PhyRxSaturationStep::Delay,
                PhyRxSaturationCompletion::DelayElapsed {
                    micros: PHY_RX_SATURATION_DELAY_MICROS,
                },
            ) => PhyRxSaturationStep::Sample {
                sample_index: 0,
                saturated_samples: 0,
            },
            (
                PhyRxSaturationStep::Sample {
                    sample_index,
                    saturated_samples,
                },
                PhyRxSaturationCompletion::StatusSampled {
                    address: PHY_RX_SATURATION_STATUS_ADDRESS,
                    sample_index: completed_index,
                    register_value,
                },
            ) if completed_index == sample_index => {
                let saturated_samples = saturated_samples
                    .wrapping_add((register_value & PHY_RX_SATURATION_STATUS_MASK != 0) as u8);
                let next = sample_index + 1;
                if next == PHY_RX_SATURATION_SAMPLE_COUNT {
                    PhyRxSaturationStep::WorkMode(PhyRxSaturationOutcome::Measured {
                        saturated_samples,
                        samples: PHY_RX_SATURATION_SAMPLE_COUNT,
                    })
                } else {
                    PhyRxSaturationStep::Sample {
                        sample_index: next,
                        saturated_samples,
                    }
                }
            }
            (
                PhyRxSaturationStep::Sample { .. },
                PhyRxSaturationCompletion::StatusSampled { .. },
            ) => return Err(PhyRxSaturationTransitionError::InvalidCapture),
            (PhyRxSaturationStep::Sample { .. }, PhyRxSaturationCompletion::CaptureTimedOut) => {
                PhyRxSaturationStep::WorkMode(PhyRxSaturationOutcome::CaptureTimedOut)
            }
            (
                PhyRxSaturationStep::WorkMode(outcome),
                PhyRxSaturationCompletion::WorkModeConfigured,
            ) => PhyRxSaturationStep::Complete(outcome),
            (PhyRxSaturationStep::Complete(_), _) => {
                return Err(PhyRxSaturationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxSaturationTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationSampleBindingError {
    NotStatusSample,
}

/// A non-cloneable token for exactly one polling sample.
///
/// Repeating the poll is a state-machine/executor decision. The target leaf
/// itself performs one volatile read and cannot spin.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxSaturationSampleBinding {
    address: usize,
    sample_index: u8,
}

impl PhyRxSaturationSampleBinding {
    pub fn new(action: PhyRxSaturationAction) -> Result<Self, PhyRxSaturationSampleBindingError> {
        match action {
            PhyRxSaturationAction::SampleStatus {
                address,
                activity_mask: PHY_RX_SATURATION_STATUS_MASK,
                sample_index,
                samples: PHY_RX_SATURATION_SAMPLE_COUNT,
            } if address == PHY_RX_SATURATION_STATUS_ADDRESS => Ok(Self {
                address,
                sample_index,
            }),
            _ => Err(PhyRxSaturationSampleBindingError::NotStatusSample),
        }
    }

    /// Perform one volatile sample and consume the issued identity.
    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyRxSaturationCompletion {
        PhyRxSaturationCompletion::StatusSampled {
            address: self.address,
            sample_index: self.sample_index,
            register_value: (self.address as *const u32).read_volatile(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PhyRxSaturationAction, PhyRxSaturationCompletion, PhyRxSaturationOutcome,
        PhyRxSaturationSampleBinding, PhyRxSaturationSampleBindingError, PhyRxSaturationTransition,
        PhyRxSaturationTransitionError, PHY_RX_SATURATION_DELAY_MICROS,
        PHY_RX_SATURATION_SAMPLE_COUNT, PHY_RX_SATURATION_STATUS_ADDRESS,
        PHY_RX_SATURATION_STATUS_MASK,
    };
    use crate::phy_pbus::PhyPbusForceTest;

    #[test]
    fn transition_reproduces_all_pbus_commands_and_async_edges() {
        let expected = [
            PhyPbusForceTest::new(4, 1, 0),
            PhyPbusForceTest::new(4, 2, 1),
            PhyPbusForceTest::new(5, 1, 0),
            PhyPbusForceTest::new(0, 1, 0x40),
            PhyPbusForceTest::new(0, 2, 0xbf),
            PhyPbusForceTest::new(1, 1, 0x189),
            PhyPbusForceTest::new(1, 2, 0),
            PhyPbusForceTest::new(2, 1, 0x100),
            PhyPbusForceTest::new(3, 1, 0x100),
            PhyPbusForceTest::new(2, 2, 0x100),
            PhyPbusForceTest::new(3, 2, 0x100),
        ];
        let mut transition = PhyRxSaturationTransition::new(0xbf);
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::ConfigureDebugMode
        );
        transition
            .advance(PhyRxSaturationCompletion::DebugModeConfigured)
            .unwrap();
        for transaction in expected {
            assert_eq!(
                transition.action(),
                PhyRxSaturationAction::ForcePbus(transaction)
            );
            transition
                .advance(PhyRxSaturationCompletion::PbusCompleted(transaction))
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::DelayMicros {
                micros: PHY_RX_SATURATION_DELAY_MICROS,
            }
        );
        transition
            .advance(PhyRxSaturationCompletion::DelayElapsed {
                micros: PHY_RX_SATURATION_DELAY_MICROS,
            })
            .unwrap();
        for sample_index in 0..PHY_RX_SATURATION_SAMPLE_COUNT {
            assert_eq!(
                transition.action(),
                PhyRxSaturationAction::SampleStatus {
                    address: PHY_RX_SATURATION_STATUS_ADDRESS,
                    activity_mask: PHY_RX_SATURATION_STATUS_MASK,
                    sample_index,
                    samples: PHY_RX_SATURATION_SAMPLE_COUNT,
                }
            );
            transition
                .advance(PhyRxSaturationCompletion::StatusSampled {
                    address: PHY_RX_SATURATION_STATUS_ADDRESS,
                    sample_index,
                    register_value: if sample_index < 7 {
                        PHY_RX_SATURATION_STATUS_MASK
                    } else {
                        0
                    },
                })
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::ConfigureWorkMode
        );
        transition
            .advance(PhyRxSaturationCompletion::WorkModeConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::Complete(PhyRxSaturationOutcome::Measured {
                saturated_samples: 7,
                samples: 100,
            })
        );
    }

    #[test]
    fn sample_rejects_wrong_address_and_index() {
        let mut transition = PhyRxSaturationTransition::new(0);
        transition
            .advance(PhyRxSaturationCompletion::DebugModeConfigured)
            .unwrap();
        for transaction in [
            PhyPbusForceTest::new(4, 1, 0),
            PhyPbusForceTest::new(4, 2, 1),
            PhyPbusForceTest::new(5, 1, 0),
            PhyPbusForceTest::new(0, 1, 0x40),
            PhyPbusForceTest::new(0, 2, 0),
            PhyPbusForceTest::new(1, 1, 0x189),
            PhyPbusForceTest::new(1, 2, 0),
            PhyPbusForceTest::new(2, 1, 0x100),
            PhyPbusForceTest::new(3, 1, 0x100),
            PhyPbusForceTest::new(2, 2, 0x100),
            PhyPbusForceTest::new(3, 2, 0x100),
        ] {
            transition
                .advance(PhyRxSaturationCompletion::PbusCompleted(transaction))
                .unwrap();
        }
        transition
            .advance(PhyRxSaturationCompletion::DelayElapsed { micros: 5 })
            .unwrap();
        assert_eq!(
            transition.advance(PhyRxSaturationCompletion::StatusSampled {
                address: PHY_RX_SATURATION_STATUS_ADDRESS + 4,
                sample_index: 0,
                register_value: PHY_RX_SATURATION_STATUS_MASK,
            }),
            Err(PhyRxSaturationTransitionError::InvalidCapture)
        );
        assert_eq!(
            transition.advance(PhyRxSaturationCompletion::StatusSampled {
                address: PHY_RX_SATURATION_STATUS_ADDRESS,
                sample_index: 1,
                register_value: PHY_RX_SATURATION_STATUS_MASK,
            }),
            Err(PhyRxSaturationTransitionError::InvalidCapture)
        );
    }

    #[test]
    fn sample_binding_accepts_only_exact_one_shot_poll_action() {
        let action = PhyRxSaturationAction::SampleStatus {
            address: PHY_RX_SATURATION_STATUS_ADDRESS,
            activity_mask: PHY_RX_SATURATION_STATUS_MASK,
            sample_index: 42,
            samples: PHY_RX_SATURATION_SAMPLE_COUNT,
        };
        assert!(PhyRxSaturationSampleBinding::new(action).is_ok());
        assert_eq!(
            PhyRxSaturationSampleBinding::new(PhyRxSaturationAction::ConfigureDebugMode),
            Err(PhyRxSaturationSampleBindingError::NotStatusSample)
        );
    }

    #[test]
    fn timeout_still_restores_work_mode_before_terminal_state() {
        let transaction = PhyPbusForceTest::new(4, 1, 0);
        let mut transition = PhyRxSaturationTransition::new(0);
        transition
            .advance(PhyRxSaturationCompletion::DebugModeConfigured)
            .unwrap();
        transition
            .advance(PhyRxSaturationCompletion::PbusTimedOut(transaction))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::ConfigureWorkMode
        );
        transition
            .advance(PhyRxSaturationCompletion::WorkModeConfigured)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::Complete(PhyRxSaturationOutcome::PbusTimedOut(transaction))
        );
    }
}
