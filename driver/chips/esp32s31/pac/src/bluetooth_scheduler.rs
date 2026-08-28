//! Restricted scheduler hardware-list publication transactions.

#![deny(unsafe_code)]

use super::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex, BluetoothTaskRegisters,
    device_fence,
};

/// Head published to one scheduler hardware list.
///
/// A non-empty value proves only the controller pointer encoding. Allocation,
/// descriptor initialization, lifetime and exclusive hardware ownership stay
/// with the controller-memory layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerHardwareListHead(Option<BluetoothControllerSramAddress>);

/// Why an SRAM address cannot represent a published scheduler-list item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerHardwareListHeadError {
    /// The address compresses to the reserved empty-head image.
    EncodesEmptyHead,
}

impl BluetoothSchedulerHardwareListHead {
    /// Construct the empty hardware-list head.
    pub const fn empty() -> Self {
        Self(None)
    }

    /// Construct a non-empty head from one validated scheduler-item address.
    pub const fn from_address(
        address: BluetoothControllerSramAddress,
    ) -> Result<Self, BluetoothSchedulerHardwareListHeadError> {
        if address.compressed_image() == 0 {
            Err(BluetoothSchedulerHardwareListHeadError::EncodesEmptyHead)
        } else {
            Ok(Self(Some(address)))
        }
    }

    /// Address of the published scheduler item, or `None` for an empty list.
    pub const fn address(self) -> Option<BluetoothControllerSramAddress> {
        self.0
    }

    const fn compressed_image(self) -> u32 {
        match self.0 {
            None => 0,
            Some(address) => address.compressed_image(),
        }
    }

    const fn from_compressed_image(image: u32) -> Self {
        if image == 0 {
            Self::empty()
        } else {
            Self(Some(BluetoothControllerSramAddress::from_compressed_image(
                image,
            )))
        }
    }
}

/// One of the two positional scheduler command words whose START field is
/// cleared by insertion-end result paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerInsertionCommand {
    /// `SCHEDULER_COMMAND_0`, selected by insertion-begin/end result four.
    Zero,
    /// `SCHEDULER_COMMAND_1`, selected by insertion-begin/end result five.
    One,
}

/// Affine evidence that one hardware-list head was published and the trailing
/// device fence completed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the published scheduler head must feed insertion-end ownership"]
pub struct BluetoothSchedulerHardwareListHeadPublished {
    index: BluetoothSchedulerHardwareListIndex,
    head: BluetoothSchedulerHardwareListHead,
}

impl BluetoothSchedulerHardwareListHeadPublished {
    /// Hardware-list index updated by the publication.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.index
    }

    /// Head retained by the published hardware-list entry.
    pub const fn head(&self) -> BluetoothSchedulerHardwareListHead {
        self.head
    }
}

/// Affine evidence that one insertion command no longer carries START and the
/// trailing device fence completed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the cleared command must feed insertion-end ownership"]
pub struct BluetoothSchedulerInsertionCommandStartCleared {
    command: BluetoothSchedulerInsertionCommand,
}

impl BluetoothSchedulerInsertionCommandStartCleared {
    /// Positional command whose START field was cleared.
    pub const fn command(&self) -> BluetoothSchedulerInsertionCommand {
        self.command
    }
}

/// Affine evidence that the hardware RUN command and trailing device fence
/// completed.
///
/// This is deliberately not evidence of the complete vendor scheduler-run
/// operation. Dynamic interrupt preparation and software broker publication
/// precede this final MMIO command and remain separately owned.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the hardware run command must feed controller ownership"]
pub struct BluetoothSchedulerHardwareRunCommandPublished {
    _private: (),
}

impl BluetoothTaskRegisters {
    /// Remove every published scheduler hardware-list head.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` member `btdm_sched.c`
    /// symbol `r_sym_bt_XPuqTHliEO5V9xpR7aJR`. Its first hardware transaction
    /// walks `0x2010_b000..=0x2010_b0f0` with stride `0x10`; every entry is
    /// freshly read, bits 19:0 are cleared, and bits 31:20 are preserved.
    ///
    /// This method deliberately does not expose the later software event and
    /// list initialization performed by the vendor function, and therefore
    /// does not claim that the complete controller or scheduler is running.
    pub fn clear_scheduler_hardware_list_heads(&mut self) {
        for entry in self.bluetooth.btdm_scheduler_table.entry_iter() {
            entry.modify(|_, writer| writer.head_pointer().set(0));
        }
        device_fence();
    }

    /// Read the currently published head of one scheduler hardware list.
    ///
    /// SOURCE: complete current insertion-begin
    /// `r_sym_bt_EabYtUaAIR05LXw3qZSA` and same-chip named
    /// `r_btdm_sched_get_hw_list_header`. Zero is returned as no head;
    /// nonzero images reconstruct a four-byte-aligned 0x2f-prefixed address.
    #[doc(hidden)]
    pub fn scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        let image = self
            .bluetooth
            .btdm_scheduler_table
            .entry(index.get() as usize)
            .read()
            .head_pointer()
            .bits();
        BluetoothSchedulerHardwareListHead::from_compressed_image(image)
    }

    /// Publish one scheduler hardware-list head while preserving the
    /// unassigned upper twelve entry bits.
    ///
    /// SOURCE: complete current insertion-end
    /// `r_sym_bt_4KfpZh0Hu5NprlqcNu0D` and same-chip named
    /// `r_btdm_sched_set_hw_list_header`.
    ///
    /// # Safety
    ///
    /// The caller must own the scheduler insertion epoch, must keep an
    /// addressed scheduler item initialized and alive until a later verified
    /// completion/removal transaction, and must serialize this publication
    /// with the scheduler interrupt owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler-epoch prerequisites"
    )]
    pub unsafe fn publish_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        self.bluetooth
            .btdm_scheduler_table
            .entry(index.get() as usize)
            .modify(|_, writer| writer.head_pointer().set(head.compressed_image()));
        device_fence();
        BluetoothSchedulerHardwareListHeadPublished { index, head }
    }

    /// Clear START in the positional command selected by insertion-end.
    ///
    /// # Safety
    ///
    /// The caller must have obtained the matching insertion-begin result and
    /// must serialize the RMW with every command producer and interrupt owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the insertion result and command serialization"
    )]
    pub unsafe fn clear_scheduler_insertion_command_start(
        &mut self,
        command: BluetoothSchedulerInsertionCommand,
    ) -> BluetoothSchedulerInsertionCommandStartCleared {
        match command {
            BluetoothSchedulerInsertionCommand::Zero => self
                .bluetooth
                .bluetooth_controller_core
                .scheduler_command_0()
                .modify(|_, writer| writer.start().clear_bit()),
            BluetoothSchedulerInsertionCommand::One => self
                .bluetooth
                .bluetooth_controller_core
                .scheduler_command_1()
                .modify(|_, writer| writer.start().clear_bit()),
        };
        device_fence();
        BluetoothSchedulerInsertionCommandStartCleared { command }
    }

    /// Publish the finite scheduler hardware RUN command.
    ///
    /// # Safety
    ///
    /// The caller must have completed the required head publication, dynamic
    /// interrupt preparation and software broker publication before issuing
    /// this final command.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the complete scheduler-run prefix and insertion ownership"
    )]
    pub unsafe fn publish_scheduler_hardware_run_command(
        &mut self,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        super::svd::fixed_register_write::publish_bluetooth_scheduler_hardware_run_command(
            &self.bluetooth.bluetooth_controller_core,
        );
        device_fence();
        BluetoothSchedulerHardwareRunCommandPublished { _private: () }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothControllerSramAddress, BluetoothSchedulerHardwareListHead,
        BluetoothSchedulerHardwareListHeadError,
    };

    #[test]
    fn non_empty_head_cannot_alias_the_empty_scheduler_state() {
        let empty_image = BluetoothControllerSramAddress::new(0x2f00_0000)
            .expect("controller SRAM window base is representable");
        assert_eq!(
            BluetoothSchedulerHardwareListHead::from_address(empty_image),
            Err(BluetoothSchedulerHardwareListHeadError::EncodesEmptyHead)
        );

        let item = BluetoothControllerSramAddress::new(0x2f00_0004)
            .expect("first non-empty item address is representable");
        assert_eq!(
            BluetoothSchedulerHardwareListHead::from_address(item)
                .expect("non-empty controller address is a valid list head")
                .address(),
            Some(item)
        );
    }
}
