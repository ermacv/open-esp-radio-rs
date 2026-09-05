#![no_std]
#![forbid(unsafe_code)]
use open_esp_radio_esp32s31_platform_pac::{AxiGdmaDescriptor, AxiGdmaMem2Mem, BurstSize};
pub fn forget_and_reuse(
    driver: &mut AxiGdmaMem2Mem<'_>,
    destination: &mut [u8],
    source: &mut [u8],
    rx: &mut [AxiGdmaDescriptor],
    tx: &mut [AxiGdmaDescriptor],
) {
    if let Ok(prepared) = driver.prepare(destination, source, rx, tx, BurstSize::Bytes16) {
        core::mem::forget(prepared.start());
        destination.fill(0);
        source.fill(0);
        rx.fill(AxiGdmaDescriptor::EMPTY);
        tx.fill(AxiGdmaDescriptor::EMPTY);
    }
}
