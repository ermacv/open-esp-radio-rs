//! Restricted scheduler hardware-list publication transactions.

#![deny(unsafe_code)]

use super::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothTaskRegisters, device_fence,
};

trait BluetoothSchedulerHardwareListHeadControl {
    fn order_descriptor_before_publication(&mut self);
    fn publish_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    );
    fn order_after_publication(&mut self);
}

trait BluetoothSchedulerHardwareListHeadObservationControl {
    fn read_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead;
    fn order_after_observation(&mut self);
}

impl BluetoothSchedulerHardwareListHeadObservationControl for BluetoothTaskRegisters {
    fn read_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        self.scheduler_hardware_list_head(index)
    }

    fn order_after_observation(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_hardware_list_head_observation(
    control: &mut impl BluetoothSchedulerHardwareListHeadObservationControl,
    index: BluetoothSchedulerHardwareListIndex,
) -> BluetoothSchedulerHardwareListHead {
    let head = control.read_head(index);
    control.order_after_observation();
    head
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothSchedulerHardwareListHeadRetirementDisposition {
    Empty,
    ExpectedHeadStillPublished,
    UnexpectedHeadChanged,
}

fn classify_scheduler_hardware_list_head_retirement(
    expected: BluetoothSchedulerHardwareListHead,
    observed: BluetoothSchedulerHardwareListHead,
) -> BluetoothSchedulerHardwareListHeadRetirementDisposition {
    if observed.address().is_none() {
        BluetoothSchedulerHardwareListHeadRetirementDisposition::Empty
    } else if observed == expected {
        BluetoothSchedulerHardwareListHeadRetirementDisposition::ExpectedHeadStillPublished
    } else {
        BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged
    }
}

impl BluetoothSchedulerHardwareListHeadControl for BluetoothTaskRegisters {
    fn order_descriptor_before_publication(&mut self) {
        device_fence();
    }

    fn publish_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) {
        let head_bits =
            super::generated::BluetoothSchedulerListHeadBits::new(head.compressed_image())
                .expect("validated scheduler head fits its generated PAC domain");
        super::generated::publish_bluetooth_scheduler_hardware_list_head(
            &self.bluetooth.btdm_scheduler_table,
            index.get() as usize,
            head_bits,
        );
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_hardware_list_head_publication(
    control: &mut impl BluetoothSchedulerHardwareListHeadControl,
    index: BluetoothSchedulerHardwareListIndex,
    head: BluetoothSchedulerHardwareListHead,
) -> BluetoothSchedulerHardwareListHeadPublished {
    control.order_descriptor_before_publication();
    control.publish_head(index, head);
    control.order_after_publication();
    BluetoothSchedulerHardwareListHeadPublished { index, head }
}

trait BluetoothSchedulerRunEventControl {
    fn clear_scheduler_run_event_source(&mut self);
    fn enable_scheduler_run_event_source(&mut self);
    fn order_after_scheduler_run_event(&mut self);
}

impl BluetoothSchedulerRunEventControl for BluetoothTaskRegisters {
    fn clear_scheduler_run_event_source(&mut self) {
        super::svd::fixed_register_image::clear_ble_scheduler_run_event_source(
            &self.bluetooth.btmac_ble_phy_init,
        );
    }

    fn enable_scheduler_run_event_source(&mut self) {
        super::generated::enable_ble_scheduler_run_event_source(&self.bluetooth.btmac_ble_phy_init);
    }

    fn order_after_scheduler_run_event(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_run_event_publication(control: &mut impl BluetoothSchedulerRunEventControl) {
    control.clear_scheduler_run_event_source();
    control.enable_scheduler_run_event_source();
    control.order_after_scheduler_run_event();
}

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

/// Affine evidence that descriptor writes were ordered before one hardware-list
/// head publication and the trailing device fence completed.
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

/// Affine proof that every scheduler hardware-list head is empty after the
/// complete initialization transaction and its trailing device fence.
///
/// This does not prove that arbitrary software list containers are empty. The
/// Controller may use it only to seed a new source-owned exclusive list epoch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the cleared hardware-list epoch must be retained by scheduler ownership"]
pub struct BluetoothSchedulerHardwareListsCleared {
    _private: (),
}

#[cfg(feature = "validation-probes")]
impl BluetoothSchedulerHardwareListsCleared {
    /// Construct host-only initialization evidence without MMIO.
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self { _private: () }
    }
}

/// Affine evidence that the synchronous scheduler-run subscriber completed.
///
/// The exact current subscriber acknowledges stale BTMAC source 14, enables
/// that source through a fresh-read field update and completes a trailing
/// device fence. The value also retains the matching head and dynamic
/// interrupt proof, so the final RUN command cannot be reached with a partial
/// or reordered prologue.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the complete scheduler-run event must feed the hardware RUN command"]
pub struct BluetoothSchedulerRunEventPublished {
    head: BluetoothSchedulerHardwareListHeadPublished,
    _interrupts: BluetoothSchedulerRunInterruptsPrepared,
}

impl BluetoothSchedulerRunEventPublished {
    /// Hardware list whose head remains retained through the run prologue.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }

    /// Exact head retained through the run prologue.
    pub const fn head(&self) -> BluetoothSchedulerHardwareListHead {
        self.head.head()
    }
}

/// Affine evidence that the hardware RUN command and trailing device fence
/// completed.
///
/// This value retains the complete typed run prologue: hardware-list head,
/// dynamic interrupt preparation and the synchronous BTMAC scheduler event.
/// It is not evidence of radio completion.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the hardware run command must feed controller ownership"]
pub struct BluetoothSchedulerHardwareRunCommandPublished {
    event: BluetoothSchedulerRunEventPublished,
}

/// RUN provenance after one fresh fenced observation found its list empty.
///
/// The token is affine and retains the complete run prologue. It proves only
/// that the hardware head was empty at this post-completion observation; the
/// software-list removal predicate and descriptor reclamation remain separate.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the empty hardware-head observation must feed software-list removal"]
pub struct BluetoothSchedulerHardwareListHeadEmptyObserved {
    run: BluetoothSchedulerHardwareRunCommandPublished,
}

impl BluetoothSchedulerHardwareListHeadEmptyObserved {
    /// Hardware list whose head was freshly observed empty.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }

    /// Originally published head retained by the completed RUN epoch.
    pub const fn completed_head(&self) -> BluetoothSchedulerHardwareListHead {
        self.run.head()
    }
}

/// Result of one bounded post-completion hardware-head observation.
#[must_use = "the retained RUN provenance must enter fail-stop handling or advance"]
pub enum BluetoothSchedulerHardwareListHeadRetirementObservation {
    /// The originally published head remains nonempty at the mandatory gate.
    /// The DTM sole-item owner must treat this as a fail-stop invariant.
    ExpectedHeadStillPublished {
        /// Unchanged affine RUN provenance retained for fail-stop handling.
        run: BluetoothSchedulerHardwareRunCommandPublished,
        /// Freshly observed typed head, retained only as diagnostic identity.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// A different nonempty head was observed after this RUN publication.
    UnexpectedHeadChanged {
        /// Unchanged affine RUN provenance; callers must fail closed.
        run: BluetoothSchedulerHardwareRunCommandPublished,
        /// Fresh conflicting head identity.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// The matching hardware-list head was freshly observed empty.
    EmptyObserved(BluetoothSchedulerHardwareListHeadEmptyObserved),
}

impl BluetoothSchedulerHardwareRunCommandPublished {
    /// Hardware list now admitted to scheduler execution.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.event.index()
    }

    /// Exact head retained while hardware owns scheduler execution.
    pub const fn head(&self) -> BluetoothSchedulerHardwareListHead {
        self.event.head()
    }
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
    pub fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        for index in 0..16 {
            super::generated::clear_bluetooth_scheduler_hardware_list_head(
                &self.bluetooth.btdm_scheduler_table,
                index,
            );
        }
        device_fence();
        BluetoothSchedulerHardwareListsCleared { _private: () }
    }

    /// Read the currently published head of one scheduler hardware list.
    ///
    /// SOURCE: complete current insertion-begin
    /// `r_sym_bt_EabYtUaAIR05LXw3qZSA` and same-chip named
    /// `r_btdm_sched_get_hw_list_header`. Zero is returned as no head;
    /// nonzero images reconstruct a four-byte-aligned 0x2f-prefixed address.
    #[doc(hidden)]
    pub(crate) fn scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        let image = super::svd::field_read::observe_bluetooth_scheduler_hardware_list_head(
            &self.bluetooth.btdm_scheduler_table,
            index.get() as usize,
        );
        BluetoothSchedulerHardwareListHead::from_compressed_image(image)
    }

    /// Observe whether the hardware list admitted by one RUN is now empty.
    ///
    /// SOURCE: the post-picker tail of current
    /// `r_sym_ble_rmNuzAO8kQQQXQIpTzGZ`, mapped to same-chip named
    /// `r_sched_txn_onSchedHwListDone`, freshly reads the hardware-list head
    /// after the source manager becomes empty and asserts that head is empty
    /// before the following software-list removal call.
    ///
    /// This finite transaction consumes and returns the affine RUN provenance
    /// on both paths, so an old empty-head observation cannot be replayed
    /// against a later scheduler epoch. The trailing device fence orders the
    /// observation before any later CPU-side descriptor transition.
    pub fn observe_scheduler_hardware_list_head_retirement(
        &mut self,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerHardwareListHeadRetirementObservation {
        let observed = execute_scheduler_hardware_list_head_observation(self, run.index());
        match classify_scheduler_hardware_list_head_retirement(run.head(), observed) {
            BluetoothSchedulerHardwareListHeadRetirementDisposition::Empty => {
                BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(
                    BluetoothSchedulerHardwareListHeadEmptyObserved { run },
                )
            }
            BluetoothSchedulerHardwareListHeadRetirementDisposition::ExpectedHeadStillPublished => {
                BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                    run,
                    observed,
                }
            }
            BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged => {
                BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                    run,
                    observed,
                }
            }
        }
    }

    /// Order prior descriptor writes and publish one scheduler hardware-list
    /// head while preserving the unassigned upper twelve entry bits.
    ///
    /// SOURCE: complete current insertion-end
    /// `r_sym_bt_4KfpZh0Hu5NprlqcNu0D` and same-chip named
    /// `r_btdm_sched_set_hw_list_header`.
    ///
    /// # Safety
    ///
    /// The caller must own the scheduler insertion epoch, must have completed
    /// every CPU write to the addressed descriptor graph, must keep that graph
    /// alive until a later verified completion/removal transaction, and must
    /// serialize this publication with the scheduler interrupt owner. This
    /// method orders prior SRAM writes before the MMIO head update; it does not
    /// validate descriptor contents or establish completion ownership.
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
        execute_scheduler_hardware_list_head_publication(self, index, head)
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
            BluetoothSchedulerInsertionCommand::Zero => {
                super::generated::clear_bluetooth_scheduler_insertion_command_0_start(
                    &self.bluetooth.bluetooth_controller_core,
                );
            }
            BluetoothSchedulerInsertionCommand::One => {
                super::generated::clear_bluetooth_scheduler_insertion_command_1_start(
                    &self.bluetooth.bluetooth_controller_core,
                );
            }
        }
        device_fence();
        BluetoothSchedulerInsertionCommandStartCleared { command }
    }

    /// Publish the synchronous base-stack scheduler event required before RUN.
    ///
    /// SOURCE: complete current `r_sym_bt_PVKilXLQPu1BjRkm4C6O` publishes
    /// broker selector two after dynamic interrupt preparation. Its complete
    /// registered subscriber `r_sym_ble_uwrf0kLZsRbzFJ7u8SEr` acknowledges
    /// BTMAC source 14 before enabling it through a fresh-read RMW. This typed
    /// transaction replaces the generic callback list without making the
    /// synchronous hardware effect asynchronous.
    #[doc(hidden)]
    pub fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        execute_scheduler_run_event_publication(self);
        BluetoothSchedulerRunEventPublished {
            head,
            _interrupts: interrupts,
        }
    }

    /// Publish the finite scheduler hardware RUN command.
    ///
    /// Consuming [`BluetoothSchedulerRunEventPublished`] proves that head
    /// publication, dynamic interrupt preparation and the synchronous BTMAC
    /// subscriber all completed in the required order.
    #[doc(hidden)]
    pub fn publish_scheduler_hardware_run_command(
        &mut self,
        event: BluetoothSchedulerRunEventPublished,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        super::svd::fixed_register_write::publish_bluetooth_scheduler_hardware_run_command(
            &self.bluetooth.bluetooth_controller_core,
        );
        device_fence();
        BluetoothSchedulerHardwareRunCommandPublished { event }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothControllerSramAddress, BluetoothSchedulerHardwareListHead,
        BluetoothSchedulerHardwareListHeadControl, BluetoothSchedulerHardwareListHeadError,
        BluetoothSchedulerHardwareListHeadObservationControl,
        BluetoothSchedulerHardwareListHeadRetirementDisposition,
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerRunEventControl,
        classify_scheduler_hardware_list_head_retirement,
        execute_scheduler_hardware_list_head_observation,
        execute_scheduler_hardware_list_head_publication, execute_scheduler_run_event_publication,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum PublicationOperation {
        DescriptorFence,
        Publish {
            index: BluetoothSchedulerHardwareListIndex,
            head: BluetoothSchedulerHardwareListHead,
        },
        DeviceFence,
    }

    struct PublicationRecorder {
        operations: std::vec::Vec<PublicationOperation>,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ObservationOperation {
        Read(BluetoothSchedulerHardwareListIndex),
        DeviceFence,
    }

    struct ObservationRecorder {
        head: BluetoothSchedulerHardwareListHead,
        operations: std::vec::Vec<ObservationOperation>,
    }

    impl BluetoothSchedulerHardwareListHeadObservationControl for ObservationRecorder {
        fn read_head(
            &mut self,
            index: BluetoothSchedulerHardwareListIndex,
        ) -> BluetoothSchedulerHardwareListHead {
            self.operations.push(ObservationOperation::Read(index));
            self.head
        }

        fn order_after_observation(&mut self) {
            self.operations.push(ObservationOperation::DeviceFence);
        }
    }

    impl BluetoothSchedulerHardwareListHeadControl for PublicationRecorder {
        fn order_descriptor_before_publication(&mut self) {
            self.operations.push(PublicationOperation::DescriptorFence);
        }

        fn publish_head(
            &mut self,
            index: BluetoothSchedulerHardwareListIndex,
            head: BluetoothSchedulerHardwareListHead,
        ) {
            self.operations
                .push(PublicationOperation::Publish { index, head });
        }

        fn order_after_publication(&mut self) {
            self.operations.push(PublicationOperation::DeviceFence);
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RunEventOperation {
        ClearStaleSource,
        EnableSource,
        DeviceFence,
    }

    struct RunEventRecorder {
        operations: std::vec::Vec<RunEventOperation>,
    }

    impl BluetoothSchedulerRunEventControl for RunEventRecorder {
        fn clear_scheduler_run_event_source(&mut self) {
            self.operations.push(RunEventOperation::ClearStaleSource);
        }

        fn enable_scheduler_run_event_source(&mut self) {
            self.operations.push(RunEventOperation::EnableSource);
        }

        fn order_after_scheduler_run_event(&mut self) {
            self.operations.push(RunEventOperation::DeviceFence);
        }
    }

    #[test]
    fn scheduler_run_event_clears_stale_source_before_enabling_it() {
        let mut recorder = RunEventRecorder {
            operations: std::vec::Vec::new(),
        };

        execute_scheduler_run_event_publication(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                RunEventOperation::ClearStaleSource,
                RunEventOperation::EnableSource,
                RunEventOperation::DeviceFence,
            ]
        );
    }

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

    #[test]
    fn descriptor_visibility_precedes_head_publication() {
        let index = BluetoothSchedulerHardwareListIndex::ZERO;
        let head = BluetoothSchedulerHardwareListHead::from_address(
            BluetoothControllerSramAddress::new(0x2f00_0100)
                .expect("test item lies in controller SRAM"),
        )
        .expect("test item does not encode the empty head");
        let mut recorder = PublicationRecorder {
            operations: std::vec::Vec::new(),
        };

        let published =
            execute_scheduler_hardware_list_head_publication(&mut recorder, index, head);

        assert_eq!(published.index(), index);
        assert_eq!(published.head(), head);
        assert_eq!(
            recorder.operations,
            [
                PublicationOperation::DescriptorFence,
                PublicationOperation::Publish { index, head },
                PublicationOperation::DeviceFence,
            ]
        );
    }

    #[test]
    fn post_completion_head_observation_is_fenced_and_bounded() {
        let index = BluetoothSchedulerHardwareListIndex::ZERO;
        let head = BluetoothSchedulerHardwareListHead::empty();
        let mut recorder = ObservationRecorder {
            head,
            operations: std::vec::Vec::new(),
        };

        let observed = execute_scheduler_hardware_list_head_observation(&mut recorder, index);

        assert_eq!(observed, head);
        assert_eq!(
            recorder.operations,
            [
                ObservationOperation::Read(index),
                ObservationOperation::DeviceFence,
            ]
        );
    }

    #[test]
    fn head_retirement_distinguishes_empty_retained_and_changed_identity() {
        let expected = BluetoothSchedulerHardwareListHead::from_address(
            BluetoothControllerSramAddress::new(0x2f00_0100)
                .expect("expected item lies in controller SRAM"),
        )
        .expect("expected item is a nonempty head");
        let changed = BluetoothSchedulerHardwareListHead::from_address(
            BluetoothControllerSramAddress::new(0x2f00_0200)
                .expect("changed item lies in controller SRAM"),
        )
        .expect("changed item is a nonempty head");

        assert_eq!(
            classify_scheduler_hardware_list_head_retirement(
                expected,
                BluetoothSchedulerHardwareListHead::empty(),
            ),
            BluetoothSchedulerHardwareListHeadRetirementDisposition::Empty
        );
        assert_eq!(
            classify_scheduler_hardware_list_head_retirement(expected, expected),
            BluetoothSchedulerHardwareListHeadRetirementDisposition::ExpectedHeadStillPublished
        );
        assert_eq!(
            classify_scheduler_hardware_list_head_retirement(expected, changed),
            BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged
        );
    }
}
