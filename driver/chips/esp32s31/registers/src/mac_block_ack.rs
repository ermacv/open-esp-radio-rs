//! Generated-PAC ownership for the finite MAC BlockAck register leaves.

#![forbid(unsafe_code)]

use super::RadioRegisters;

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
    pub interface: u8,
    pub window: u8,
    pub current_sequence: u16,
    pub loaded_start_sequence: u16,
    pub bitmap_status: u64,
    pub bitmap_load: u64,
}

const fn rx_block_ack_images(
    interface: u8,
    peer: [u8; 6],
    tid: u8,
    window: u16,
) -> RxBlockAckImages {
    let peer_head = u32::from_le_bytes([peer[0], peer[1], peer[2], peer[3]]);
    let peer_tail = u16::from_le_bytes([peer[4], peer[5]]) as u32;
    let peer_tail_and_policy = peer_tail | ((interface as u32) << 16) | ((window as u32) << 18);
    let active_control = 0xc000_0001 | ((tid as u32) << 12);
    RxBlockAckImages {
        peer_head,
        peer_tail_and_policy,
        active_control,
    }
}

const fn rx_block_ack_register_index(hardware_index: u8) -> usize {
    7 - hardware_index as usize
}

impl RadioRegisters {
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
        hardware_index: u8,
        interface: u8,
        peer: [u8; 6],
        tid: u8,
        starting_sequence: u16,
        window: u16,
    ) {
        assert!(hardware_index < 8);
        assert!(interface <= 3);
        assert!(tid <= 15);
        assert!(starting_sequence <= 0x0fff);
        assert!((1..=0x7f).contains(&window));

        let block = &self.peripherals.wifi_mac_rx_dma;
        let register_index = rx_block_ack_register_index(hardware_index);
        let images = rx_block_ack_images(interface, peer, tid, window);
        let peer_tail = images.peer_tail_and_policy as u16;

        open_esp_radio_esp32s31_pac::zero_based_field_write::rx_block_ack_peer_head(
            block,
            register_index,
            images.peer_head,
        );
        open_esp_radio_esp32s31_pac::zero_based_field_write::rx_block_ack_peer_tail_and_policy(
            block,
            register_index,
            peer_tail,
            interface,
            window as u8,
        );
        block
            .rx_block_ack_entry_start_sequence_load(register_index)
            .modify(|_, w| w.sequence().set(starting_sequence));
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().clear_bit());
        open_esp_radio_esp32s31_pac::zero_register_write::clear_rx_block_ack_bitmap_low_load(
            block,
            register_index,
        );
        open_esp_radio_esp32s31_pac::zero_register_write::clear_rx_block_ack_bitmap_high_load(
            block,
            register_index,
        );
        let update_bit = 1_u8 << hardware_index;
        // The checked index keeps the OR result inside the eight-bit field.
        // This preserves the blob's fresh-read OR operation.
        block.rx_block_ack_agreement_update().modify(|r, w| {
            w.ordinary_entry_update()
                .set(r.ordinary_entry_update().bits() | update_bit)
        });
        open_esp_radio_esp32s31_pac::zero_based_field_write::rx_block_ack_active_control(
            block,
            register_index,
            true,
            tid,
            true,
            true,
        );
    }

    /// Delete one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp.a[hal_ampdu.o]::
    /// hal_agreement_del_rx_ba`, size `0x72`.
    pub fn delete_rx_block_ack_entry(&mut self, hardware_index: u8) {
        assert!(hardware_index < 8);
        let block = &self.peripherals.wifi_mac_rx_dma;
        let register_index = rx_block_ack_register_index(hardware_index);
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().clear_bit());
        open_esp_radio_esp32s31_pac::zero_register_write::clear_rx_block_ack_bitmap_low_load(
            block,
            register_index,
        );
        open_esp_radio_esp32s31_pac::zero_register_write::clear_rx_block_ack_bitmap_high_load(
            block,
            register_index,
        );
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().set_bit());
        // The final full-word zero is a distinct observable edge in the blob.
        open_esp_radio_esp32s31_pac::zero_register_write::clear_rx_block_ack_entry_control(
            block,
            register_index,
        );
    }

    /// Sample both hardware-maintained and software-load words of one entry.
    pub fn rx_block_ack_entry_snapshot(
        &self,
        hardware_index: u8,
    ) -> Option<RxBlockAckEntrySnapshot> {
        if hardware_index >= 8 {
            return None;
        }
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
            interface: peer_tail_and_policy.interface().bits(),
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
        let (control_and_sequence, bitmap_low, bitmap_high, address_high) = match hardware_queue {
            0 => (
                block.tx_block_ack_control_sequence_q0().read().bits(),
                block.tx_block_ack_bitmap_low_q0().read().bits(),
                block.tx_block_ack_bitmap_high_q0().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q0()
                    .read()
                    .bits(),
            ),
            1 => (
                block.tx_block_ack_control_sequence_q1().read().bits(),
                block.tx_block_ack_bitmap_low_q1().read().bits(),
                block.tx_block_ack_bitmap_high_q1().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q1()
                    .read()
                    .bits(),
            ),
            2 => (
                block.tx_block_ack_control_sequence_q2().read().bits(),
                block.tx_block_ack_bitmap_low_q2().read().bits(),
                block.tx_block_ack_bitmap_high_q2().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q2()
                    .read()
                    .bits(),
            ),
            3 => (
                block.tx_block_ack_control_sequence_q3().read().bits(),
                block.tx_block_ack_bitmap_low_q3().read().bits(),
                block.tx_block_ack_bitmap_high_q3().read().bits(),
                block
                    .tx_block_ack_transmitter_address_high_q3()
                    .read()
                    .bits(),
            ),
            _ => return None,
        };
        Some(TxBlockAckRegisterImage {
            control_and_sequence,
            bitmap_low,
            bitmap_high,
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
    use super::{RxBlockAckImages, rx_block_ack_images};

    #[test]
    fn rx_block_ack_images_match_the_recovered_leaf() {
        assert_eq!(
            rx_block_ack_images(1, [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0], 6, 16),
            RxBlockAckImages {
                peer_head: 0xa8fb_1570,
                peer_tail_and_policy: 0x0041_f048,
                active_control: 0xc000_6001,
            }
        );
    }
}
