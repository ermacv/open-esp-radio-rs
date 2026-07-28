//! Generated-PAC ownership for ordinary EDCA TX queue transactions.

use super::{device_fence, RadioRegisters};

const ORDINARY_QUEUE_COUNT: u8 = 4;
const ENABLE_VALID_MASK: u32 = 0xc000_0000;
const ENABLE_MASK: u32 = 0x8000_0000;
const VALID_MASK: u32 = 0x4000_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxProgram {
    pub plcp0: u32,
    pub plcp1: u32,
    pub power: u32,
    pub length_control: u32,
    pub timeout: u16,
    pub priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxCompletionRegisters {
    pub aux_a: u32,
    pub aux_b: u32,
    pub aux_c: u32,
    pub primary: u32,
    pub alternate: u32,
    pub trigger_flow: bool,
}

const fn physical_bank(queue: u8) -> usize {
    (ORDINARY_QUEUE_COUNT - 1 - queue) as usize
}

impl RadioRegisters {
    /// Program one ordinary queue up to, but excluding, its ENABLE|VALID edge.
    ///
    /// Keeping the final edge separate lets the MAC publish its software
    /// ownership state before hardware can complete the queue.
    pub fn prepare_legacy_mac_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        assert!(program.timeout <= 0x0fff);
        assert!(program.priority <= 0x0f);
        assert!(program.priority_count <= 0x0fff);
        assert!(program.aifsn <= 0x0f);
        assert!(program.contention_window <= 0x03ff);
        assert!(program.interface <= 3);

        let bank = physical_bank(queue);
        let control_bank = &self.peripherals.wifi_mac_tx_queue_control;
        let control = control_bank.control(bank);
        if control.read().bits() & ENABLE_VALID_MASK != 0 {
            return false;
        }

        // SOURCE: complete hal_mac_tx_config_timeout. This precedes every
        // vector write in the recovered lmacSetTxFrame parent.
        control_bank
            .config(bank)
            .modify(|_, w| unsafe { w.timeout().bits(program.timeout) });

        // SAFETY: the recovered formatter publishes each complete image.
        unsafe {
            control.write_with_zero(|w| w.bits(program.plcp0));
            self.peripherals
                .wifi_mac_tx_queue_vector
                .plcp1(bank)
                .write_with_zero(|w| w.bits(program.plcp1));
        }
        control_bank
            .ppdu_control(bank)
            .modify(|_, w| w.legacy_clear_unknown().clear_bit());
        control_bank
            .protection(bank)
            .modify(|_, w| w.protection_high_unknown().clear_bit());
        // SAFETY: the complete hal_mac_tx_set_ppdu leaf stores whole words.
        unsafe {
            self.peripherals
                .wifi_mac_tx_queue_vector
                .length_control(bank)
                .write_with_zero(|w| w.bits(program.length_control));
            self.peripherals
                .wifi_mac_tx_queue_vector
                .power(bank)
                .write_with_zero(|w| w.bits(program.power));
        }

        // SOURCE: complete mac_tx_set_pti. Each field is intentionally a
        // separate fresh-read RMW and must not be coalesced.
        control_bank
            .config(bank)
            .modify(|_, w| unsafe { w.priority().bits(program.priority) });
        let pti = self.peripherals.wifi_mac_tx_queue_vector.pti(bank);
        pti.modify(|_, w| unsafe { w.pti_2().bits(program.priority) });
        pti.modify(|_, w| unsafe { w.pti_1().bits(program.priority) });
        pti.modify(|_, w| unsafe { w.pti_0().bits(program.priority) });
        pti.modify(|_, w| unsafe { w.pti_3().bits(program.priority) });
        pti.modify(|_, w| unsafe { w.count().bits(program.priority_count) });

        // SOURCE: complete hal_mac_tx_config_edca. Preserve three distinct
        // hardware edges in the recovered order.
        control_bank
            .config(bank)
            .modify(|_, w| unsafe { w.aifsn().bits(program.aifsn) });
        control_bank
            .config(bank)
            .modify(|_, w| unsafe { w.contention_window().bits(program.contention_window) });
        control_bank
            .config(bank)
            .modify(|_, w| unsafe { w.interface().bits(program.interface) });
        true
    }

    pub fn start_legacy_mac_tx(&mut self, queue: u8, plcp0: u32) {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        device_fence();
        // SAFETY: prepare_legacy_mac_tx validated the selected queue and the
        // recovered enable leaf publishes this complete ownership image.
        unsafe {
            self.peripherals
                .wifi_mac_tx_queue_control
                .control(physical_bank(queue))
                .write_with_zero(|w| w.bits(plcp0 | ENABLE_VALID_MASK));
        }
        device_fence();
    }

    pub fn take_mac_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let completion_mask = 1_u32 << queue;
        let common = &self.peripherals.wifi_mac_tx_common;
        if common.complete_state().read().bits() & completion_mask == 0 {
            return None;
        }

        let bank = physical_bank(queue);
        let completion = &self.peripherals.wifi_mac_tx_completion;
        let aux_a = completion.aux_a(bank).read().bits();
        let aux_b = completion.aux_b(bank).read().bits();
        let aux_c = completion.aux_c(bank).read().bits();
        let primary = completion.primary(bank).read().bits();
        let alternate = completion.alternate(bank).read().bits();
        let trigger_flow = common.queue_state().read().bits() & (1_u32 << (24 + queue)) != 0;
        let clear = common.complete_clear().read().bits();
        // SAFETY: the complete recovery writes the preserved register image
        // with only this bounded ordinary-queue completion bit asserted.
        unsafe {
            common
                .complete_clear()
                .write_with_zero(|w| w.bits(clear | completion_mask));
        }
        device_fence();
        Some(MacTxCompletionRegisters {
            aux_a,
            aux_b,
            aux_c,
            primary,
            alternate,
            trigger_flow,
        })
    }

    pub fn begin_mac_tx_timeout_abort(&mut self, queue: u8) -> bool {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let timeout_mask = 1_u32 << (16 + queue);
        let common = &self.peripherals.wifi_mac_tx_common;
        if common.queue_state().read().bits() & timeout_mask == 0 {
            return false;
        }
        // SAFETY: value three is the complete hal_mac_tx_set_cca disable
        // encoding recovered for the bounded timeout transaction.
        common
            .cca_control()
            .modify(|_, w| unsafe { w.force().bits(3) });
        device_fence();
        true
    }

    pub fn finish_mac_tx_timeout_abort(&mut self, queue: u8) -> Option<bool> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let timeout_mask = 1_u32 << (16 + queue);
        let common = &self.peripherals.wifi_mac_tx_common;
        if common.queue_state().read().bits() & timeout_mask == 0 {
            return None;
        }

        let control = self
            .peripherals
            .wifi_mac_tx_queue_control
            .control(physical_bank(queue));
        let control_image = control.read().bits();
        let was_valid = control_image & VALID_MASK != 0;
        // SAFETY: exact recovered full-image invalidate edge.
        unsafe { control.write_with_zero(|w| w.bits(control_image & !VALID_MASK)) };
        let cca_image = common.cca_control().read().bits();
        // SAFETY: preserve the fresh image and clear only the generated field.
        unsafe {
            common
                .cca_control()
                .write_with_zero(|w| w.bits(cca_image & 0x3fff_ffff));
        }
        if was_valid {
            let invalid_image = control.read().bits();
            // SAFETY: exact recovered conditional disable edge.
            unsafe { control.write_with_zero(|w| w.bits(invalid_image & !ENABLE_MASK)) };
        }
        // SAFETY: bounded queue maps to one instruction-proven W1C timeout bit.
        unsafe {
            common
                .queue_state_clear()
                .write_with_zero(|w| w.bits(timeout_mask));
        }
        device_fence();
        Some(control.read().bits() & ENABLE_VALID_MASK == 0)
    }

    pub fn detach_completed_mac_tx(&mut self, queue: u8) -> bool {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let control = self
            .peripherals
            .wifi_mac_tx_queue_control
            .control(physical_bank(queue));
        let image = control.read().bits();
        // SAFETY: exact recovered full-image queue close edge.
        unsafe { control.write_with_zero(|w| w.bits(image & !ENABLE_VALID_MASK)) };
        device_fence();
        control.read().bits() & ENABLE_VALID_MASK == 0
    }
}
