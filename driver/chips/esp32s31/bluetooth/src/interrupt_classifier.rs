//! Evidence-bounded classification of the primary Bluetooth MAC IRQ suffix.
//!
//! The restricted PAC owns the exact masked-status sample and W1C prefix.
//! This module first rejects the four baseline groups consumed by the complete
//! assertion prefix, then classifies the three dynamic source groups consumed
//! by the source-124 scheduler suffix. Raw status and diagnostic images remain
//! inside the PAC; this layer receives positional source facts only.
//!
//! The reference handler can observe `SCHEDULER_STATE` twice: once while
//! handling bank-one source 3 and again while constructing scheduler work.
//! Distinct input types preserve those two temporal positions. A future hard
//! handler must obtain each observation at its named point; it must not reuse
//! one register image for both merely because their bit geometry is equal.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::{BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch};
pub use open_esp_radio_esp32s31_pac::{
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
};

/// Dynamic scheduler trigger selected from one acknowledged primary snapshot.
///
/// Names deliberately remain positional. Current evidence proves the branch
/// geometry and scheduler-work effects, but not Bluetooth Link-Layer event
/// names for the individual hardware bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPrimarySchedulerTrigger {
    /// None of the three reviewed dynamic groups is present.
    None,
    /// Bank-zero source 21 is present without source 27 or 28.
    Bank0Source21,
    /// Bank-zero source 27 or 28 is present.
    Bank0Sources27Or28 {
        /// Whether source 21 was present in the same acknowledged image.
        source_21_pending: bool,
    },
    /// Bank-one source 3 is present and takes precedence over the bank-zero
    /// branch, while retaining whether source 27 or 28 can mark the work.
    Bank1Source3 {
        /// Whether bank-zero source 27 or 28 was also present.
        bank_0_sources_27_or_28_pending: bool,
    },
}

impl BluetoothPrimarySchedulerTrigger {
    const fn from_epoch(epoch: &BluetoothPrimaryInterruptEpoch) -> Self {
        let sources_27_or_28_pending = epoch.bank_0_sources_27_or_28_pending();

        if epoch.bank_1_source_3_pending() {
            Self::Bank1Source3 {
                bank_0_sources_27_or_28_pending: sources_27_or_28_pending,
            }
        } else if sources_27_or_28_pending {
            Self::Bank0Sources27Or28 {
                source_21_pending: epoch.bank_0_source_21_pending(),
            }
        } else if epoch.bank_0_source_21_pending() {
            Self::Bank0Source21
        } else {
            Self::None
        }
    }

    #[cfg(test)]
    const fn from_dynamic_fields_for_validation(
        source_21_pending: bool,
        sources_27_or_28_pending: bool,
        source_3_pending: bool,
    ) -> Self {
        let epoch = BluetoothPrimaryInterruptEpoch::for_dynamic_validation(
            source_21_pending,
            sources_27_or_28_pending,
            source_3_pending,
        );
        Self::from_epoch(&epoch)
    }

    const fn work_inputs(self) -> Option<(bool, bool)> {
        match self {
            Self::None => None,
            Self::Bank0Source21 => Some((false, true)),
            Self::Bank0Sources27Or28 { source_21_pending } => Some((true, source_21_pending)),
            Self::Bank1Source3 {
                bank_0_sources_27_or_28_pending,
            } => Some((bank_0_sources_27_or_28_pending, true)),
        }
    }
}

/// Classified dynamic suffix of one complete primary interrupt observation.
///
/// The acknowledged PAC epoch remains affine while only semantic scheduler
/// trigger facts are exposed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a classified primary epoch must drive fault or scheduler handling"]
pub struct BluetoothPrimaryInterruptClassification {
    epoch: BluetoothPrimaryInterruptEpoch,
    scheduler_trigger: BluetoothPrimarySchedulerTrigger,
}

impl BluetoothPrimaryInterruptClassification {
    /// Classify one complete restricted-PAC epoch or preserve its fatal path.
    ///
    /// Any baseline fault takes precedence even when dynamic scheduler sources
    /// were pending in the same snapshot. A live Controller must retain the
    /// returned error and enter its fail-stop/quiesce path without publishing
    /// ordinary Link-Layer work for this epoch.
    pub const fn from_epoch(
        epoch: BluetoothPrimaryInterruptEpoch,
    ) -> Result<Self, BluetoothPrimaryControllerFault> {
        if epoch.fault_sources().is_fault() {
            return Err(BluetoothPrimaryControllerFault { epoch });
        }
        let scheduler_trigger = BluetoothPrimarySchedulerTrigger::from_epoch(&epoch);
        Ok(Self {
            epoch,
            scheduler_trigger,
        })
    }

    /// Return the positional dynamic scheduler trigger.
    pub const fn scheduler_trigger(&self) -> BluetoothPrimarySchedulerTrigger {
        self.scheduler_trigger
    }

    /// Request the first, bank-one-only scheduler-state observation.
    ///
    /// `None` means the complete reference handler does not perform this read.
    pub const fn reference_gate(&self) -> Option<BluetoothSchedulerReferenceGate> {
        match self.scheduler_trigger {
            BluetoothPrimarySchedulerTrigger::Bank1Source3 { .. } => {
                Some(BluetoothSchedulerReferenceGate)
            }
            _ => None,
        }
    }

    /// Request the later scheduler-state observation used to construct work.
    ///
    /// Every reviewed dynamic trigger produces exactly one such request.
    pub const fn work_classifier(&self) -> Option<BluetoothSchedulerWorkClassifier> {
        match self.scheduler_trigger.work_inputs() {
            Some((mark_candidate, state_publication_requested)) => {
                Some(BluetoothSchedulerWorkClassifier {
                    mark_candidate,
                    state_publication_requested,
                })
            }
            None => None,
        }
    }
}

/// Fatal primary interrupt result retaining semantic source presence.
///
/// The type classifies the reference handler's assertion path; it does not
/// panic in interrupt context. The future Controller lifecycle owns the
/// fail-stop transition, user-visible reset reason and quiescent recovery.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the Controller fault must be retained until fail-stop recovery completes"]
pub struct BluetoothPrimaryControllerFault {
    epoch: BluetoothPrimaryInterruptEpoch,
}

impl BluetoothPrimaryControllerFault {
    /// Return the positional fault sources that selected fail-stop handling.
    pub const fn sources(&self) -> BluetoothPrimaryFaultSources {
        self.epoch.fault_sources()
    }
}

/// Classifier token proving that bank-one source 3 requires a reference gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerReferenceGate;

impl BluetoothSchedulerReferenceGate {
    /// Decide the exact hardware action following the first state read.
    pub const fn classify(
        self,
        observation: BluetoothSchedulerReferenceGateObservation,
    ) -> BluetoothSchedulerReferenceAction {
        if !observation.is_busy() {
            BluetoothSchedulerReferenceAction::ClearReferenceAndRunPostClearSchedulerAction
        } else {
            BluetoothSchedulerReferenceAction::PreserveReference
        }
    }
}

/// Required reference-path disposition selected after the first state read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerReferenceAction {
    /// Write zero to `SCHEDULER_REFERENCE`, then run the mandatory open
    /// equivalent of the reviewed selector-6 scheduler action.
    ///
    /// The register write alone is not a complete disposition. The pinned BLE
    /// consumer immediately checks scheduler transaction/list consistency.
    /// That typed open action is still a publication blocker.
    ClearReferenceAndRunPostClearSchedulerAction,
    /// Leave `SCHEDULER_REFERENCE` unchanged while the busy bit is set.
    PreserveReference,
}

/// Deferred-work classifier retaining the two booleans selected by the ISR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerWorkClassifier {
    mark_candidate: bool,
    state_publication_requested: bool,
}

impl BluetoothSchedulerWorkClassifier {
    /// Classify the second scheduler-state observation into one worker wake.
    pub const fn classify(
        self,
        observation: &BluetoothSchedulerWorkObservation,
    ) -> BluetoothSchedulerWorkerWake {
        let reference_state = observation.reference_path_active();
        BluetoothSchedulerWorkerWake {
            class: if self.mark_candidate && reference_state {
                BluetoothSchedulerWorkerWakeClass::Marked
            } else {
                BluetoothSchedulerWorkerWakeClass::Ordinary
            },
            reference_state_publication: if self.state_publication_requested {
                Some(reference_state)
            } else {
                None
            },
        }
    }
}

/// Sticky class carried by the single deferred scheduler work item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerWorkerWakeClass {
    /// Process scheduler work without setting the reference-path marker.
    Ordinary,
    /// Preserve that the reference path was active when a mark-capable source
    /// fired. The marker must remain sticky if wakes are coalesced.
    Marked,
}

/// One deferred scheduler-worker wake derived from a dynamic primary IRQ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerWorkerWake {
    class: BluetoothSchedulerWorkerWakeClass,
    reference_state_publication: Option<bool>,
}

impl BluetoothSchedulerWorkerWake {
    /// Whether this wake carries the sticky reference-path marker.
    pub const fn class(self) -> BluetoothSchedulerWorkerWakeClass {
        self.class
    }

    /// Optional reference-state value published by the reviewed software path.
    ///
    /// This records the exact binary behavior needed to evaluate a replacement
    /// worker. It does not make the vendor callback selector an open-driver ABI.
    pub const fn reference_state_publication(self) -> Option<bool> {
        self.reference_state_publication
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothPrimaryInterruptClassification, BluetoothPrimarySchedulerTrigger,
        BluetoothSchedulerReferenceAction, BluetoothSchedulerReferenceGate,
        BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkClassifier,
        BluetoothSchedulerWorkObservation, BluetoothSchedulerWorkerWake,
        BluetoothSchedulerWorkerWakeClass,
    };
    use open_esp_radio_esp32s31_pac::BluetoothPrimaryInterruptEpoch;

    const fn trigger(
        source_21_pending: bool,
        sources_27_or_28_pending: bool,
        source_3_pending: bool,
    ) -> BluetoothPrimarySchedulerTrigger {
        BluetoothPrimarySchedulerTrigger::from_dynamic_fields_for_validation(
            source_21_pending,
            sources_27_or_28_pending,
            source_3_pending,
        )
    }

    const fn classify_work(
        trigger: BluetoothPrimarySchedulerTrigger,
        busy: bool,
        reference_state_29: bool,
    ) -> Option<BluetoothSchedulerWorkerWake> {
        match trigger.work_inputs() {
            Some((mark_candidate, state_publication_requested)) => Some(
                BluetoothSchedulerWorkClassifier {
                    mark_candidate,
                    state_publication_requested,
                }
                .classify(
                    &BluetoothSchedulerWorkObservation::from_fields_for_validation(
                        busy,
                        reference_state_29,
                        0,
                    ),
                ),
            ),
            None => None,
        }
    }

    #[test]
    fn baseline_fault_preempts_dynamic_scheduler_work() {
        let epoch = BluetoothPrimaryInterruptEpoch::for_fault_validation();
        let fault = BluetoothPrimaryInterruptClassification::from_epoch(epoch)
            .expect_err("a baseline assertion source must preempt scheduler work");

        assert!(fault.sources().is_fault());
        assert!(fault.sources().bank_1_source_8_pending());
    }

    #[test]
    fn fault_free_epoch_reaches_dynamic_scheduler_classifier() {
        let epoch = BluetoothPrimaryInterruptEpoch::for_dynamic_validation(false, true, true);
        let classification = BluetoothPrimaryInterruptClassification::from_epoch(epoch)
            .expect("dynamic sources are not fault lanes");

        assert_eq!(
            classification.scheduler_trigger(),
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: true,
            }
        );
    }

    #[test]
    fn bank_zero_trigger_table_preserves_source_precedence_and_pairing() {
        assert_eq!(
            trigger(false, false, false),
            BluetoothPrimarySchedulerTrigger::None
        );
        assert_eq!(
            trigger(true, false, false),
            BluetoothPrimarySchedulerTrigger::Bank0Source21
        );
        assert_eq!(
            trigger(false, true, false),
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: false,
            }
        );
        assert_eq!(
            trigger(true, true, false),
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: true,
            }
        );
    }

    #[test]
    fn bank_one_source_three_has_precedence_and_retains_mark_candidate() {
        assert_eq!(
            trigger(true, false, true),
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: false,
            }
        );
        assert_eq!(
            trigger(false, true, true),
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: true,
            }
        );
    }

    #[test]
    fn reference_gate_requires_post_clear_action_only_when_not_busy() {
        let gate = BluetoothSchedulerReferenceGate;

        assert_eq!(
            gate.classify(
                BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(false)
            ),
            BluetoothSchedulerReferenceAction::ClearReferenceAndRunPostClearSchedulerAction
        );
        assert_eq!(
            gate.classify(
                BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(true)
            ),
            BluetoothSchedulerReferenceAction::PreserveReference
        );
    }

    #[test]
    fn source_twenty_one_requests_ordinary_work_and_state_publication() {
        for (busy, state_29, expected_publication) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            let wake = classify_work(
                BluetoothPrimarySchedulerTrigger::Bank0Source21,
                busy,
                state_29,
            )
            .expect("source 21 must request work");
            assert_eq!(wake.class(), BluetoothSchedulerWorkerWakeClass::Ordinary);
            assert_eq!(
                wake.reference_state_publication(),
                Some(expected_publication)
            );
        }
    }

    #[test]
    fn sources_twenty_seven_or_twenty_eight_mark_only_the_active_reference_path() {
        let unmarked = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: false,
            },
            true,
            false,
        )
        .expect("high source group must request work");
        let marked = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: false,
            },
            true,
            true,
        )
        .expect("high source group must request work");

        assert_eq!(
            unmarked.class(),
            BluetoothSchedulerWorkerWakeClass::Ordinary
        );
        assert_eq!(marked.class(), BluetoothSchedulerWorkerWakeClass::Marked);
        assert_eq!(marked.reference_state_publication(), None);
    }

    #[test]
    fn combined_bank_zero_trigger_marks_and_publishes_the_same_second_read() {
        let wake = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: true,
            },
            true,
            true,
        )
        .expect("combined bank-zero trigger must request work");

        assert_eq!(wake.class(), BluetoothSchedulerWorkerWakeClass::Marked);
        assert_eq!(wake.reference_state_publication(), Some(true));
    }

    #[test]
    fn bank_one_trigger_always_publishes_and_marks_only_with_the_high_bank_zero_group() {
        let ordinary = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: false,
            },
            true,
            true,
        )
        .expect("bank-one trigger must request work");
        let marked = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: true,
            },
            true,
            true,
        )
        .expect("bank-one trigger must request work");

        assert_eq!(
            ordinary.class(),
            BluetoothSchedulerWorkerWakeClass::Ordinary
        );
        assert_eq!(marked.class(), BluetoothSchedulerWorkerWakeClass::Marked);
        assert_eq!(ordinary.reference_state_publication(), Some(true));
        assert_eq!(marked.reference_state_publication(), Some(true));
    }

    #[test]
    fn no_dynamic_trigger_produces_no_scheduler_work() {
        assert_eq!(
            classify_work(BluetoothPrimarySchedulerTrigger::None, true, true),
            None
        );
    }
}
