//! Role-consistent composition of reviewed DTM event words into bound memory.
//!
//! This layer combines already validated LLL transforms with the lower
//! consuming memory transaction. The resulting state remains CPU-only and
//! retains TX packet readiness where that role requires it. It does not prove
//! list insertion, a visibility fence, a hardware latch or completion
//! ownership.

#![forbid(unsafe_code)]

use core::{convert::Infallible, marker::PhantomData};

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPositionalEventPrepared,
    BluetoothDtmMemoryGraphPrepareFailure, BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    BluetoothDtmPositionalEventWords,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerLockModifyRequest,
};

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength,
    BluetoothDtmPayloadPattern, BluetoothDtmPreparedTxGraph, BluetoothDtmRole,
    BluetoothDtmSchedulerItemEvent, dtm_scheduler_item::apply_overlap_insertion_power,
};

/// Type marker for a transmitter event with a prepared packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTransmitterEvent {}

/// Type marker for a receiver event without a transmitter packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReceiverEvent {}

/// Why two validated DTM transforms cannot describe one event plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReviewedEventWordsPlanError {
    /// Link-state and scheduler-item transforms encode different DTM roles.
    RoleMismatch {
        /// Role required by the selected constructor.
        expected: BluetoothDtmRole,
        /// Role selected by the link-state reset.
        link_state: BluetoothDtmRole,
        /// Role selected by the scheduler-item transform.
        scheduler_item: BluetoothDtmRole,
    },
}

/// Validated role-consistent plan for the seventeen reviewed event words.
///
/// Private chain links are deliberately absent from plan identity. They are
/// replaced inside `prepare` with fresh links sampled from the consumed graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmReviewedEventWordsPlan<Role> {
    link_state: BluetoothDtmLinkStateReset,
    scheduler_item: BluetoothDtmSchedulerItemEvent,
    epoch: BluetoothControllerSchedulerEpoch,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmReviewedEventWordsPlan<Role> {
    fn new_for_role(
        link_state: BluetoothDtmLinkStateReset,
        scheduler_item: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
        expected: BluetoothDtmRole,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanError> {
        let link_role = link_state.role();
        let scheduler_role = scheduler_item.role();
        if link_role != expected || scheduler_role != expected {
            return Err(BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
                expected,
                link_state: link_role,
                scheduler_item: scheduler_role,
            });
        }
        Ok(Self {
            link_state,
            scheduler_item,
            epoch,
            _role: PhantomData,
        })
    }

    fn apply_to_seed(
        &self,
        seed: open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmPositionalEventSeed,
    ) -> BluetoothDtmPositionalEventWords {
        let current = seed.words();
        let link_state = self
            .link_state
            .with_private_links(seed.tx_head(), seed.rx_tail())
            .apply(current.link_state());
        let scheduler_item = self
            .scheduler_item
            .apply(current.scheduler_item(), self.epoch);
        let scheduler_item = apply_overlap_insertion_power(scheduler_item, link_state);
        BluetoothDtmPositionalEventWords::new(link_state, scheduler_item)
    }
}

impl BluetoothDtmReviewedEventWordsPlan<BluetoothDtmTransmitterEvent> {
    /// Pair two transmitter transforms into a packet-gated event plan.
    pub fn new_transmitter(
        link_state: BluetoothDtmLinkStateReset,
        scheduler_item: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanError> {
        Self::new_for_role(
            link_state,
            scheduler_item,
            epoch,
            BluetoothDtmRole::Transmitter,
        )
    }

    /// Apply this TX plan only to a graph carrying a complete standard packet.
    ///
    /// Any lower validation failure returns an ordinary CPU owner. A retry
    /// must deliberately prepare a fresh packet-readiness proof.
    pub fn prepare(
        self,
        owner: BluetoothDtmPreparedTxGraph,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmTransmitterEvent>,
        BluetoothDtmMemoryGraphPrepareFailure,
    > {
        let (memory, pattern, length) = owner.into_parts();
        let prepared = memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(self.apply_to_seed(seed)))?;

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            packet: BluetoothDtmEventPacket::Transmitter { pattern, length },
            _role: PhantomData,
        })
    }
}

impl BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent> {
    /// Pair two receiver transforms into an RX event plan.
    pub fn new_receiver(
        link_state: BluetoothDtmLinkStateReset,
        scheduler_item: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanError> {
        Self::new_for_role(
            link_state,
            scheduler_item,
            epoch,
            BluetoothDtmRole::Receiver,
        )
    }

    /// Apply this RX plan to one exact ordinary CPU-owned graph.
    pub fn prepare(
        self,
        owner: BluetoothDtmMemoryGraphCpuOwned,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmReceiverEvent>,
        BluetoothDtmMemoryGraphPrepareFailure,
    > {
        let prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(self.apply_to_seed(seed)))?;

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            packet: BluetoothDtmEventPacket::Receiver,
            _role: PhantomData,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothDtmEventPacket {
    Transmitter {
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    },
    Receiver,
}

/// CPU-owned bound graph containing one role-consistent reviewed word image.
///
/// The role marker preserves whether TX packet readiness was consumed into the
/// event. This type exposes no packet mutation or publication operation.
pub struct BluetoothDtmReviewedEventWordsPrepared<Role> {
    memory: BluetoothDtmMemoryGraphPositionalEventPrepared,
    packet: BluetoothDtmEventPacket,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmReviewedEventWordsPrepared<Role> {
    /// Return the role shared by both applied transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventPacket::Receiver => BluetoothDtmRole::Receiver,
        }
    }

    /// Return the typed controller-SRAM identity of the prepared item.
    ///
    /// This identity is derived from the retained non-forgeable graph binding;
    /// it does not grant publication or hardware ownership.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Read back the exact seventeen CPU-owned positional words.
    pub fn words(&self) -> BluetoothDtmPositionalEventWords {
        self.memory.words()
    }

    /// Install the common scheduler bookkeeping prefix for this exact graph.
    ///
    /// The resulting state remains CPU-owned and cancellable. Only that later
    /// state may form a scheduler request; this prevents event words without
    /// the in-flight sentinel and cleared completion link from being admitted.
    pub fn prepare_scheduler_bookkeeping(self) -> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
        BluetoothDtmSchedulerBookkeepingPrepared {
            memory: self.memory.prepare_scheduler_bookkeeping(),
            packet: self.packet,
            _role: PhantomData,
        }
    }

    /// Cancel before publication and recover the exact prior CPU-owned graph.
    pub fn cancel(self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.memory.cancel()
    }
}

impl BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmTransmitterEvent> {
    /// Return the exact standard pattern retained from packet preparation.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { pattern, .. } => pattern,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }

    /// Return the exact payload length retained from packet preparation.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { length, .. } => length,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }
}

/// CPU-owned DTM graph after the reviewed scheduler bookkeeping prefix.
///
/// Forming its lock/modify request is still not hardware publication. The
/// complete descriptor, private packet-engine latch and visibility fence are
/// deliberately absent from this state.
#[must_use = "the scheduler-prepared DTM graph must remain owned or be cancelled"]
pub struct BluetoothDtmSchedulerBookkeepingPrepared<Role> {
    memory: BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    packet: BluetoothDtmEventPacket,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
    /// Return the role shared by the retained DTM transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventPacket::Receiver => BluetoothDtmRole::Receiver,
        }
    }

    /// Return the typed controller-SRAM identity of the retained item.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Form the reviewed argument-zero DTM insertion request for this graph.
    ///
    /// Complete DTM call-path evidence supplies positional argument zero to
    /// the common forced-insertion path. This method performs no MMIO and does
    /// not change CPU ownership.
    ///
    /// This remains crate-private until the lower memory layer represents the
    /// complete hardware-consumed descriptor and publication boundary. It is
    /// currently used only to retain the exact request identity in ownership
    /// tests; external controller code cannot submit an incomplete graph.
    #[allow(
        dead_code,
        reason = "publication stays closed until the complete hardware-consumed descriptor exists"
    )]
    pub(crate) const fn scheduler_lock_modify_request(
        &self,
    ) -> BluetoothSchedulerLockModifyRequest {
        match BluetoothSchedulerLockModifyRequest::new(self.scheduler_item_address(), 0) {
            Ok(request) => request,
            Err(_) => unreachable!(),
        }
    }

    /// Cancel before publication and recover the prepared event words.
    pub fn cancel(self) -> BluetoothDtmReviewedEventWordsPrepared<Role> {
        BluetoothDtmReviewedEventWordsPrepared {
            memory: self.memory.cancel(),
            packet: self.packet,
            _role: PhantomData,
        }
    }
}

impl BluetoothDtmSchedulerBookkeepingPrepared<BluetoothDtmTransmitterEvent> {
    /// Return the exact standard pattern retained through bookkeeping.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { pattern, .. } => pattern,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }

    /// Return the exact payload length retained through bookkeeping.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { length, .. } => length,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmBoundSramLinkAddress, BluetoothDtmMemoryGraphModelAddress,
        BluetoothDtmMemoryGraphStorage, BluetoothDtmSchedulerAllocationConfig,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{BluetoothDtmReviewedEventWordsPlan, BluetoothDtmReviewedEventWordsPlanError};
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern,
        BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmSchedulerItemEvent,
        BluetoothDtmTxGraphPrepare,
    };

    fn owner(base: u32) -> crate::BluetoothDtmMemoryGraphCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(base)
            .expect("test base has valid compressed-pointer syntax");
        BluetoothDtmMemoryGraphStorage::pin_static_model(
            storage,
            base,
            BluetoothDtmSchedulerAllocationConfig::new(2, 3, 5, 4),
        )
        .expect("test graph fits physical controller SRAM")
    }

    fn epoch() -> BluetoothControllerSchedulerEpoch {
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        )
    }

    fn item(role: BluetoothDtmRole) -> BluetoothDtmSchedulerItemEvent {
        BluetoothDtmSchedulerItemEvent::new(
            BluetoothDtmChannel::new(5).expect("channel five is valid"),
            match role {
                BluetoothDtmRole::Transmitter => BluetoothDtmPhy::Le2M,
                BluetoothDtmRole::Receiver => BluetoothDtmPhy::LeCoded,
            },
            role,
            1_012,
            1_020,
        )
        .expect("selected PHY is valid for its role")
    }

    #[test]
    fn tx_plan_requires_and_retains_the_prepared_packet_identity() {
        let stale = BluetoothDtmBoundSramLinkAddress::new(0x2f00_0400)
            .expect("stale model link remains syntactically valid");
        let reset = BluetoothDtmLinkStateReset::new(
            Some(stale),
            Some(stale),
            0x15,
            0x2a,
            BluetoothDtmRole::Transmitter,
        )
        .expect("bounded reset fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_transmitter(
            reset,
            item(BluetoothDtmRole::Transmitter),
            epoch(),
        )
        .expect("both transforms encode TX");

        let packet = owner(0x2f07_0000).prepare_dtm_tx_packet(
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadLength::from_hci_image(3),
        );
        assert_eq!(&packet.prepared_bytes()[0x12..], &[0x0f; 3]);

        let prepared = plan
            .prepare(packet)
            .expect("fresh private links replace both stale plan links");
        assert_eq!(prepared.role(), BluetoothDtmRole::Transmitter);
        assert_eq!(
            prepared.packet_pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(prepared.packet_length().hci_image(), 3);
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        assert_eq!(
            scheduler_prepared.packet_pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(scheduler_prepared.packet_length().hci_image(), 3);
        let request = scheduler_prepared.scheduler_lock_modify_request();
        assert_eq!(
            request.address(),
            scheduler_prepared.scheduler_item_address()
        );
        assert_eq!(request.argument(), 0);
        let prepared = scheduler_prepared.cancel();
        let words = prepared.words();
        assert_eq!(words.link_state().word_00, 0x8ff1_c057);
        assert_eq!(words.link_state().tx_head_link_image(), 0x1c057);
        assert_eq!(words.link_state().word_08, 0x0ff1_c04b);
        assert_eq!(words.scheduler_item().link_state_link_image(), 0x1c000);
        assert_eq!(words.scheduler_item().word_14, 0x5150_0000);
        assert_eq!(words.scheduler_item().word_2c, 0);
        let _owner = prepared.cancel();
    }

    #[test]
    fn rx_plan_applies_both_role_specific_transforms_to_the_bound_graph() {
        let reset = BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Receiver)
            .expect("zero dynamic fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
            reset,
            item(BluetoothDtmRole::Receiver),
            epoch(),
        )
        .expect("both transforms encode RX");

        let prepared = plan
            .prepare(owner(0x2f00_0100))
            .expect("current graph anchors satisfy the generated RX image");
        assert_eq!(prepared.role(), BluetoothDtmRole::Receiver);
        let words = prepared.words();
        assert_eq!(words.link_state().tx_head_link_image(), 0x097);
        assert_eq!(words.link_state().rx_tail_link_image(), 0x08b);
        assert_eq!(words.scheduler_item().link_state_link_image(), 0x040);
        assert_eq!(words.scheduler_item().word_14, 0xf000_0000);
        assert_eq!(words.scheduler_item().word_2c, 0x000f_0001);
        assert_eq!(words.scheduler_item().word_44, 103);
        assert_eq!(words.scheduler_item().word_48, 105);
    }

    #[test]
    fn plan_rejects_mixed_roles_before_it_can_consume_memory() {
        let reset =
            BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Transmitter)
                .expect("zero dynamic fields are valid");
        assert_eq!(
            BluetoothDtmReviewedEventWordsPlan::new_transmitter(
                reset,
                item(BluetoothDtmRole::Receiver),
                epoch(),
            ),
            Err(BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
                expected: BluetoothDtmRole::Transmitter,
                link_state: BluetoothDtmRole::Transmitter,
                scheduler_item: BluetoothDtmRole::Receiver,
            })
        );
    }
}
