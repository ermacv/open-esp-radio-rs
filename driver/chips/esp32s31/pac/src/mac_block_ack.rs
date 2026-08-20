//! Generated-PAC ownership for the finite MAC BlockAck register leaves.

#![forbid(unsafe_code)]

use super::{
    MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacRxBlockAckEntryIndex,
    MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow, RadioRegisters,
};

/// Result sampled for one completed TX hardware queue.
///
/// SOURCE: complete `libpp.a[hal_debug.o]::dbg_read_rx_ba`.
/// The hot TX completion path samples the three BlockAck payload words plus
/// the independent hardware result bit. Use [`TxBlockAckDiagnosticSnapshot`]
/// when the transmitter address and queues four through seven are needed for
/// diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckRegisterImage {
    pub control_and_sequence: u32,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
    pub block_ack_received: bool,
}

/// Three-word TX BlockAck payload sampled without adjacent validity state.
///
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::hal_mac_tx_get_blockack`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckPayload {
    pub control_and_sequence: u32,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
}

/// Complete five-word `WDEVTXQBA` result plus adjacent TX queue information.
///
/// SOURCE: complete `libpp.a[hal_debug.o]::dbg_read_rx_ba`.
/// Despite the function name, its strings identify eight reverse-addressed
/// TX BlockAck result banks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckDiagnosticSnapshot {
    pub control_and_sequence: u32,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
    pub transmitter_address: [u8; 6],
    pub acknowledgement_tid: u8,
    pub acknowledgement_received: bool,
    pub block_ack_received: bool,
    pub last_tx_was_trigger_based: bool,
    pub trigger_based_packet_count: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalTxBlockAckSnapshot {
    pub bitmap: u64,
    pub transmitter_address_words: [u32; 2],
    pub fragment_number: u8,
    pub starting_sequence: u16,
    pub tid: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RxBlockAckImages {
    peer_head: u32,
    peer_tail_and_policy: u32,
    active_control: u32,
}

/// Live image of one ordinary receive BlockAck hardware bank.
///
/// SOURCE: complete `libpp.a[hal_ampdu.o]::hal_ba_session_store`.
/// The status and load words are deliberately exposed separately: the blob
/// snapshots the hardware-maintained words and restores them through their
/// adjacent software-load words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckEntrySnapshot {
    pub control: u32,
    pub peer: [u8; 6],
    pub interface: MacInterface,
    pub window: u8,
    pub current_sequence: u16,
    pub loaded_start_sequence: u16,
    pub bitmap_status: u64,
    pub bitmap_load: u64,
}

/// Latched projection of one indirect extra-SoftAP receive BlockAck bank.
///
/// SOURCE: complete `libpp.a[hal_ampdu.o]::
/// hal_debug_read_extra_softap_rx_ba` selects each logical index through
/// control bits 5..9, asserts agreement-update bit nine, reads the control,
/// peer, policy and starting-sequence staging words, then releases the latch.
/// The two bitmap words are the adjacent raw staging words used by complete
/// `hal_agreement_clr_extra_softap_rx_ba`; they remain raw observations rather
/// than a claimed protocol interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtraSoftApRxBlockAckEntrySnapshot {
    pub control: u32,
    pub peer: [u8; 6],
    pub interface: MacInterface,
    pub window: u8,
    pub starting_sequence: u16,
    pub bitmap_staging_words: [u32; 2],
}

const fn rx_block_ack_images(
    interface: MacInterface,
    peer: [u8; 6],
    tid: MacRxBlockAckTid,
    window: MacRxBlockAckWindow,
) -> RxBlockAckImages {
    let peer_head = u32::from_le_bytes([peer[0], peer[1], peer[2], peer[3]]);
    let peer_tail = u16::from_le_bytes([peer[4], peer[5]]) as u32;
    let peer_tail_and_policy = peer_tail | (interface.bits() << 16) | (window.get() << 18);
    let active_control = 0xc000_0001 | (tid.get() << 12);
    RxBlockAckImages {
        peer_head,
        peer_tail_and_policy,
        active_control,
    }
}

const fn rx_block_ack_register_index(hardware_index: MacRxBlockAckEntryIndex) -> usize {
    7 - hardware_index.get() as usize
}

impl RadioRegisters {
    /// Sample the three words copied by `hal_mac_tx_get_blockack`.
    pub fn read_tx_block_ack_payload(&self, hardware_queue: u8) -> Option<TxBlockAckPayload> {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let (control_and_sequence, bitmap_low, bitmap_high) = match hardware_queue {
            0 => (
                block.tx_block_ack_control_sequence_q0().read().bits(),
                block.tx_block_ack_bitmap_low_q0().read().bits(),
                block.tx_block_ack_bitmap_high_q0().read().bits(),
            ),
            1 => (
                block.tx_block_ack_control_sequence_q1().read().bits(),
                block.tx_block_ack_bitmap_low_q1().read().bits(),
                block.tx_block_ack_bitmap_high_q1().read().bits(),
            ),
            2 => (
                block.tx_block_ack_control_sequence_q2().read().bits(),
                block.tx_block_ack_bitmap_low_q2().read().bits(),
                block.tx_block_ack_bitmap_high_q2().read().bits(),
            ),
            3 => (
                block.tx_block_ack_control_sequence_q3().read().bits(),
                block.tx_block_ack_bitmap_low_q3().read().bits(),
                block.tx_block_ack_bitmap_high_q3().read().bits(),
            ),
            _ => return None,
        };
        Some(TxBlockAckPayload {
            control_and_sequence,
            bitmap_low,
            bitmap_high,
        })
    }

    /// Program one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_add_rx_ba`, size `0x12e`.
    ///
    /// The eight entries are direct reverse-addressed banks, not the shared
    /// extra-SoftAP staging window at `0x2010_4ea4`. Logical index zero lives
    /// at `0x2010_4274`; each successive index subtracts `0x24`.
    pub fn program_rx_block_ack_entry(
        &mut self,
        hardware_index: MacRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let register_index = rx_block_ack_register_index(hardware_index);
        let images = rx_block_ack_images(interface, peer, tid, window);
        let peer_tail = images.peer_tail_and_policy as u16;

        super::svd::zero_based_field_write::rx_block_ack_peer_head(
            block,
            register_index,
            images.peer_head,
        );
        super::svd::zero_based_field_write::rx_block_ack_peer_tail_and_policy(
            block,
            register_index,
            peer_tail,
            interface.bits() as u8,
            window.get() as u8,
        );
        block
            .rx_block_ack_entry_start_sequence_load(register_index)
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().clear_bit());
        super::svd::zero_register_write::clear_rx_block_ack_bitmap_low_load(block, register_index);
        super::svd::zero_register_write::clear_rx_block_ack_bitmap_high_load(block, register_index);
        let update_bit = 1_u8 << hardware_index.get();
        // The checked index keeps the OR result inside the eight-bit field.
        // This preserves the blob's fresh-read OR operation.
        block.rx_block_ack_agreement_update().modify(|r, w| {
            w.ordinary_entry_update()
                .set(r.ordinary_entry_update().bits() | update_bit)
        });
        super::svd::zero_based_field_write::rx_block_ack_active_control(
            block,
            register_index,
            true,
            tid.get() as u8,
            true,
            true,
        );
    }

    /// Delete one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_del_rx_ba`, size `0x72`.
    pub fn delete_rx_block_ack_entry(&mut self, hardware_index: MacRxBlockAckEntryIndex) {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let register_index = rx_block_ack_register_index(hardware_index);
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().clear_bit());
        super::svd::zero_register_write::clear_rx_block_ack_bitmap_low_load(block, register_index);
        super::svd::zero_register_write::clear_rx_block_ack_bitmap_high_load(block, register_index);
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().set_bit());
        // The final full-word zero is a distinct observable edge in the blob.
        super::svd::zero_register_write::clear_rx_block_ack_entry_control(block, register_index);
    }

    /// Move one live ordinary receive BlockAck entry to a new window.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_clr_rx_ba`, size `0x92`.
    ///
    /// The vendor clears validity with a full-word zero, reloads the starting
    /// sequence and physical window, clears both bitmap load words, publishes
    /// the ordinary-bank update bit, and finally restores the active control
    /// word. Peer and interface fields remain unchanged.
    pub fn reset_rx_block_ack_window(
        &mut self,
        hardware_index: MacRxBlockAckEntryIndex,
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let register_index = rx_block_ack_register_index(hardware_index);

        super::svd::zero_register_write::clear_rx_block_ack_entry_control(block, register_index);
        block
            .rx_block_ack_entry_start_sequence_load(register_index)
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        block
            .rx_block_ack_entry_peer_tail_and_policy(register_index)
            .modify(|_, w| w.window().set(window.get() as u8));
        super::svd::zero_register_write::clear_rx_block_ack_bitmap_low_load(block, register_index);
        super::svd::zero_register_write::clear_rx_block_ack_bitmap_high_load(block, register_index);
        let update_bit = 1_u8 << hardware_index.get();
        block.rx_block_ack_agreement_update().modify(|r, w| {
            w.ordinary_entry_update()
                .set(r.ordinary_entry_update().bits() | update_bit)
        });
        super::svd::zero_based_field_write::rx_block_ack_active_control(
            block,
            register_index,
            true,
            tid.get() as u8,
            true,
            true,
        );
    }

    /// Program one extra-SoftAP receive BlockAck entry through the shared
    /// staging window.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_add_extra_softap_rx_ba`, size `0x130`.
    ///
    /// This transaction is distinct from the eight direct ordinary banks.
    /// The selected entry is staged, committed through update bit eight,
    /// latched for the vendor's readback sequence through bit nine, and then
    /// released. The diagnostic log call is intentionally outside the PAC;
    /// its ordered volatile readback is retained here.
    pub fn program_extra_softap_rx_block_ack_entry(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) -> bool {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let index = hardware_index.get() as u8;
        let images = rx_block_ack_images(interface, peer, tid, window);

        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        super::svd::zero_based_field_write::extra_softap_rx_block_ack_peer_head(
            block,
            images.peer_head,
        );
        super::svd::zero_based_field_write::extra_softap_rx_block_ack_peer_tail_and_policy(
            block,
            images.peer_tail_and_policy as u16,
            interface.bits() as u8,
            window.get() as u8,
        );
        block
            .extra_softap_rx_block_ack_start_sequence()
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        super::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_low(block);
        super::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_high(block);
        super::svd::zero_based_field_write::extra_softap_rx_block_ack_active_control(
            block,
            true,
            index,
            tid.get() as u8,
            true,
            true,
        );

        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().set_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().clear_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());

        // Preserve the complete vendor readback order. The values feed only
        // wifi_log in the blob and therefore do not cross this PAC boundary.
        let peer_tail_and_policy = block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .read()
            .bits();
        let peer_head = block.extra_softap_rx_block_ack_peer_head().read().bits();
        let observed_starting_sequence = block
            .extra_softap_rx_block_ack_start_sequence()
            .read()
            .sequence()
            .bits();
        let _ = block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .read();
        let control = block.extra_softap_rx_block_ack_control().read().bits();

        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());

        peer_head == images.peer_head
            && peer_tail_and_policy == images.peer_tail_and_policy
            && observed_starting_sequence == starting_sequence.get() as u16
            && control == (images.active_control | (u32::from(index) << 5))
    }

    /// Move one live extra-SoftAP receive BlockAck entry to a new window.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_clr_extra_softap_rx_ba`, size `0xc6`.
    ///
    /// Despite the vendor leaf's `clr` name, this does not delete the entry.
    /// It preserves peer, interface, TID and validity, replaces the starting
    /// sequence and hardware window, clears the accumulated bitmap, and
    /// commits the staged image. The vendor's TID argument feeds only its log
    /// call and therefore does not cross this register-only boundary.
    pub fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let index = hardware_index.get() as u8;

        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());
        let _ = block.extra_softap_rx_block_ack_control().read();
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());
        block
            .extra_softap_rx_block_ack_start_sequence()
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .modify(|_, w| w.window().set(window.get() as u8));
        super::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_low(block);
        super::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_high(block);
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().set_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().clear_bit());
    }

    /// Latch and sample one indirect extra-SoftAP agreement without changing
    /// its committed contents.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
    ) -> ExtraSoftApRxBlockAckEntrySnapshot {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let index = hardware_index.get() as u8;
        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());

        // Preserve the complete vendor diagnostic read order before sampling
        // the two adjacent staging words.
        let control = block.extra_softap_rx_block_ack_control().read().bits();
        let peer_head = block.extra_softap_rx_block_ack_peer_head().read().bits();
        let peer_tail_and_policy = block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .read();
        let starting_sequence = block
            .extra_softap_rx_block_ack_start_sequence()
            .read()
            .sequence()
            .bits();
        let peer_head_again = block.extra_softap_rx_block_ack_peer_head().read().bits();
        let control_again = block.extra_softap_rx_block_ack_control().read().bits();
        let bitmap_staging_words = [
            block.extra_softap_rx_block_ack_bitmap_low().read().bits(),
            block.extra_softap_rx_block_ack_bitmap_high().read().bits(),
        ];

        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());

        debug_assert_eq!(peer_head, peer_head_again);
        debug_assert_eq!(control, control_again);
        let head = peer_head.to_le_bytes();
        let tail = peer_tail_and_policy
            .peer_address_tail()
            .bits()
            .to_le_bytes();
        ExtraSoftApRxBlockAckEntrySnapshot {
            control,
            peer: [head[0], head[1], head[2], head[3], tail[0], tail[1]],
            interface: match peer_tail_and_policy.interface().bits() {
                0 => MacInterface::Station,
                1 => MacInterface::AccessPoint,
                2 => MacInterface::Context2,
                3 => MacInterface::Context3,
                _ => unreachable!("two-bit interface field cannot exceed three"),
            },
            window: peer_tail_and_policy.window().bits(),
            starting_sequence,
            bitmap_staging_words,
        }
    }

    /// Delete one extra-SoftAP receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_del_extra_softap_rx_ba`, size `0x94`.
    pub fn delete_extra_softap_rx_block_ack_entry(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
    ) {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let index = hardware_index.get() as u8;

        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());
        let _ = block.extra_softap_rx_block_ack_control().read();
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());
        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.valid().clear_bit());
        super::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_low(block);
        super::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_high(block);
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().set_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().clear_bit());
    }

    /// Sample both hardware-maintained and software-load words of one entry.
    pub fn rx_block_ack_entry_snapshot(
        &self,
        hardware_index: MacRxBlockAckEntryIndex,
    ) -> Option<RxBlockAckEntrySnapshot> {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let register_index = rx_block_ack_register_index(hardware_index);
        let control = block
            .rx_block_ack_entry_control(register_index)
            .read()
            .bits();
        let peer_head = block
            .rx_block_ack_entry_peer_head(register_index)
            .read()
            .bits()
            .to_le_bytes();
        let peer_tail_and_policy = block
            .rx_block_ack_entry_peer_tail_and_policy(register_index)
            .read();
        let peer_tail = peer_tail_and_policy
            .peer_address_tail()
            .bits()
            .to_le_bytes();
        let bitmap_status = u64::from(
            block
                .rx_block_ack_entry_bitmap_low_status(register_index)
                .read()
                .bits(),
        ) | (u64::from(
            block
                .rx_block_ack_entry_bitmap_high_status(register_index)
                .read()
                .bits(),
        ) << 32);
        let bitmap_load = u64::from(
            block
                .rx_block_ack_entry_bitmap_low_load(register_index)
                .read()
                .bits(),
        ) | (u64::from(
            block
                .rx_block_ack_entry_bitmap_high_load(register_index)
                .read()
                .bits(),
        ) << 32);
        Some(RxBlockAckEntrySnapshot {
            control,
            peer: [
                peer_head[0],
                peer_head[1],
                peer_head[2],
                peer_head[3],
                peer_tail[0],
                peer_tail[1],
            ],
            interface: match peer_tail_and_policy.interface().bits() {
                0 => MacInterface::Station,
                1 => MacInterface::AccessPoint,
                2 => MacInterface::Context2,
                3 => MacInterface::Context3,
                _ => unreachable!("two-bit interface field cannot exceed three"),
            },
            window: peer_tail_and_policy.window().bits(),
            current_sequence: block
                .rx_block_ack_entry_current_sequence(register_index)
                .read()
                .sequence()
                .bits(),
            loaded_start_sequence: block
                .rx_block_ack_entry_start_sequence_load(register_index)
                .read()
                .sequence()
                .bits(),
            bitmap_status,
            bitmap_load,
        })
    }

    /// Sample the TX BlockAck payload and its independent validity result.
    pub fn read_tx_block_ack_registers(
        &self,
        hardware_queue: u8,
    ) -> Option<TxBlockAckRegisterImage> {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let payload = self.read_tx_block_ack_payload(hardware_queue)?;
        let address_high = match hardware_queue {
            0 => block
                .tx_block_ack_transmitter_address_high_q0()
                .read()
                .bits(),
            1 => block
                .tx_block_ack_transmitter_address_high_q1()
                .read()
                .bits(),
            2 => block
                .tx_block_ack_transmitter_address_high_q2()
                .read()
                .bits(),
            3 => block
                .tx_block_ack_transmitter_address_high_q3()
                .read()
                .bits(),
            _ => return None,
        };
        Some(TxBlockAckRegisterImage {
            control_and_sequence: payload.control_and_sequence,
            bitmap_low: payload.bitmap_low,
            bitmap_high: payload.bitmap_high,
            block_ack_received: address_high & (1 << 21) != 0,
        })
    }

    /// Sample one complete TX BlockAck result and its queue-information word.
    ///
    /// SOURCE: complete `libpp.a[hal_debug.o]::dbg_read_rx_ba`.
    /// Queue zero occupies `0x2010_5528..0x2010_5538`; logical queues descend
    /// by `0x7c` through queue seven at `0x2010_51c4..0x2010_51d4`.
    pub fn tx_block_ack_diagnostic_snapshot(
        &self,
        hardware_queue: u8,
    ) -> Option<TxBlockAckDiagnosticSnapshot> {
        let block = &self.peripherals.wifi_mac_rx_dma;
        let (
            control_and_sequence,
            bitmap_low,
            bitmap_high,
            address_high,
            address_low,
            queue_information,
        ) = match hardware_queue {
            0 => (
                block.tx_block_ack_control_sequence_q0().read().bits(),
                block.tx_block_ack_bitmap_low_q0().read().bits(),
                block.tx_block_ack_bitmap_high_q0().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q0()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q0()
                    .read()
                    .bits(),
                block.tx_queue_information_q0().read().bits(),
            ),
            1 => (
                block.tx_block_ack_control_sequence_q1().read().bits(),
                block.tx_block_ack_bitmap_low_q1().read().bits(),
                block.tx_block_ack_bitmap_high_q1().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q1()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q1()
                    .read()
                    .bits(),
                block.tx_queue_information_q1().read().bits(),
            ),
            2 => (
                block.tx_block_ack_control_sequence_q2().read().bits(),
                block.tx_block_ack_bitmap_low_q2().read().bits(),
                block.tx_block_ack_bitmap_high_q2().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q2()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q2()
                    .read()
                    .bits(),
                block.tx_queue_information_q2().read().bits(),
            ),
            3 => (
                block.tx_block_ack_control_sequence_q3().read().bits(),
                block.tx_block_ack_bitmap_low_q3().read().bits(),
                block.tx_block_ack_bitmap_high_q3().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q3()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q3()
                    .read()
                    .bits(),
                block.tx_queue_information_q3().read().bits(),
            ),
            4 => (
                block.tx_block_ack_control_sequence_q4().read().bits(),
                block.tx_block_ack_bitmap_low_q4().read().bits(),
                block.tx_block_ack_bitmap_high_q4().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q4()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q4()
                    .read()
                    .bits(),
                block.tx_queue_information_q4().read().bits(),
            ),
            5 => (
                block.tx_block_ack_control_sequence_q5().read().bits(),
                block.tx_block_ack_bitmap_low_q5().read().bits(),
                block.tx_block_ack_bitmap_high_q5().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q5()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q5()
                    .read()
                    .bits(),
                block.tx_queue_information_q5().read().bits(),
            ),
            6 => (
                block.tx_block_ack_control_sequence_q6().read().bits(),
                block.tx_block_ack_bitmap_low_q6().read().bits(),
                block.tx_block_ack_bitmap_high_q6().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q6()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q6()
                    .read()
                    .bits(),
                block.tx_queue_information_q6().read().bits(),
            ),
            7 => (
                block.tx_block_ack_control_sequence_q7().read().bits(),
                block.tx_block_ack_bitmap_low_q7().read().bits(),
                block.tx_block_ack_bitmap_high_q7().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q7()
                    .read()
                    .bits(),
                block
                    .tx_block_ack_transmitter_address_low_q7()
                    .read()
                    .bits(),
                block.tx_queue_information_q7().read().bits(),
            ),
            _ => return None,
        };
        let low = address_low.to_le_bytes();
        let high = address_high.to_le_bytes();
        Some(TxBlockAckDiagnosticSnapshot {
            control_and_sequence,
            bitmap_low,
            bitmap_high,
            transmitter_address: [low[0], low[1], low[2], low[3], high[0], high[1]],
            acknowledgement_tid: ((address_high >> 16) & 0x0f) as u8,
            acknowledgement_received: address_high & (1 << 20) != 0,
            block_ack_received: address_high & (1 << 21) != 0,
            last_tx_was_trigger_based: queue_information & (1 << 20) != 0,
            trigger_based_packet_count: ((queue_information >> 13) & 0x7f) as u8,
        })
    }

    /// Sample the standalone internal WDEVTXBA result.
    ///
    /// SOURCE: complete `libpp.a[hal_debug.o]::
    /// dbg_read_internal_txba`; all five addresses and three masks are exact.
    /// The blob does not name byte ordering inside the two TA words, so they
    /// remain raw.
    pub fn internal_tx_block_ack_snapshot(&self) -> InternalTxBlockAckSnapshot {
        let block = &self.peripherals.wifi_mac_internal_tx_block_ack;
        let control = block.control_sequence().read();
        InternalTxBlockAckSnapshot {
            bitmap: u64::from(block.bitmap_low().read().bits())
                | (u64::from(block.bitmap_high().read().bits()) << 32),
            transmitter_address_words: [
                block.transmitter_address_word_0().read().bits(),
                block.transmitter_address_word_1().read().bits(),
            ],
            fragment_number: control.fragment_number().bits(),
            starting_sequence: control.starting_sequence().bits(),
            tid: control.tid().bits(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacRxBlockAckTid, MacRxBlockAckWindow,
        RxBlockAckImages, rx_block_ack_images,
    };

    #[test]
    fn rx_block_ack_images_match_the_recovered_leaf() {
        assert_eq!(
            rx_block_ack_images(
                MacInterface::AccessPoint,
                [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0],
                MacRxBlockAckTid::new(6).unwrap(),
                MacRxBlockAckWindow::new(16).unwrap(),
            ),
            RxBlockAckImages {
                peer_head: 0xa8fb_1570,
                peer_tail_and_policy: 0x0041_f048,
                active_control: 0xc000_6001,
            }
        );
    }

    #[test]
    fn extra_softap_entry_domain_matches_the_vendor_allocator() {
        assert!(MacExtraSoftApRxBlockAckEntryIndex::new(0).is_some());
        assert!(MacExtraSoftApRxBlockAckEntryIndex::new(7).is_some());
        assert!(MacExtraSoftApRxBlockAckEntryIndex::new(8).is_none());
    }
}
