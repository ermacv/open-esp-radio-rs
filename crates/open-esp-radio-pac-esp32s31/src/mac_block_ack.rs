//! Generated-PAC ownership for the finite MAC BlockAck register leaves.

use super::{device_fence, RadioRegisters};

/// Raw three-word result sampled for one completed TX hardware queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckRegisterImage {
    pub control_and_sequence: u32,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RxBlockAckImages {
    peer_head: u32,
    peer_tail_and_policy: u32,
    active_control: u32,
}

const fn rx_block_ack_images(
    hardware_index: u8,
    interface: u8,
    peer: [u8; 6],
    tid: u8,
    window: u16,
) -> RxBlockAckImages {
    let peer_head = u32::from_le_bytes([peer[0], peer[1], peer[2], peer[3]]);
    let peer_tail = u16::from_le_bytes([peer[4], peer[5]]) as u32;
    let peer_tail_and_policy = peer_tail | ((interface as u32) << 16) | ((window as u32) << 18);
    let tid_image = tid.wrapping_shl(2) & 0x0f;
    let active_control = 0xc000_0001 | ((hardware_index as u32) << 5) | ((tid_image as u32) << 12);
    RxBlockAckImages {
        peer_head,
        peer_tail_and_policy,
        active_control,
    }
}

impl RadioRegisters {
    /// Program one receive BlockAck entry through the recovered finite leaf.
    ///
    /// SOURCE: pinned `libpp.a::hal_mac_set_rx_ba`; the SVD records each
    /// register and bit field with the corresponding migration transcription.
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
        let images = rx_block_ack_images(hardware_index, interface, peer, tid, window);
        let peer_tail = images.peer_tail_and_policy as u16;
        // The recovered caller passes its per-TID table-byte selector through
        // the leaf's `argument << 14` transform. Preserve that instruction
        // image rather than replacing it with the nominal TID value.
        let tid_image = ((images.active_control >> 12) & 0x0f) as u8;

        // SAFETY: validation proves the index fits the generated five-bit
        // field. This first RMW intentionally preserves every other bit.
        block
            .rx_block_ack_control()
            .modify(|_, w| unsafe { w.index().bits(hardware_index) });
        // SAFETY: the complete recovered leaf publishes these whole words.
        unsafe {
            block
                .rx_block_ack_peer_head()
                .write_with_zero(|w| w.bits(images.peer_head));
            block
                .rx_block_ack_peer_tail_and_policy()
                .write_with_zero(|w| {
                    w.peer_address_tail()
                        .bits(peer_tail)
                        .interface()
                        .bits(interface)
                        .window()
                        .bits(window as u8)
                });
            block
                .rx_block_ack_start_sequence()
                .write_with_zero(|w| w.sequence().bits(starting_sequence));
            block
                .rx_block_ack_bitmap_low()
                .write_with_zero(|w| w.bits(0));
            block
                .rx_block_ack_bitmap_high()
                .write_with_zero(|w| w.bits(0));
            block.rx_block_ack_control().write_with_zero(|w| {
                w.valid()
                    .set_bit()
                    .write()
                    .set_bit()
                    .tid()
                    .bits(tid_image)
                    .index()
                    .bits(hardware_index)
                    .enable()
                    .set_bit()
            });
        }

        let update = block.rx_block_ack_agreement_update();
        update.modify(|_, w| w.commit().set_bit());
        update.modify(|_, w| w.commit().clear_bit());
        update.modify(|_, w| w.readback_latch().set_bit());
        device_fence();
        let _ = block.rx_block_ack_peer_tail_and_policy().read().bits();
        let _ = block.rx_block_ack_peer_head().read().bits();
        let _ = block.rx_block_ack_start_sequence().read().bits();
        let _ = block.rx_block_ack_control().read().bits();
        device_fence();
        update.modify(|_, w| w.readback_latch().clear_bit());
    }

    /// Remove one receive BlockAck entry through the recovered finite leaf.
    pub fn clear_rx_block_ack_entry(&mut self, hardware_index: u8) {
        assert!(hardware_index < 8);
        let block = &self.peripherals.wifi_mac_rx_dma;
        // SAFETY: validation proves the index fits the generated field.
        block
            .rx_block_ack_control()
            .modify(|_, w| unsafe { w.index().bits(hardware_index) });
        let update = block.rx_block_ack_agreement_update();
        update.modify(|_, w| w.readback_latch().set_bit());
        device_fence();
        let _ = block.rx_block_ack_control().read().bits();
        device_fence();
        update.modify(|_, w| w.readback_latch().clear_bit());
        block
            .rx_block_ack_control()
            .modify(|_, w| w.valid().clear_bit());
        // SAFETY: the complete recovered clear leaf publishes zero to both
        // bitmap words.
        unsafe {
            block
                .rx_block_ack_bitmap_low()
                .write_with_zero(|w| w.bits(0));
            block
                .rx_block_ack_bitmap_high()
                .write_with_zero(|w| w.bits(0));
        }
        update.modify(|_, w| w.commit().set_bit());
        update.modify(|_, w| w.commit().clear_bit());
    }

    /// Sample the three completed TX BlockAck words for one hardware queue.
    ///
    /// Reads remain in control/sequence, low bitmap, high bitmap order.
    pub fn read_tx_block_ack_registers(
        &self,
        hardware_queue: u8,
    ) -> Option<TxBlockAckRegisterImage> {
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
        Some(TxBlockAckRegisterImage {
            control_and_sequence,
            bitmap_low,
            bitmap_high,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{rx_block_ack_images, RxBlockAckImages};

    #[test]
    fn rx_block_ack_images_match_the_recovered_leaf() {
        assert_eq!(
            rx_block_ack_images(3, 1, [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0], 6, 16),
            RxBlockAckImages {
                peer_head: 0xa8fb_1570,
                peer_tail_and_policy: 0x0041_f048,
                active_control: 0xc000_8061,
            }
        );
    }
}
