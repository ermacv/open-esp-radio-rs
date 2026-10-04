/// Each hart's interrupt-stack bound of the ESP32-S31 runtime ELF at `elf`,
/// as the image build's gate computes it.
pub fn interrupt_stacks(
    root: &std::path::Path,
    elf: &std::path::Path,
) -> crate::Result<oer_esp32s31_firmware::interrupt_stack::InterruptStacks> {
    oer_esp32s31_firmware::interrupt_stack::interrupt_stacks(root, elf)
}
