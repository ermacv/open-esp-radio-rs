//! Restricted controller memory-list pointer publication.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

const CONTROLLER_SRAM_ADDRESS_MASK: u32 = 0xffc0_0003;
const CONTROLLER_SRAM_ADDRESS_PREFIX: u32 = 0x2f00_0000;

/// One of the three positional hardware list selectors proven by the
/// selector leaves in the ESP32-S31 controller archive.
///
/// No RX, TX, free-list, or ready-list meaning is assigned yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListSelector {
    /// Vendor selector value one.
    One,
    /// Vendor selector value two.
    Two,
    /// Vendor selector value three.
    Three,
}

/// One of the two positional pointer words associated with each selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListSlot {
    /// First word in the selected pair.
    A,
    /// Second word in the selected pair.
    B,
}

/// Address accepted by the controller's reviewed compressed-pointer format.
///
/// The type proves only the instruction-level encoding contract: four-byte
/// alignment and the fixed `0x2f` SRAM prefix reconstructed by controller
/// software. It does not prove allocation, lifetime, list contents, or which
/// protocol role owns the pointed-to memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerSramAddress(u32);

/// Why a raw address cannot be represented by the reviewed controller format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerSramAddressError {
    /// The low two address bits are not zero.
    Unaligned,
    /// The fixed address positions do not match the observed SRAM window.
    OutsideEncodableWindow,
}

impl BluetoothControllerSramAddress {
    /// Validate one raw address against the exact compressed-pointer domain.
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        if address & 0x3 != 0 {
            return Err(BluetoothControllerSramAddressError::Unaligned);
        }
        if address & CONTROLLER_SRAM_ADDRESS_MASK != CONTROLLER_SRAM_ADDRESS_PREFIX {
            return Err(BluetoothControllerSramAddressError::OutsideEncodableWindow);
        }
        Ok(Self(address))
    }

    /// Return the validated CPU address without granting dereference access.
    pub const fn address(self) -> u32 {
        self.0
    }

    const fn compressed(self) -> u32 {
        (self.0 >> 2) & 0x000f_ffff
    }
}

/// Exact low-twenty-bit image accepted by the recovered selector leaves.
///
/// `Zero` is intentionally not named empty, null, or disabled: one recovered
/// caller supplies literal zero for slot B, but its hardware meaning is not
/// yet established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListPointerImage {
    /// Publish the finite low-twenty-bit zero image.
    Zero,
    /// Publish one validated compressed SRAM address.
    Address(BluetoothControllerSramAddress),
}

impl BluetoothMemoryListPointerImage {
    const fn compressed(self) -> u32 {
        match self {
            Self::Zero => 0,
            Self::Address(address) => address.compressed(),
        }
    }
}

impl BluetoothTaskRegisters {
    /// Program one positional controller memory-list pointer.
    ///
    /// SOURCE: complete ESP32-S31 `libble_app.a` `ble_phy.c` member `72.o`
    /// symbols `r_sym_ble_LboRu27EaU8MV8Q7UUfZ` and
    /// `r_sym_ble_ZzrExMrn8EDiTFI7PENK`. Selector values one through three
    /// choose the three A/B pairs at `0x20101280..=0x20101294`. The exact
    /// transaction performs a fresh read/write clearing bits 0..19, then a
    /// second fresh read/write OR-ing the compressed image into that complete
    /// second read. The latter detail intentionally preserves any bits that
    /// hardware may publish between the two RMW operations.
    ///
    /// This API deliberately retains positional selector/slot names. The
    /// vendor evidence does not yet establish RX/TX direction or list
    /// semantics.
    ///
    /// # Safety
    ///
    /// The caller must own a powered controller lifecycle state in which the
    /// selected hardware list may be changed, and must ensure the pointed-to
    /// storage remains correctly initialized, exclusively serialized, and
    /// alive until a later verified transaction removes it from hardware.
    #[allow(
        unsafe_code,
        reason = "the signature retains controller-lifecycle and pointed-storage prerequisites"
    )]
    #[doc(hidden)]
    pub unsafe fn program_memory_list_pointer(
        &mut self,
        selector: BluetoothMemoryListSelector,
        slot: BluetoothMemoryListSlot,
        image: BluetoothMemoryListPointerImage,
    ) {
        let compressed = image.compressed();

        macro_rules! program {
            ($register:expr) => {{
                let register = $register;
                register.modify(|_, writer| unsafe { writer.compressed_sram_pointer().bits(0) });
                register
                    .modify(|reader, writer| unsafe { writer.bits(reader.bits() | compressed) });
            }};
        }

        let controller = &self.bluetooth.bluetooth_controller_core;
        match (selector, slot) {
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::A) => {
                program!(controller.mmgmt_list_1_pointer_a());
            }
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::B) => {
                program!(controller.mmgmt_list_1_pointer_b());
            }
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::A) => {
                program!(controller.mmgmt_list_2_pointer_a());
            }
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::B) => {
                program!(controller.mmgmt_list_2_pointer_b());
            }
            (BluetoothMemoryListSelector::Three, BluetoothMemoryListSlot::A) => {
                program!(controller.mmgmt_list_3_pointer_a());
            }
            (BluetoothMemoryListSelector::Three, BluetoothMemoryListSlot::B) => {
                program!(controller.mmgmt_list_3_pointer_b());
            }
        }
        device_fence();
    }
}

#[cfg(test)]
mod tests {
    use super::super::RadioHardware;
    use super::{
        BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
        BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
    };

    #[test]
    fn compressed_controller_address_rejects_unrepresentable_values() {
        assert_eq!(
            BluetoothControllerSramAddress::new(0x2f00_0001),
            Err(BluetoothControllerSramAddressError::Unaligned)
        );
        assert_eq!(
            BluetoothControllerSramAddress::new(0x2e00_0000),
            Err(BluetoothControllerSramAddressError::OutsideEncodableWindow)
        );
        assert_eq!(
            BluetoothControllerSramAddress::new(0x2f40_0000),
            Err(BluetoothControllerSramAddressError::OutsideEncodableWindow)
        );

        let first =
            BluetoothControllerSramAddress::new(0x2f00_0000).expect("window base is representable");
        let last =
            BluetoothControllerSramAddress::new(0x2f3f_fffc).expect("window end is representable");
        assert_eq!(first.address(), 0x2f00_0000);
        assert_eq!(first.compressed(), 0);
        assert_eq!(last.compressed(), 0x000f_ffff);
        assert_eq!(BluetoothMemoryListPointerImage::Zero.compressed(), 0);
        assert_eq!(
            BluetoothMemoryListPointerImage::Address(last).compressed(),
            0x000f_ffff
        );
    }

    #[test]
    fn memory_list_pairs_follow_the_reviewed_selector_geometry() {
        let cold = RadioHardware::for_validation().into_bluetooth();
        let (task, _interrupts) = cold.separate_interrupt_owner();
        let controller = &task.bluetooth.bluetooth_controller_core;

        let addresses = [
            controller.mmgmt_list_1_pointer_a().as_ptr() as usize,
            controller.mmgmt_list_1_pointer_b().as_ptr() as usize,
            controller.mmgmt_list_2_pointer_a().as_ptr() as usize,
            controller.mmgmt_list_2_pointer_b().as_ptr() as usize,
            controller.mmgmt_list_3_pointer_a().as_ptr() as usize,
            controller.mmgmt_list_3_pointer_b().as_ptr() as usize,
        ];
        assert_eq!(
            addresses,
            [
                0x2010_1280,
                0x2010_1284,
                0x2010_1288,
                0x2010_128c,
                0x2010_1290,
                0x2010_1294,
            ]
        );

        let _all = [
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::A),
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::B),
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::A),
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::B),
            (
                BluetoothMemoryListSelector::Three,
                BluetoothMemoryListSlot::A,
            ),
            (
                BluetoothMemoryListSelector::Three,
                BluetoothMemoryListSlot::B,
            ),
        ];
    }
}
