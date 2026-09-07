//! Legacy advertising session adaptation, including connectable roles.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod connectable;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first;

#[cfg(target_arch = "riscv32")]
pub use active::{
    LegacyAdvertisingActiveDrive, LegacyAdvertisingDelaySource, LegacyAdvertisingRecurringDrive,
    drive_legacy_advertising_active_ready, drive_legacy_advertising_recurring_ready,
};
#[cfg(target_arch = "riscv32")]
pub use connectable::{
    active::{
        LegacyConnectableAdvertisingReadyContinuations,
        drive_legacy_connectable_advertising_active_ready,
        drive_legacy_connectable_advertising_initial_pending_ready_with,
        drive_legacy_connectable_advertising_pending_ready_with,
        drive_legacy_connectable_advertising_stopping_ready,
    },
    first::{
        LegacyConnectableAdvertisingFirstControllerTimeWait,
        LegacyConnectableAdvertisingFirstDrive, LegacyConnectableAdvertisingFirstResume,
        drive_legacy_connectable_advertising_first_ready,
    },
    recurring::{
        AdvertisingForwardOrder, LegacyConnectableAdvertisingRecurringCancellationReady,
        LegacyConnectableAdvertisingRecurringCancellationWait,
        LegacyConnectableAdvertisingRecurringControllerTimeReady,
        LegacyConnectableAdvertisingRecurringControllerTimeWait,
        LegacyConnectableAdvertisingRecurringDriveHandler,
        LegacyConnectableAdvertisingRecurringStopHandler,
        begin_legacy_connectable_advertising_recurring_command_ready_with,
        begin_legacy_connectable_advertising_recurring_response_pending_with,
        cancel_legacy_connectable_advertising_recurring_candidate_with,
        cancel_legacy_connectable_advertising_recurring_graph_prepared_with,
        cancel_legacy_connectable_advertising_recurring_merged_with,
        cancel_legacy_connectable_advertising_recurring_prepared_with,
        cancel_legacy_connectable_advertising_recurring_scheduled_with,
        cancel_legacy_connectable_advertising_recurring_sequence_pending_with,
        cancel_legacy_connectable_advertising_recurring_sequence_ready_with,
        drive_legacy_connectable_advertising_recurring_candidate_with,
        drive_legacy_connectable_advertising_recurring_graph_prepared_with,
        drive_legacy_connectable_advertising_recurring_merged_with,
        drive_legacy_connectable_advertising_recurring_prepared_with,
        finish_legacy_connectable_advertising_no_connection_stopping_with,
        retain_legacy_connectable_advertising_recurring_controller_time,
        retain_legacy_connectable_advertising_recurring_retry_for_hci,
    },
};
#[cfg(target_arch = "riscv32")]
pub use first::{
    LegacyAdvertisingFirstControllerTimeWait, LegacyAdvertisingFirstDrive,
    LegacyAdvertisingFirstResume, drive_legacy_advertising_first_ready,
};
