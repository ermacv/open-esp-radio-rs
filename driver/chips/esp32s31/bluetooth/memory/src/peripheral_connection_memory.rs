//! Pinned allocation graph for one ESP32-S31 BLE peripheral connection.
//!
//! This is the stable memory boundary recovered from the current controller
//! artifact.  It owns the two reusable scheduler items, their shared context,
//! the connection link state and the initially empty transmit queue sentinel.
//! It deliberately does not encode anchor policy, packet sequence state or a
//! hardware-ready event image.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    scheduler_context::BluetoothSchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Bytes retained by one connection link-state allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES: usize = 0x84;
/// Bytes retained by one connection scheduler-item allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Scheduler items retained by one connection allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT: usize = 2;
/// Bytes retained by the initially empty transmit queue sentinel.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES: usize = 0x18;

const LINK_STATE_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES / 4;
const TX_SENTINEL_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES / 4;

const LINK_STATE_SCHEDULER_HEAD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL: usize = 0x74 / 4;
const LINK_STATE_RX_RESERVE: usize = 0x78 / 4;

const SCHEDULER_ITEM_NEXT: usize = 0;
const SCHEDULER_ITEM_CONTEXT: usize = 1;
const SCHEDULER_ITEM_LINK_STATE: usize = 2;
const SCHEDULER_ITEM_CLASS: usize = 0x4c / 4;
const SCHEDULER_ITEM_LINK_MASK: u32 = 0x000f_ffff;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0010_0000;
const SCHEDULER_ITEM_PERIPHERAL_PREFIX: u32 = 0x0020_0000;
const SCHEDULER_ITEM_CONNECTION_CLASS: u32 = 3 << 8;

const TX_SENTINEL_STATE: usize = 0x0c / 4;
const TX_SENTINEL_CLASS: usize = 0x10 / 4;
const TX_SENTINEL_EMPTY_QUEUE: u32 = 0x8000_0000;
const TX_SENTINEL_CONNECTION_CLASS: u32 = 2;

#[repr(C, align(4))]
struct BluetoothPeripheralConnectionLinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl BluetoothPeripheralConnectionLinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
        }
    }

    fn initialize_allocation(
        &self,
        scheduler_head: BluetoothControllerSramLinkAddress,
        tx_sentinel: BluetoothControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.words[LINK_STATE_SCHEDULER_HEAD].set(scheduler_head.controller_address().address());
        self.words[LINK_STATE_TX_HEAD].set(tx_sentinel.controller_address().address());
        self.words[LINK_STATE_TX_TAIL].set(tx_sentinel.controller_address().address());
    }

    fn has_empty_receive_queue(&self) -> bool {
        self.words[LINK_STATE_RX_HEAD].get() == 0
            && self.words[LINK_STATE_RX_TAIL].get() == 0
            && self.words[LINK_STATE_RX_RESERVE].get() == 0
    }

    fn retains_transmit_sentinel(&self, sentinel: BluetoothControllerSramLinkAddress) -> bool {
        let address = sentinel.controller_address().address();
        self.words[LINK_STATE_TX_HEAD].get() == address
            && self.words[LINK_STATE_TX_TAIL].get() == address
    }

    fn retains_scheduler_head(&self, head: BluetoothControllerSramLinkAddress) -> bool {
        self.words[LINK_STATE_SCHEDULER_HEAD].get() == head.controller_address().address()
    }
}

#[repr(C, align(4))]
struct BluetoothPeripheralConnectionSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl BluetoothPeripheralConnectionSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn initialize_allocation(
        &self,
        successor: Option<BluetoothControllerSramLinkAddress>,
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        let successor = successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        self.words[SCHEDULER_ITEM_NEXT].set(SCHEDULER_ITEM_ALLOCATION_PREFIX | successor);
        self.words[SCHEDULER_ITEM_CONTEXT].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE]
            .set(SCHEDULER_ITEM_PERIPHERAL_PREFIX | link_state.compressed_image());
        self.words[SCHEDULER_ITEM_CLASS].set(SCHEDULER_ITEM_CONNECTION_CLASS);
    }

    fn retains_allocation(
        &self,
        successor: Option<BluetoothControllerSramLinkAddress>,
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) -> bool {
        let successor = successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        self.words[SCHEDULER_ITEM_NEXT].get() & SCHEDULER_ITEM_LINK_MASK == successor
            && self.words[SCHEDULER_ITEM_CONTEXT].get() & SCHEDULER_ITEM_LINK_MASK
                == scheduler_context.compressed_image()
            && self.words[SCHEDULER_ITEM_LINK_STATE].get() & SCHEDULER_ITEM_LINK_MASK
                == link_state.compressed_image()
    }
}

#[repr(C, align(4))]
struct BluetoothPeripheralConnectionTxSentinelStorage {
    words: [VolatileCell<u32>; TX_SENTINEL_WORDS],
}

impl BluetoothPeripheralConnectionTxSentinelStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; TX_SENTINEL_WORDS],
        }
    }

    fn initialize_empty(&self) {
        for word in &self.words {
            word.set(0);
        }
        self.words[TX_SENTINEL_STATE].set(TX_SENTINEL_EMPTY_QUEUE);
        self.words[TX_SENTINEL_CLASS].set(TX_SENTINEL_CONNECTION_CLASS);
    }

    fn is_empty_queue_sentinel(&self) -> bool {
        self.words[TX_SENTINEL_STATE].get() == TX_SENTINEL_EMPTY_QUEUE
            && self.words[TX_SENTINEL_CLASS].get() == TX_SENTINEL_CONNECTION_CLASS
    }
}

/// Static storage for the allocation-time graph of one peripheral connection.
#[pin_project]
#[repr(C)]
pub struct BluetoothPeripheralConnectionMemoryGraphStorage {
    link_state: BluetoothPeripheralConnectionLinkStateStorage,
    scheduler_context: BluetoothSchedulerContextStorage,
    scheduler_items: [BluetoothPeripheralConnectionSchedulerItemStorage;
        BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: BluetoothPeripheralConnectionTxSentinelStorage,
    #[pin]
    _pin: PhantomPinned,
}

const GRAPH_BYTES: u32 =
    core::mem::size_of::<BluetoothPeripheralConnectionMemoryGraphStorage>() as u32;
const LINK_STATE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPeripheralConnectionMemoryGraphStorage, link_state) as u32;
const SCHEDULER_CONTEXT_OFFSET: u32 = core::mem::offset_of!(
    BluetoothPeripheralConnectionMemoryGraphStorage,
    scheduler_context
) as u32;
const SCHEDULER_ITEMS_OFFSET: u32 = core::mem::offset_of!(
    BluetoothPeripheralConnectionMemoryGraphStorage,
    scheduler_items
) as u32;
const TX_SENTINEL_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPeripheralConnectionMemoryGraphStorage, tx_sentinel) as u32;

/// Why peripheral-connection storage cannot become a bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
}

/// Failed binding that returns the exact unchanged static allocation.
pub struct BluetoothPeripheralConnectionMemoryGraphBindFailure {
    storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
    error: BluetoothPeripheralConnectionMemoryGraphBindError,
}

impl BluetoothPeripheralConnectionMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        error: BluetoothPeripheralConnectionMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> BluetoothPeripheralConnectionMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        BluetoothPeripheralConnectionMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothPeripheralConnectionMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPeripheralConnectionMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothPeripheralConnectionMemoryGraphModelAddress {
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }

    const fn address(self) -> u32 {
        self.0.address()
    }
}

/// Immutable geometry retained by the peripheral-connection graph owner.
pub struct BluetoothPeripheralConnectionMemoryGraphBinding {
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramLinkAddress,
    scheduler_items:
        [BluetoothControllerSramLinkAddress; BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: BluetoothControllerSramLinkAddress,
}

impl BluetoothPeripheralConnectionMemoryGraphBinding {
    fn new(base: u32) -> Result<Self, BluetoothPeripheralConnectionMemoryGraphBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothPeripheralConnectionMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || GRAPH_BYTES > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(
                BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram,
            );
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let link = |offset: u32| {
            BluetoothControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| BluetoothPeripheralConnectionMemoryGraphBindError::ZeroCompressedLink)
        };
        let scheduler_item = |index: usize| {
            let index = u32::try_from(index).map_err(|_| {
                BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram
            })?;
            let offset = index
                .checked_mul(BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES as u32)
                .and_then(|offset| SCHEDULER_ITEMS_OFFSET.checked_add(offset))
                .ok_or(
                    BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram,
                )?;
            link(offset)
        };

        Ok(Self {
            base: base_address,
            end_exclusive: base + GRAPH_BYTES,
            link_state: link(LINK_STATE_OFFSET)?,
            scheduler_context: link(SCHEDULER_CONTEXT_OFFSET)?,
            scheduler_items: [scheduler_item(0)?, scheduler_item(1)?],
            tx_sentinel: link(TX_SENTINEL_OFFSET)?,
        })
    }

    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    pub const fn link_state_address(&self) -> BluetoothControllerSramLinkAddress {
        self.link_state
    }

    pub const fn scheduler_head_address(&self) -> BluetoothControllerSramLinkAddress {
        self.scheduler_items[1]
    }

    pub const fn scheduler_context_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_context.controller_address()
    }

    pub const fn tx_sentinel_address(&self) -> BluetoothControllerSramLinkAddress {
        self.tx_sentinel
    }
}

/// Unique CPU owner of one allocation-time peripheral-connection graph.
#[must_use = "the bound peripheral-connection graph must be retained"]
pub struct BluetoothPeripheralConnectionMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothPeripheralConnectionMemoryGraphStorage>,
    binding: BluetoothPeripheralConnectionMemoryGraphBinding,
}

impl BluetoothPeripheralConnectionMemoryGraphCpuOwned {
    pub const fn binding(&self) -> &BluetoothPeripheralConnectionMemoryGraphBinding {
        &self.binding
    }

    /// The recovered allocation starts without any receive buffer owner.
    pub fn has_empty_receive_queue(&self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .link_state
            .has_empty_receive_queue()
    }

    /// The recovered allocation starts with one shared head/tail TX sentinel.
    pub fn has_empty_transmit_queue(&self) -> bool {
        let graph = self.storage.as_ref().get_ref();
        graph
            .link_state
            .retains_transmit_sentinel(self.binding.tx_sentinel)
            && graph.tx_sentinel.is_empty_queue_sentinel()
    }

    fn reinitialize_graph(&mut self) {
        let graph = self.storage.as_mut().project();
        graph.scheduler_context.clear();
        graph.scheduler_items[0].initialize_allocation(
            None,
            self.binding.scheduler_context,
            self.binding.link_state,
        );
        graph.scheduler_items[1].initialize_allocation(
            Some(self.binding.scheduler_items[0]),
            self.binding.scheduler_context,
            self.binding.link_state,
        );
        graph
            .link_state
            .initialize_allocation(self.binding.scheduler_items[1], self.binding.tx_sentinel);
        graph.tx_sentinel.initialize_empty();
    }

    /// Both reusable scheduler items still form the recovered private pool.
    pub fn has_recovered_scheduler_pool(&self) -> bool {
        let graph = self.storage.as_ref().get_ref();
        graph
            .link_state
            .retains_scheduler_head(self.binding.scheduler_items[1])
            && graph.scheduler_items[0].retains_allocation(
                None,
                self.binding.scheduler_context,
                self.binding.link_state,
            )
            && graph.scheduler_items[1].retains_allocation(
                Some(self.binding.scheduler_items[0]),
                self.binding.scheduler_context,
                self.binding.link_state,
            )
    }
}

impl BluetoothPeripheralConnectionMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothPeripheralConnectionLinkStateStorage::new(),
            scheduler_context: BluetoothSchedulerContextStorage::new(),
            scheduler_items: [const { BluetoothPeripheralConnectionSchedulerItemStorage::new() };
                BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
            tx_sentinel: BluetoothPeripheralConnectionTxSentinelStorage::new(),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        BluetoothPeripheralConnectionMemoryGraphBindFailure,
    > {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothPeripheralConnectionMemoryGraphBindFailure::new(
                    storage,
                    BluetoothPeripheralConnectionMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothPeripheralConnectionMemoryGraphModelAddress,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        BluetoothPeripheralConnectionMemoryGraphBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        BluetoothPeripheralConnectionMemoryGraphBindFailure,
    > {
        let binding = match BluetoothPeripheralConnectionMemoryGraphBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothPeripheralConnectionMemoryGraphBindFailure::new(
                    storage, error,
                ));
            }
        };
        let mut owner = BluetoothPeripheralConnectionMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

impl Default for BluetoothPeripheralConnectionMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothPeripheralConnectionMemoryGraphBindError,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };

    fn storage() -> &'static mut BluetoothPeripheralConnectionMemoryGraphStorage {
        std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ))
    }

    #[test]
    fn binding_builds_the_recovered_allocation_topology() {
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        let owner =
            BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
                .expect("the complete graph fits physical controller SRAM");

        assert!(owner.has_recovered_scheduler_pool());
        assert!(owner.has_empty_receive_queue());
        assert!(owner.has_empty_transmit_queue());
    }

    #[test]
    fn out_of_window_binding_returns_the_same_storage() {
        let storage = storage();
        let identity = core::ptr::addr_of!(*storage);
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f07_fff0)
            .expect("the final aligned controller SRAM address is syntactically valid");
        let failure = match BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(
            storage, base,
        ) {
            Ok(_) => panic!("the complete graph crosses the physical SRAM boundary"),
            Err(failure) => failure,
        };

        assert_eq!(
            failure.error(),
            BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::addr_of!(*storage), identity);
    }
}
