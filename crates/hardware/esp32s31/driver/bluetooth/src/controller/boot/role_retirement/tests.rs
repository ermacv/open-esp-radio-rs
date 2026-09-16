use super::*;
use crate::{
    le::{advertising::*, dtm::*, peripheral::*, scanning::*},
    resources::runtime_owner::RuntimeOwnerSlot,
};
use oer_esp32s31_bluetooth_memory::*;
use std::boxed::Box;

fn roles() -> ControllerRoleResources {
    ControllerRoleResources {
        dtm_resources: DtmRuntimeResources::claim_static_model(
            Box::leak(Box::new(DtmMemoryGraphStorage::new())),
            DtmMemoryGraphModelAddress::new(0x2f00_1000).unwrap(),
            DtmRuntimeConfig::new(
                DtmSchedulerAllocationConfig::new(1, 2, 1),
                DtmDefaultTxPowerDbm::new(3),
            ),
        )
        .unwrap_or_else(|_| panic!("DTM model")),
        legacy_advertising_resources: LegacyAdvertisingRuntimeResources::claim_static_model(
            Box::leak(Box::new(LegacyAdvertisingMemoryGraphStorage::new())),
            LegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_8000).unwrap(),
            LegacyAdvertisingDefaultTxPowerDbm::new(4),
        )
        .unwrap_or_else(|_| panic!("advertising model")),
        legacy_connectable_advertising_resources:
            LegacyConnectableAdvertisingRuntimeResources::claim_static_model(
                Box::leak(Box::new(
                    LegacyConnectableAdvertisingMemoryGraphStorage::new(),
                )),
                LegacyConnectableAdvertisingMemoryGraphModelAddress::new(0x2f01_0000).unwrap(),
                LegacyAdvertisingDefaultTxPowerDbm::new(5),
            )
            .unwrap_or_else(|_| panic!("connectable model")),
        passive_scan_resources: PassiveScanRuntimeResources::claim_static_model(
            Box::leak(Box::new(PassiveScanMemoryGraphStorage::new())),
            PassiveScanMemoryGraphModelAddress::new(0x2f01_8000).unwrap(),
            PassiveScanRuntimeConfig::new(
                PassiveScanSchedulerAllocationConfig::new(0, 0).unwrap(),
                PassiveScanDefaultTxPowerDbm::new(6),
            ),
        )
        .unwrap_or_else(|_| panic!("scanner model")),
        peripheral_connection_resources: PeripheralConnectionRuntimeResources::claim_static_model(
            Box::leak(Box::new(PeripheralConnectionMemoryGraphStorage::new())),
            PeripheralConnectionMemoryGraphModelAddress::new(0x2f02_0000).unwrap(),
            Box::leak(Box::new(NonScanningRxMemoryStorage::new())),
            NonScanningRxMemoryModelAddress::new(0x2f02_8000).unwrap(),
            PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(7)),
        )
        .unwrap_or_else(|_| panic!("peripheral model")),
    }
}

fn rejected(
    lease: &mut crate::resources::runtime_owner::RuntimeOwnerLease<'_, ControllerRoleResources>,
    expected: ControllerRoleRetirementError,
) {
    let authority = Box::new(73);
    let address = core::ptr::from_ref(&*authority);
    let (error, authority) = lease
        .try_retire_roles(authority, |_| -> Result<(), ((), Box<i32>)> {
            panic!("busy role must reject before entering the HCI barrier")
        })
        .err()
        .expect("checked-out role must reject");
    assert!(matches!(error, RoleBarrierError::Role(actual) if actual == expected));
    assert_eq!(core::ptr::from_ref(&*authority), address);
}

#[test]
fn checked_out_roles_preserve_the_group_and_authority_until_exact_restore() {
    let mut slot = RuntimeOwnerSlot::new(roles());
    let mut lease = slot.lease().unwrap();
    let dtm = lease.dtm_resources.begin_session_epoch().unwrap();
    rejected(&mut lease, ControllerRoleRetirementError::Dtm);
    lease
        .dtm_resources
        .restore_idle(dtm.cancel())
        .unwrap_or_else(|_| panic!("same DTM"));
    let scan = lease.passive_scan_resources.begin_event().unwrap();
    rejected(&mut lease, ControllerRoleRetirementError::Scanning);
    lease
        .passive_scan_resources
        .restore_idle(scan)
        .unwrap_or_else(|_| panic!("same scanner"));
    let peripheral = lease.peripheral_connection_resources.begin_event().unwrap();
    rejected(
        &mut lease,
        ControllerRoleRetirementError::PeripheralConnection,
    );
    lease
        .peripheral_connection_resources
        .restore_idle(peripheral)
        .unwrap_or_else(|_| panic!("same peripheral"));
    assert_eq!(lease.retirement_ready(), Ok(()));
}

#[test]
fn transport_rejection_preserves_all_roles_and_success_returns_their_original_owners() {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerCommandReadyClaim,
        LeControllerHciResources, LeControllerHciRetirementError,
    };
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
        27,
        1,
    )
    .unwrap();
    let mut resources = LeControllerHciResources::<NoopRawMutex, 2, 2, 80>::new(config).unwrap();
    let mut hci = resources.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        hci.controller.claim_initial_command_ready(())
    else {
        panic!("initial authority");
    };
    let (mut retired, proof) = {
        let mut slot = RuntimeOwnerSlot::new(roles());
        let result = {
            let mut lease = slot.lease().unwrap();
            block_on(hci.host.acl_credit_sender().return_completed_packets(&[])).unwrap();
            let (error, ready) = lease
                .try_retire_roles(ready, |ready| hci.controller.try_retire_transport(ready))
                .err()
                .expect("accepted credits must drain");
            assert!(matches!(
                error,
                RoleBarrierError::Barrier(LeControllerHciRetirementError::HostPacketsPending)
            ));
            assert_eq!(lease.retirement_ready(), Ok(()));
            let mut buffer = [0; 80];
            let oer_bluetooth_hci::LeControllerActivePeripheralIntake::HostCompletedPackets {
                ready,
                command,
                ..
            } = hci.controller.try_receive_active_peripheral_with_buffer(
                ready,
                None,
                &mut buffer,
                |_, _| panic!("credits"),
            )
            else {
                panic!("preserved credit command");
            };
            assert!(command.is_ok());
            lease
                .try_retire_roles(ready, |ready| hci.controller.try_retire_transport(ready))
                .unwrap_or_else(|_| panic!("drained original epoch"))
        };
        assert!(slot.lease().is_none());
        result
    };
    assert!(proof.matches_endpoint(&hci.controller));
    assert_eq!(retired.retirement_ready(), Ok(()));
    // A real allocation can leave the returned group and restore to it after
    // boot storage is gone; identity and RX topology checks remain enforced.
    let allocation = retired
        .peripheral_connection_resources
        .begin_event()
        .unwrap();
    retired
        .peripheral_connection_resources
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("original graph and RX identities"));
    assert_eq!(retired.retirement_ready(), Ok(()));
    assert!(block_on(hci.host.acl_credit_sender().return_completed_packets(&[])).is_err());
}

#[test]
fn advertising_generations_and_shared_peripheral_allocation_survive_rejected_retirement() {
    use oer_bluetooth_ll::{
        LeDeviceAddress, LeDeviceAddressKind,
        advertising::{
            AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
            LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannelMap,
        },
        connectable_advertising::{
            LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
            LegacyConnectableAdvertisingSet, LegacyScanResponseData,
        },
    };
    let address =
        LeDeviceAddress::from_wire_bytes([2, 3, 5, 7, 11, 13], LeDeviceAddressKind::Public);
    let interval = AdvertisingInterval::new(32).unwrap();
    let set = LegacyNonconnectableAdvertisingSet::new(
        LegacyNonconnectableAdvertisement::new(
            address,
            LegacyAdvertisingData::new(&[2, 1, 6]).unwrap(),
        ),
        PrimaryAdvertisingChannelMap::all(),
        interval,
    );
    let mut slot = RuntimeOwnerSlot::new(roles());
    let mut lease = slot.lease().unwrap();
    let event = lease.legacy_advertising_resources.begin_event(set).unwrap();
    let (prepared, _) = event.into_parts();
    let generation = prepared.identity().generation().get();
    rejected(&mut lease, ControllerRoleRetirementError::Advertising);
    assert!(matches!(
        lease
            .legacy_advertising_resources
            .restore_cancelled(prepared.cancel()),
        crate::le::advertising::legacy::LegacyAdvertisingCancelledRestoreOutcome::Restored
    ));
    let definition = crate::le::advertising::connectable::refine_portable_set(
        LegacyConnectableAdvertisingSet::new(
            LegacyConnectableAdvertisement::new(
                address,
                LegacyAdvertisingData::new_owned(&[2, 1, 6]).unwrap(),
                LeChannelSelectionAlgorithmTwoSupport::Supported,
            ),
            LegacyScanResponseData::new_owned(&[]).unwrap(),
            PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
            interval,
        ),
    )
    .unwrap();
    let owners = &mut *lease;
    let prepared = owners
        .legacy_connectable_advertising_resources
        .begin_event(definition, &mut owners.peripheral_connection_resources)
        .unwrap_or_else(|_| panic!("paired connectable owners"));
    rejected(
        &mut lease,
        ControllerRoleRetirementError::ConnectableAdvertising,
    );
    let cancelled = prepared.cancel().unwrap_or_else(|_| panic!("same RX pool"));
    let owners = &mut *lease;
    let restored = owners
        .legacy_connectable_advertising_resources
        .restore_cancelled(cancelled, &mut owners.peripheral_connection_resources)
        .unwrap_or_else(|_| panic!("same paired owners"));
    assert_eq!(restored, definition);
    let (mut owners, ()) = lease
        .try_retire_roles((), |()| Ok::<_, ((), ())>(()))
        .unwrap_or_else(|_| panic!("all roles returned"));
    assert_eq!(owners.retirement_ready(), Ok(()));
    let next = owners
        .legacy_advertising_resources
        .begin_event(set)
        .unwrap();
    assert_eq!(
        next.into_parts().0.identity().generation().get(),
        generation + 1
    );
}
