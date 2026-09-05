//! Typed upstream AXI-GDMA register operations and DMA visibility fences.

use super::{descriptor::BurstSize, transfer::AxiGdmaMem2Mem};
use core::{
    arch::asm,
    sync::atomic::{Ordering, compiler_fence},
};
use esp_hal::peripherals::{AXI_GDMA, HP_SYS_CLKRST};

pub(super) const INTERNAL_SRAM_START: usize = 0x2f00_0000;

pub(super) const INTERNAL_SRAM_END: usize = 0x2f08_0000;

pub(super) const PSRAM_START: usize = 0x5000_0000;

pub(super) const PSRAM_END: usize = 0x5400_0000;

const CHANNEL: usize = 0;

const M2M_TRIGGER_ID: u8 = 6;

const TX_DONE: u32 = 1 << 0;

const TX_EOF: u32 = 1 << 1;

const TX_DESCRIPTOR_ERROR: u32 = 1 << 2;

const TX_TOTAL_EOF: u32 = 1 << 3;

const TX_FIFO_OVERFLOW: u32 = 1 << 4;

const TX_FIFO_UNDERFLOW: u32 = 1 << 5;

const TX_ALL: u32 =
    TX_DONE | TX_EOF | TX_DESCRIPTOR_ERROR | TX_TOTAL_EOF | TX_FIFO_OVERFLOW | TX_FIFO_UNDERFLOW;

pub(super) const TX_ERRORS: u32 = TX_DESCRIPTOR_ERROR | TX_FIFO_OVERFLOW | TX_FIFO_UNDERFLOW;

const RX_DONE: u32 = 1 << 0;

const RX_SUCCESS_EOF: u32 = 1 << 1;

const RX_ERROR_EOF: u32 = 1 << 2;

const RX_DESCRIPTOR_ERROR: u32 = 1 << 3;

const RX_DESCRIPTOR_EMPTY: u32 = 1 << 4;

const RX_FIFO_OVERFLOW: u32 = 1 << 5;

const RX_FIFO_UNDERFLOW: u32 = 1 << 6;

const RX_ALL: u32 = RX_DONE
    | RX_SUCCESS_EOF
    | RX_ERROR_EOF
    | RX_DESCRIPTOR_ERROR
    | RX_DESCRIPTOR_EMPTY
    | RX_FIFO_OVERFLOW
    | RX_FIFO_UNDERFLOW;

pub(super) const RX_ERRORS: u32 =
    RX_ERROR_EOF | RX_DESCRIPTOR_ERROR | RX_DESCRIPTOR_EMPTY | RX_FIFO_OVERFLOW | RX_FIFO_UNDERFLOW;

pub(super) fn terminal_status() -> Option<(u32, u32)> {
    let regs = AXI_GDMA::regs();
    let rx_raw = regs.in_ch(CHANNEL).in_int().raw().read().bits();
    let tx_raw = regs.out_ch(CHANNEL).out_int().raw().read().bits();
    let failed = rx_raw & RX_ERRORS != 0 || tx_raw & TX_ERRORS != 0;
    let completed = rx_raw & RX_SUCCESS_EOF != 0 && tx_raw & TX_TOTAL_EOF != 0;
    (failed || completed).then_some((rx_raw, tx_raw))
}

pub(super) fn enable_channel_interrupts() {
    let regs = AXI_GDMA::regs();
    unsafe {
        regs.in_ch(CHANNEL)
            .in_int()
            .ena()
            .write_with_zero(|writer| writer.bits(RX_SUCCESS_EOF | RX_ERRORS));
        regs.out_ch(CHANNEL)
            .out_int()
            .ena()
            .write_with_zero(|writer| writer.bits(TX_TOTAL_EOF | TX_ERRORS));
    }
}

pub(super) fn disable_channel_interrupts() {
    let regs = AXI_GDMA::regs();
    unsafe {
        regs.in_ch(CHANNEL)
            .in_int()
            .ena()
            .write_with_zero(|writer| writer.bits(0));
        regs.out_ch(CHANNEL)
            .out_int()
            .ena()
            .write_with_zero(|writer| writer.bits(0));
    }
}

pub(super) fn enable_and_configure_group() {
    let clock = HP_SYS_CLKRST::regs().axi_pdma_ctrl0();
    clock.modify(|_, writer| {
        writer
            .axi_pdma_sys_clk_en()
            .set_bit()
            .axi_pdma_rst_en()
            .set_bit()
    });
    clock.modify(|_, writer| writer.axi_pdma_rst_en().clear_bit());

    let regs = AXI_GDMA::regs();
    // The channel state machines can consume and write back descriptors
    // while the shared AXI read/write masters remain stale. Reset both
    // layers before publishing any memory window or channel state. This
    // mirrors IDF's group reset and esp-hal's AXI-master initialization.
    regs.misc_conf().modify(|_, writer| {
        writer
            .clk_en()
            .set_bit()
            .axim_rst_rd_inter()
            .set_bit()
            .axim_rst_wr_inter()
            .set_bit()
    });
    regs.misc_conf().modify(|_, writer| {
        writer
            .axim_rst_rd_inter()
            .clear_bit()
            .axim_rst_wr_inter()
            .clear_bit()
    });
    regs.intr_mem_start_addr().write(|writer| unsafe {
        writer
            .access_intr_mem_start_addr()
            .bits(INTERNAL_SRAM_START as u32)
    });
    regs.intr_mem_end_addr().write(|writer| unsafe {
        writer
            .access_intr_mem_end_addr()
            .bits((INTERNAL_SRAM_END - 1) as u32)
    });
    regs.extr_mem_start_addr()
        .write(|writer| unsafe { writer.access_extr_mem_start_addr().bits(0x4000_0000) });
    regs.extr_mem_end_addr()
        .write(|writer| unsafe { writer.access_extr_mem_end_addr().bits(PSRAM_END as u32 - 1) });
}

pub(super) fn dma_fence() {
    compiler_fence(Ordering::SeqCst);
    unsafe { asm!("fence rw, rw", options(nostack)) };
    compiler_fence(Ordering::SeqCst);
}

impl<'d> AxiGdmaMem2Mem<'d> {
    pub(super) fn configure_channel(&mut self, rx_head: u32, tx_head: u32, burst: BurstSize) {
        self.stop_and_reset_channel();
        disable_channel_interrupts();
        let regs = AXI_GDMA::regs();
        let input = regs.in_ch(CHANNEL);
        let output = regs.out_ch(CHANNEL);

        input
            .in_int()
            .clr()
            .write(|writer| unsafe { writer.bits(RX_ALL) });
        output
            .out_int()
            .clr()
            .write(|writer| unsafe { writer.bits(TX_ALL) });

        input.in_conf0().modify(|_, writer| unsafe {
            writer
                .mem_trans_en()
                .set_bit()
                .indscr_burst_en()
                .set_bit()
                .in_burst_size_sel()
                .bits(burst.register_value())
        });
        input
            .in_conf1()
            .modify(|_, writer| writer.in_check_owner().set_bit());
        input
            .in_peri_sel()
            .write(|writer| unsafe { writer.peri_in_sel().bits(M2M_TRIGGER_ID) });
        input
            .in_link2()
            .write(|writer| unsafe { writer.inlink_addr().bits(rx_head) });

        output.out_conf0().modify(|_, writer| unsafe {
            writer
                .out_auto_wrback()
                .set_bit()
                .out_eof_mode()
                .set_bit()
                .outdscr_burst_en()
                .set_bit()
                .out_burst_size_sel()
                .bits(burst.register_value())
        });
        output
            .out_conf1()
            .modify(|_, writer| writer.out_check_owner().set_bit());
        output
            .out_peri_sel()
            .write(|writer| unsafe { writer.peri_out_sel().bits(M2M_TRIGGER_ID) });
        output
            .out_link2()
            .write(|writer| unsafe { writer.outlink_addr().bits(tx_head) });
    }
}

impl<'d> AxiGdmaMem2Mem<'d> {
    pub(super) fn start(&mut self) {
        dma_fence();
        let regs = AXI_GDMA::regs();
        regs.in_ch(CHANNEL)
            .in_link1()
            .modify(|_, writer| writer.inlink_start().set_bit());
        regs.out_ch(CHANNEL)
            .out_link1()
            .modify(|_, writer| writer.outlink_start().set_bit());
    }
}

impl<'d> AxiGdmaMem2Mem<'d> {
    pub(super) fn stop_and_reset_channel(&mut self) {
        let regs = AXI_GDMA::regs();
        let input = regs.in_ch(CHANNEL);
        let output = regs.out_ch(CHANNEL);

        input
            .in_link1()
            .modify(|_, writer| writer.inlink_stop().set_bit());
        output
            .out_link1()
            .modify(|_, writer| writer.outlink_stop().set_bit());
        input
            .in_conf0()
            .modify(|_, writer| writer.in_rst().set_bit());
        input
            .in_conf0()
            .modify(|_, writer| writer.in_rst().clear_bit());
        output
            .out_conf0()
            .modify(|_, writer| writer.out_rst().set_bit());
        output
            .out_conf0()
            .modify(|_, writer| writer.out_rst().clear_bit());
    }
}
