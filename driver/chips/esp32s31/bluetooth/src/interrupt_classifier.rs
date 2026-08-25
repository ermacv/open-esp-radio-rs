//! Evidence-bounded classification of the primary Bluetooth MAC IRQ suffix.
//!
//! The restricted PAC owns the exact masked-status sample and W1C prefix.
//! This module first rejects the four baseline groups consumed by the complete
//! assertion prefix, then classifies the three dynamic source groups consumed
//! by the source-124 scheduler suffix. Still-opaque bits remain in the retained
//! observation and are not assigned Link-Layer meanings here.
//!
//! The reference handler can observe `SCHEDULER_STATE` twice: once while
//! handling bank-one source 3 and again while constructing scheduler work.
//! Distinct input types preserve those two temporal positions. A future hard
//! handler must obtain each observation at its named point; it must not reuse
//! one register image for both merely because their bit geometry is equal.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_pac::{
    BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK, BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK,
};
use open_esp_radio_esp32s31_pac::{
    BluetoothPrimaryFaultEvidence, BluetoothPrimaryInterruptEpoch,
    BluetoothPrimaryInterruptObservation,
};

const BANK_0_SOURCE_21: u32 = 1 << 21;
const BANK_0_SOURCES_27_28: u32 = (1 << 27) | (1 << 28);
const BANK_1_SOURCE_3: u32 = 1 << 3;
const SCHEDULER_STATE_29: u32 = 1 << 29;
const SCHEDULER_BUSY: u32 = 1 << 31;

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
    const fn from_status_bits(bank_0: u32, bank_1: u32) -> Self {
        let sources_27_or_28_pending = bank_0 & BANK_0_SOURCES_27_28 != 0;

        if bank_1 & BANK_1_SOURCE_3 != 0 {
            Self::Bank1Source3 {
                bank_0_sources_27_or_28_pending: sources_27_or_28_pending,
            }
        } else if sources_27_or_28_pending {
            Self::Bank0Sources27Or28 {
                source_21_pending: bank_0 & BANK_0_SOURCE_21 != 0,
            }
        } else if bank_0 & BANK_0_SOURCE_21 != 0 {
            Self::Bank0Source21
        } else {
            Self::None
        }
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
/// The original two-bank observation remains available so later layers do not
/// lose baseline or currently opaque pending bits while consuming the dynamic
/// scheduler classification.
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
        if epoch.fault_evidence().is_fault() {
            return Err(BluetoothPrimaryControllerFault { epoch });
        }
        let observation = epoch.observation();
        let scheduler_trigger = BluetoothPrimarySchedulerTrigger::from_status_bits(
            observation.bank_0_bits(),
            observation.bank_1_bits(),
        );
        Ok(Self {
            epoch,
            scheduler_trigger,
        })
    }

    /// Return the lossless acknowledged status image.
    pub const fn observation(&self) -> BluetoothPrimaryInterruptObservation {
        self.epoch.observation()
    }

    /// Consume the classification and return its acknowledged epoch.
    pub fn into_epoch(self) -> BluetoothPrimaryInterruptEpoch {
        self.epoch
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

/// Fatal primary interrupt result retaining every captured diagnostic word.
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
    /// Return the complete masked observation that selected the fault path.
    pub const fn observation(&self) -> BluetoothPrimaryInterruptObservation {
        self.epoch.observation()
    }

    /// Return every source and conditional diagnostic image captured by PAC.
    pub const fn evidence(&self) -> BluetoothPrimaryFaultEvidence {
        self.epoch.fault_evidence()
    }

    /// Return the complete acknowledged interrupt epoch.
    pub fn into_epoch(self) -> BluetoothPrimaryInterruptEpoch {
        self.epoch
    }
}

/// First scheduler-state observation made only for bank-one source 3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerReferenceGateObservation(u32);

impl BluetoothSchedulerReferenceGateObservation {
    /// Wrap one complete `SCHEDULER_STATE` image read at the reference-gate
    /// point of the hard-handler suffix.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete observed register image.
    pub const fn bits(self) -> u32 {
        self.0
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
        if observation.bits() & SCHEDULER_BUSY == 0 {
            BluetoothSchedulerReferenceAction::ClearReference
        } else {
            BluetoothSchedulerReferenceAction::PreserveReference
        }
    }
}

/// Hardware action selected for `SCHEDULER_REFERENCE` after the first read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerReferenceAction {
    /// Write the complete zero image to `SCHEDULER_REFERENCE`.
    ClearReference,
    /// Leave `SCHEDULER_REFERENCE` unchanged while the busy bit is set.
    PreserveReference,
}

/// Later scheduler-state observation used to construct the worker wake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerWorkObservation(u32);

impl BluetoothSchedulerWorkObservation {
    /// Wrap one complete `SCHEDULER_STATE` image read at the scheduler-work
    /// point, after any reference-gate action.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete observed register image.
    pub const fn bits(self) -> u32 {
        self.0
    }

    const fn reference_state(self) -> bool {
        self.0 & (SCHEDULER_BUSY | SCHEDULER_STATE_29) == (SCHEDULER_BUSY | SCHEDULER_STATE_29)
    }
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
        observation: BluetoothSchedulerWorkObservation,
    ) -> BluetoothSchedulerWorkerWake {
        let reference_state = observation.reference_state();
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
        BANK_0_SOURCE_21, BANK_0_SOURCES_27_28, BANK_1_SOURCE_3,
        BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK, BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK,
        BluetoothPrimaryInterruptClassification, BluetoothPrimarySchedulerTrigger,
        BluetoothSchedulerReferenceAction, BluetoothSchedulerReferenceGate,
        BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkClassifier,
        BluetoothSchedulerWorkObservation, BluetoothSchedulerWorkerWake,
        BluetoothSchedulerWorkerWakeClass, SCHEDULER_BUSY, SCHEDULER_STATE_29,
    };
    use open_esp_radio_esp32s31_pac::BluetoothPrimaryInterruptEpoch;

    const fn trigger(bank_0: u32, bank_1: u32) -> BluetoothPrimarySchedulerTrigger {
        BluetoothPrimarySchedulerTrigger::from_status_bits(bank_0, bank_1)
    }

    const fn classify_work(
        trigger: BluetoothPrimarySchedulerTrigger,
        scheduler_state: u32,
    ) -> Option<BluetoothSchedulerWorkerWake> {
        match trigger.work_inputs() {
            Some((mark_candidate, state_publication_requested)) => Some(
                BluetoothSchedulerWorkClassifier {
                    mark_candidate,
                    state_publication_requested,
                }
                .classify(BluetoothSchedulerWorkObservation::from_bits(
                    scheduler_state,
                )),
            ),
            None => None,
        }
    }

    #[test]
    fn dynamic_masks_are_exact_complete_helper_images() {
        assert_eq!(
            BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK,
            BANK_0_SOURCE_21 | BANK_0_SOURCES_27_28
        );
        assert_eq!(BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK, BANK_1_SOURCE_3);
    }

    #[test]
    fn baseline_fault_preempts_dynamic_scheduler_work_and_retains_diagnostics() {
        let epoch = BluetoothPrimaryInterruptEpoch::for_validation(
            BANK_0_SOURCE_21,
            BANK_1_SOURCE_3 | (1 << 9) | (1 << 12),
            0x1111_2222,
            0x3333_4444,
            0x5555_6666,
        );
        let expected_observation = epoch.observation();
        let fault = BluetoothPrimaryInterruptClassification::from_epoch(epoch)
            .expect_err("a baseline assertion source must preempt scheduler work");

        assert_eq!(fault.observation(), expected_observation);
        assert_eq!(fault.evidence().bank_1_source_bits(), (1 << 9) | (1 << 12));
        assert_eq!(
            fault.evidence().source_9_details(),
            Some([0x1111_2222, 0x3333_4444])
        );
        assert_eq!(fault.evidence().source_12_state(), Some(0x5555_6666));
    }

    #[test]
    fn fault_free_epoch_reaches_dynamic_scheduler_classifier() {
        let epoch = BluetoothPrimaryInterruptEpoch::for_validation(
            BANK_0_SOURCES_27_28,
            BANK_1_SOURCE_3,
            u32::MAX,
            u32::MAX,
            u32::MAX,
        );
        let expected_observation = epoch.observation();
        let classification = BluetoothPrimaryInterruptClassification::from_epoch(epoch)
            .expect("dynamic sources are not fault lanes");

        assert_eq!(classification.observation(), expected_observation);
        assert_eq!(
            classification.scheduler_trigger(),
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: true,
            }
        );
    }

    #[test]
    fn bank_zero_trigger_table_preserves_source_precedence_and_pairing() {
        assert_eq!(trigger(0, 0), BluetoothPrimarySchedulerTrigger::None);
        assert_eq!(
            trigger(BANK_0_SOURCE_21, 0),
            BluetoothPrimarySchedulerTrigger::Bank0Source21
        );
        assert_eq!(
            trigger(1 << 27, 0),
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: false,
            }
        );
        assert_eq!(
            trigger(BANK_0_SOURCE_21 | (1 << 28), 0),
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: true,
            }
        );
    }

    #[test]
    fn bank_one_source_three_has_precedence_and_retains_mark_candidate() {
        assert_eq!(
            trigger(BANK_0_SOURCE_21, BANK_1_SOURCE_3),
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: false,
            }
        );
        assert_eq!(
            trigger(BANK_0_SOURCES_27_28, BANK_1_SOURCE_3),
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: true,
            }
        );
    }

    #[test]
    fn reference_gate_clears_only_when_the_first_observation_is_not_busy() {
        let gate = BluetoothSchedulerReferenceGate;

        assert_eq!(
            gate.classify(BluetoothSchedulerReferenceGateObservation::from_bits(0)),
            BluetoothSchedulerReferenceAction::ClearReference
        );
        assert_eq!(
            gate.classify(BluetoothSchedulerReferenceGateObservation::from_bits(
                SCHEDULER_BUSY
            )),
            BluetoothSchedulerReferenceAction::PreserveReference
        );
    }

    #[test]
    fn source_twenty_one_requests_ordinary_work_and_state_publication() {
        for (state, expected_publication) in [
            (0, false),
            (SCHEDULER_BUSY, false),
            (SCHEDULER_STATE_29, false),
            (SCHEDULER_BUSY | SCHEDULER_STATE_29, true),
        ] {
            let wake = classify_work(BluetoothPrimarySchedulerTrigger::Bank0Source21, state)
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
            SCHEDULER_BUSY,
        )
        .expect("high source group must request work");
        let marked = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank0Sources27Or28 {
                source_21_pending: false,
            },
            SCHEDULER_BUSY | SCHEDULER_STATE_29,
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
            SCHEDULER_BUSY | SCHEDULER_STATE_29,
        )
        .expect("combined bank-zero trigger must request work");

        assert_eq!(wake.class(), BluetoothSchedulerWorkerWakeClass::Marked);
        assert_eq!(wake.reference_state_publication(), Some(true));
    }

    #[test]
    fn bank_one_trigger_always_publishes_and_marks_only_with_the_high_bank_zero_group() {
        let state = SCHEDULER_BUSY | SCHEDULER_STATE_29;
        let ordinary = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: false,
            },
            state,
        )
        .expect("bank-one trigger must request work");
        let marked = classify_work(
            BluetoothPrimarySchedulerTrigger::Bank1Source3 {
                bank_0_sources_27_or_28_pending: true,
            },
            state,
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
            classify_work(BluetoothPrimarySchedulerTrigger::None, u32::MAX),
            None
        );
    }
}
