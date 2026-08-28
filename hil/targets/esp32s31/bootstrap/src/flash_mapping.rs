//! Inspection of the Flash XIP mapping inherited from the bootloader.

pub const XIP_START: usize = 0x4000_0000;
pub const XIP_END: usize = 0x4400_0000;

/// Translates a currently mapped Flash XIP address to its physical address.
///
/// # Safety
///
/// The SPI0 MMU selector is shared hardware state. This may only run during
/// single-core bootstrap, before interrupts or the other core can inspect or
/// change the selector concurrently.
pub unsafe fn physical_address(virtual_address: usize) -> Option<u32> {
    let relative = virtual_address.checked_sub(XIP_START)?;
    if virtual_address >= XIP_END {
        return None;
    }

    let registers = esp_hal::peripherals::SPI0::regs();
    let page_code = registers.mmu_power_ctrl().read().mmu_page_size().bits();
    let page_size = 0x4_0000usize.checked_shr(u32::from(page_code))?;
    if page_size == 0 {
        return None;
    }
    let entry = u32::try_from(relative / page_size).ok()?;
    let within_page = u32::try_from(relative % page_size).ok()?;

    registers
        .mmu_item_index()
        .write(|writer| unsafe { writer.mmu_item_index().bits(entry) });
    let physical_page = u32::from(registers.mmu_item_content().read().paddr().bits())
        .checked_mul(u32::try_from(page_size).ok()?)?;
    physical_page.checked_add(within_page)
}
