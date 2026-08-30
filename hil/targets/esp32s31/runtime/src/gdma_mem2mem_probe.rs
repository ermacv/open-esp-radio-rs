//! Diagnostic PSRAM-to-SRAM AXI-GDMA promotion benchmark.
//!
//! This executes once before radio initialization. It uses the same PSRAM
//! mapping and linker profile as the Wi-Fi workload. The benchmark consumes
//! its dedicated DMA channel before radio initialization, so no DMA state
//! participates in the measured datapath.

use core::{
    arch::asm,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use esp_hal::peripherals::DMA_AXI_CH0;
use open_esp_radio_esp32s31_platform_pac::{AxiGdmaDescriptor, AxiGdmaMem2Mem, BurstSize};

const FRAME_SIZE: usize = 1536;
const FRAMES_PER_BATCH: usize = 32;
const BATCH_SIZE: usize = FRAME_SIZE * FRAMES_PER_BATCH;
const ITERATIONS: usize = 64;
const CACHE_LINE: usize = 64;
const DESCRIPTOR_COUNT: usize = 16;

#[repr(C, align(64))]
struct Batch([u8; BATCH_SIZE]);

#[repr(C, align(64))]
struct DescriptorBatch([AxiGdmaDescriptor; DESCRIPTOR_COUNT]);

#[repr(C, align(64))]
struct CacheLine([u8; CACHE_LINE]);

#[unsafe(link_section = ".psram.bss.gdma_mem2mem_probe.source")]
static mut SOURCE: Batch = Batch([0; BATCH_SIZE]);

#[unsafe(link_section = ".psram.bss.gdma_mem2mem_probe.next")]
static mut NEXT_SOURCE: Batch = Batch([0; BATCH_SIZE]);

#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.destination")]
static mut DESTINATION: Batch = Batch([0; BATCH_SIZE]);

#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.sram_source")]
static mut SRAM_SOURCE: CacheLine = CacheLine([0; CACHE_LINE]);

// The runtime's ordinary `.bss` deliberately lives in PSRAM. DMA descriptors
// are control structures and must remain in uncached internal SRAM even though
// AXI-GDMA can access the PSRAM payload itself.
#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.rx_descriptors")]
static mut RX_DESCRIPTORS: DescriptorBatch =
    DescriptorBatch([AxiGdmaDescriptor::EMPTY; DESCRIPTOR_COUNT]);

#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.tx_descriptors")]
static mut TX_DESCRIPTORS: DescriptorBatch =
    DescriptorBatch([AxiGdmaDescriptor::EMPTY; DESCRIPTOR_COUNT]);

// Kept in uncached SRAM so JTAG can recover the exact failing frontier even
// when the diagnostic runs before the asynchronous USB logger makes progress.
#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.status")]
#[unsafe(no_mangle)]
#[used]
pub static OPEN_RADIO_GDMA_PROBE_STATUS: AtomicU32 = AtomicU32::new(0);

#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.detail")]
#[unsafe(no_mangle)]
#[used]
pub static OPEN_RADIO_GDMA_PROBE_DETAIL: AtomicU32 = AtomicU32::new(0);

// Total cycles/instructions for 64 iterations of, respectively: CPU copy,
// blocking GDMA, CPU copy + next-batch preparation, serial GDMA + preparation,
// overlapped GDMA + preparation, and interrupt-driven GDMA.
#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.cycles")]
#[unsafe(no_mangle)]
#[used]
pub static OPEN_RADIO_GDMA_PROBE_CYCLES: [AtomicU32; 6] = [const { AtomicU32::new(0) }; 6];

#[unsafe(link_section = ".dma.bss.gdma_mem2mem_probe.instructions")]
#[unsafe(no_mangle)]
#[used]
pub static OPEN_RADIO_GDMA_PROBE_INSTRUCTIONS: [AtomicU32; 6] = [const { AtomicU32::new(0) }; 6];

fn mark(status: u32, detail: u32) {
    OPEN_RADIO_GDMA_PROBE_DETAIL.store(detail, Ordering::Relaxed);
    OPEN_RADIO_GDMA_PROBE_STATUS.store(status, Ordering::Release);
}

#[inline(always)]
fn cycles() -> u32 {
    let value: u32;
    unsafe { asm!("rdcycle {value}", value = out(reg) value, options(nomem, nostack)) };
    value
}

#[inline(always)]
fn instructions() -> u32 {
    let value: u32;
    unsafe { asm!("rdinstret {value}", value = out(reg) value, options(nomem, nostack)) };
    value
}

#[inline(never)]
fn dirty_batch(batch: &mut [u8], salt: u8) {
    for (line, bytes) in batch.chunks_exact_mut(CACHE_LINE).enumerate() {
        bytes[0] = salt.wrapping_add(line as u8);
    }
}

#[inline(never)]
fn prepare_next_batch(batch: &mut [u8], salt: u8) -> u32 {
    let mut checksum = 0u32;
    for (index, byte) in batch.iter_mut().enumerate() {
        *byte = (index as u8)
            .wrapping_mul(29)
            .wrapping_add(salt.rotate_left((index & 7) as u32));
        checksum = checksum.rotate_left(5) ^ u32::from(*byte);
    }
    core::hint::black_box(checksum)
}

#[inline(never)]
fn fill_pattern(batch: &mut [u8], salt: u8) {
    for (index, byte) in batch.iter_mut().enumerate() {
        *byte = (index as u8)
            .wrapping_mul(37)
            .wrapping_add(salt.rotate_left((index & 7) as u32));
    }
}

#[inline(never)]
fn fill_volatile(batch: &mut [u8], value: u8) {
    for byte in batch {
        unsafe { ptr::write_volatile(byte, value) };
    }
}

fn first_mismatch(left: &[u8], right: &[u8]) -> Option<(usize, u8, u8)> {
    left.iter()
        .zip(right)
        .enumerate()
        .find_map(|(index, (&left, &right))| (left != right).then_some((index, left, right)))
}

fn report(slot: usize, label: &str, elapsed_cycles: u32, retired_instructions: u32) {
    OPEN_RADIO_GDMA_PROBE_CYCLES[slot].store(elapsed_cycles, Ordering::Release);
    OPEN_RADIO_GDMA_PROBE_INSTRUCTIONS[slot].store(retired_instructions, Ordering::Release);
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem mode={} cycles={} instructions={} cycles_batch={} cycles_frame={} bytes={}",
        label,
        elapsed_cycles,
        retired_instructions,
        elapsed_cycles / ITERATIONS as u32,
        elapsed_cycles / (ITERATIONS * FRAMES_PER_BATCH) as u32,
        BATCH_SIZE * ITERATIONS,
    );
}

pub(crate) async fn run(channel: DMA_AXI_CH0<'static>) {
    mark(0x0001, 0);
    let source = unsafe { &mut *ptr::addr_of_mut!(SOURCE) };
    let next_source = unsafe { &mut *ptr::addr_of_mut!(NEXT_SOURCE) };
    let destination = unsafe { &mut *ptr::addr_of_mut!(DESTINATION) };
    let sram_source = unsafe { &mut *ptr::addr_of_mut!(SRAM_SOURCE) };
    let rx_descriptors = unsafe { &mut *ptr::addr_of_mut!(RX_DESCRIPTORS) };
    let tx_descriptors = unsafe { &mut *ptr::addr_of_mut!(TX_DESCRIPTORS) };

    assert!((0x5000_0000..0x5400_0000).contains(&(source.0.as_ptr() as usize)));
    assert!((0x5000_0000..0x5400_0000).contains(&(next_source.0.as_ptr() as usize)));
    assert!((0x2f00_0000..0x2f08_0000).contains(&(destination.0.as_ptr() as usize)));

    for (index, byte) in source.0.iter_mut().enumerate() {
        *byte = index as u8;
    }
    for (index, byte) in next_source.0.iter_mut().enumerate() {
        *byte = !(index as u8);
    }

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0, iteration as u8);
        destination.0.copy_from_slice(&source.0);
        core::hint::black_box(&destination.0[..]);
    }
    report(
        0,
        "cpu-copy-dirty",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let mut gdma = AxiGdmaMem2Mem::new(channel);
    mark(0x0010, 0);

    fill_pattern(&mut sram_source.0, 0x71);
    fill_volatile(&mut destination.0[..CACHE_LINE], 0xa5);
    mark(0x0011, CACHE_LINE as u32);
    let transfer = match gdma.prepare(
        &mut destination.0[..CACHE_LINE],
        &mut sram_source.0,
        &mut rx_descriptors.0,
        &mut tx_descriptors.0,
        BurstSize::Bytes32,
    ) {
        Ok(transfer) => transfer,
        Err(error) => {
            mark(0xe010, CACHE_LINE as u32);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem sram_control=FAIL stage=prepare error={:?}",
                error,
            );
            return;
        }
    };
    let sram_report = match transfer.start().wait_blocking(100_000_000) {
        Ok(report) => report,
        Err(error) => {
            mark(0xe020, CACHE_LINE as u32);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem sram_control=FAIL stage=wait error={:?}",
                error,
            );
            return;
        }
    };
    if let Some((offset, actual, expected)) =
        first_mismatch(&destination.0[..CACHE_LINE], &sram_source.0)
    {
        let detail =
            ((offset as u32 & 0xffff) << 16) | (u32::from(actual) << 8) | u32::from(expected);
        mark(0xe030, detail);
        log::error!(
            "OPEN_RADIO_HIL gdma_mem2mem sram_control=FAIL stage=compare offset={} actual={} expected={}",
            offset,
            actual,
            expected,
        );
        return;
    }
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem sram_control=PASS rx_raw={:08x} tx_raw={:08x}",
        sram_report.rx_raw,
        sram_report.tx_raw,
    );

    for (case, size) in [64usize, 1536, 4032, 4096, BATCH_SIZE]
        .into_iter()
        .enumerate()
    {
        fill_pattern(&mut source.0[..size], 0x31u8.wrapping_add(case as u8));
        fill_volatile(&mut destination.0[..size], 0xa5);
        mark(0x0100 + case as u32, size as u32);
        let transfer = match gdma.prepare(
            &mut destination.0[..size],
            &mut source.0[..size],
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        ) {
            Ok(transfer) => transfer,
            Err(error) => {
                mark(0xe100 + case as u32, size as u32);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem correctness=FAIL stage=prepare size={} error={:?}",
                    size,
                    error,
                );
                return;
            }
        };
        mark(0x0200 + case as u32, size as u32);
        let report = match transfer.start().wait_blocking(100_000_000) {
            Ok(report) => report,
            Err(error) => {
                mark(0xe200 + case as u32, size as u32);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem correctness=FAIL stage=wait size={} error={:?}",
                    size,
                    error,
                );
                return;
            }
        };
        mark(0x0300 + case as u32, size as u32);
        if let Some((offset, actual, expected)) =
            first_mismatch(&destination.0[..size], &source.0[..size])
        {
            let detail =
                ((offset as u32 & 0xffff) << 16) | (u32::from(actual) << 8) | u32::from(expected);
            mark(0xe300 + case as u32, detail);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem correctness=FAIL stage=compare size={} offset={} actual={} expected={}",
                size,
                offset,
                actual,
                expected,
            );
            return;
        }
        log::info!(
            "OPEN_RADIO_HIL gdma_mem2mem correctness=PASS size={} descriptors={} rx_raw={:08x} tx_raw={:08x}",
            size,
            report.descriptors,
            report.rx_raw,
            report.tx_raw,
        );
    }

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0, iteration as u8);
        gdma.prepare(
            &mut destination.0,
            &mut source.0,
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        )
        .unwrap()
        .start()
        .wait_blocking(100_000_000)
        .unwrap();
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        1,
        "gdma-blocking-dirty",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let mut checksum = 0u32;
    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0, iteration as u8);
        destination.0.copy_from_slice(&source.0);
        checksum ^= prepare_next_batch(&mut next_source.0, iteration as u8);
        core::hint::black_box(&destination.0[..]);
    }
    report(
        2,
        "cpu-copy-next-prep",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0, iteration as u8);
        gdma.prepare(
            &mut destination.0,
            &mut source.0,
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        )
        .unwrap()
        .start()
        .wait_blocking(100_000_000)
        .unwrap();
        checksum ^= prepare_next_batch(&mut next_source.0, iteration as u8);
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        3,
        "gdma-serial-next-prep",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0, iteration as u8);
        let transfer = gdma
            .prepare(
                &mut destination.0,
                &mut source.0,
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            )
            .unwrap()
            .start();
        checksum ^= prepare_next_batch(&mut next_source.0, iteration as u8);
        transfer.wait_blocking(100_000_000).unwrap();
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        4,
        "gdma-overlap-next-prep",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0, iteration as u8);
        gdma.prepare(
            &mut destination.0,
            &mut source.0,
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        )
        .unwrap()
        .start()
        .await
        .unwrap();
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        5,
        "gdma-async-dirty",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    assert_eq!(&destination.0[..], &source.0[..]);
    mark(0x600d, BATCH_SIZE as u32);
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem result=PASS checksum={}",
        checksum
    );
}
