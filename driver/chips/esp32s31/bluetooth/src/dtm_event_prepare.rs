//! Role-consistent composition of reviewed DTM event words into bound memory.
//!
//! This layer combines already validated LLL transforms with the lower
//! consuming memory transaction. The resulting state remains CPU-only and
//! covers control words only: it does not prove TX packet preparation, list
//! insertion, a visibility fence, a hardware latch or completion ownership.

#![forbid(unsafe_code)]

use core::convert::Infallible;

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPositionalEventPrepared,
    BluetoothDtmMemoryGraphPrepareFailure, BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    BluetoothDtmPositionalEventWords,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerLockModifyRequest,
};

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothDtmLinkStateReset, BluetoothDtmRole,
    BluetoothDtmSchedulerItemEvent,
};

/// Why two validated DTM transforms cannot describe one event plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReviewedEventWordsPlanError {
    /// Link-state and scheduler-item transforms encode different DTM roles.
    RoleMismatch {
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
pub struct BluetoothDtmReviewedEventWordsPlan {
    link_state: BluetoothDtmLinkStateReset,
    scheduler_item: BluetoothDtmSchedulerItemEvent,
    epoch: BluetoothControllerSchedulerEpoch,
    role: BluetoothDtmRole,
}

impl BluetoothDtmReviewedEventWordsPlan {
    /// Pair both reviewed transforms only when they encode the same role.
    pub const fn new(
        link_state: BluetoothDtmLinkStateReset,
        scheduler_item: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanError> {
        let role = link_state.role();
        let scheduler_role = scheduler_item.role();
        match (role, scheduler_role) {
            (BluetoothDtmRole::Transmitter, BluetoothDtmRole::Transmitter)
            | (BluetoothDtmRole::Receiver, BluetoothDtmRole::Receiver) => {}
            _ => {
                return Err(BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
                    link_state: role,
                    scheduler_item: scheduler_role,
                });
            }
        }
        Ok(Self {
            link_state,
            scheduler_item,
            epoch,
            role,
        })
    }

    /// Apply the plan to one exact graph through its consuming transaction.
    ///
    /// Any lower validation failure retains the byte-unchanged original owner.
    pub fn prepare(
        self,
        owner: BluetoothDtmMemoryGraphCpuOwned,
    ) -> Result<BluetoothDtmReviewedEventWordsPrepared, BluetoothDtmMemoryGraphPrepareFailure> {
        let prepared = owner.try_prepare_positional_event(|seed| {
            let current = seed.words();
            let link_state = self
                .link_state
                .with_private_links(seed.tx_head(), seed.rx_tail())
                .apply(current.link_state());
            let scheduler_item = self
                .scheduler_item
                .apply(current.scheduler_item(), self.epoch);
            Ok::<_, Infallible>(BluetoothDtmPositionalEventWords::new(
                link_state,
                scheduler_item,
            ))
        })?;

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            role: self.role,
        })
    }
}

/// CPU-owned bound graph containing one role-consistent reviewed word image.
///
/// This type intentionally omits packet access and every controller address or
/// publication operation. For TX, packet-pattern preparation remains a
/// separate prerequisite that this word-only state does not claim.
pub struct BluetoothDtmReviewedEventWordsPrepared {
    memory: BluetoothDtmMemoryGraphPositionalEventPrepared,
    role: BluetoothDtmRole,
}

impl BluetoothDtmReviewedEventWordsPrepared {
    /// Return the role shared by both applied transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.role
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
    pub fn prepare_scheduler_bookkeeping(self) -> BluetoothDtmSchedulerBookkeepingPrepared {
        BluetoothDtmSchedulerBookkeepingPrepared {
            memory: self.memory.prepare_scheduler_bookkeeping(),
            role: self.role,
        }
    }

    /// Cancel before publication and recover the exact prior CPU-owned graph.
    pub fn cancel(self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.memory.cancel()
    }
}

/// CPU-owned DTM graph after the reviewed scheduler bookkeeping prefix.
///
/// Forming its lock/modify request is still not hardware publication. The
/// complete descriptor, private packet-engine latch and visibility fence are
/// deliberately absent from this state.
#[must_use = "the scheduler-prepared DTM graph must remain owned or be cancelled"]
pub struct BluetoothDtmSchedulerBookkeepingPrepared {
    memory: BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    role: BluetoothDtmRole,
}

impl BluetoothDtmSchedulerBookkeepingPrepared {
    /// Return the role shared by the retained DTM transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.role
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
    pub(crate) const fn scheduler_lock_modify_request(
        &self,
    ) -> BluetoothSchedulerLockModifyRequest {
        match BluetoothSchedulerLockModifyRequest::new(self.scheduler_item_address(), 0) {
            Ok(request) => request,
            Err(_) => unreachable!(),
        }
    }

    /// Cancel before publication and recover the prepared event words.
    pub fn cancel(self) -> BluetoothDtmReviewedEventWordsPrepared {
        BluetoothDtmReviewedEventWordsPrepared {
            memory: self.memory.cancel(),
            role: self.role,
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
        BluetoothDtmTxPacketPrepare,
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
    fn tx_plan_rebinds_stale_links_and_preserves_the_prepared_packet_slot() {
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
        let plan = BluetoothDtmReviewedEventWordsPlan::new(
            reset,
            item(BluetoothDtmRole::Transmitter),
            epoch(),
        )
        .expect("both transforms encode TX");

        let mut owner = owner(0x2f07_0000);
        let packet = owner.tx_packet_mut().prepare(
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadLength::from_hci_image(3),
        );
        assert_eq!(packet.payload(), [0x0f; 3]);
        packet.release();

        let prepared = plan
            .prepare(owner)
            .expect("fresh private links replace both stale plan links");
        assert_eq!(prepared.role(), BluetoothDtmRole::Transmitter);
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        let request = scheduler_prepared.scheduler_lock_modify_request();
        assert_eq!(
            request.address(),
            scheduler_prepared.scheduler_item_address()
        );
        assert_eq!(request.argument(), 0);
        let prepared = scheduler_prepared.cancel();
        let words = prepared.words();
        assert_eq!(words.link_state().word_00, 0x8ff1_c057);
        assert_eq!(words.link_state().word_00 & 0x000f_ffff, 0x1c057);
        assert_eq!(words.link_state().word_08, 0x0ff1_c04b);
        assert_eq!(words.scheduler_item().word_08 & 0x000f_ffff, 0x1c000);
        assert_eq!(words.scheduler_item().word_00 & 0x0050_0000, 0x0010_0000);
        assert_eq!(words.scheduler_item().word_2c, 0);

        let mut owner = prepared.cancel();
        let packet = owner.tx_packet_mut().prepare(
            BluetoothDtmPayloadPattern::RepeatedAllZeros,
            BluetoothDtmPayloadLength::from_hci_image(0),
        );
        let storage = packet.release();
        assert_eq!(storage.bytes()[0x12..0x15], [0x0f; 3]);
    }

    #[test]
    fn rx_plan_applies_both_role_specific_transforms_to_the_bound_graph() {
        let reset = BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Receiver)
            .expect("zero dynamic fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new(
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
        assert_eq!(words.link_state().word_00 & 0x000f_ffff, 0x097);
        assert_eq!(words.link_state().word_08 & 0x000f_ffff, 0x08b);
        assert_eq!(words.scheduler_item().word_08 & 0x000f_ffff, 0x040);
        assert_eq!(words.scheduler_item().word_00 & 0x0050_0000, 0x0040_0000);
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
            BluetoothDtmReviewedEventWordsPlan::new(
                reset,
                item(BluetoothDtmRole::Receiver),
                epoch(),
            ),
            Err(BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
                link_state: BluetoothDtmRole::Transmitter,
                scheduler_item: BluetoothDtmRole::Receiver,
            })
        );
    }
}
