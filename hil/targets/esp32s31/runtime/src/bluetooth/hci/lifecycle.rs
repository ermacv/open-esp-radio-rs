//! Diagnostic Controller epoch restart and retirement.
//!
//! One owner keeps the Bluetooth system and its HCI service and runs them
//! until the HCI task hands over its Host end. The HCI task first resets the
//! Controller through that end; the owner then retires the drained Host end,
//! stops the Controller on the shared radio, checks that both directions of
//! the old end report the transport closed and, for a restart, starts the
//! Controller again on the same storage and returns a fresh Host end. A
//! retired Controller, or one whose old end stayed open, is kept stopped
//! and never resumed.

use core::{convert::Infallible, future::pending, pin::pin};

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::with_timeout;
use embedded_io::{Error as _, ErrorKind};
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress,
    bt_hci::{ControllerToHostPacket, transport::Transport},
};
use oer_esp32s31_bluetooth_system::{
    BluetoothHciService, BluetoothHostTransport, BluetoothSystem, start,
};
use oer_hil_protocol::{
    BluetoothHciLifecycle, BluetoothHciLifecycleEvidence, BluetoothHciResponse,
};

use super::{Host, OPERATION_TIMEOUT, QUEUE, RESET, RawCommand};
use crate::bluetooth::Radio;

static HANDOFFS: Channel<
    CriticalSectionRawMutex,
    (BluetoothHciLifecycle, BluetoothHostTransport),
    1,
> = Channel::new();
static OUTCOMES: Channel<
    CriticalSectionRawMutex,
    (
        BluetoothHciLifecycleEvidence,
        Option<BluetoothHostTransport>,
    ),
    1,
> = Channel::new();

/// Run the Controller and its HCI service, ending one epoch per handover.
pub(super) async fn run(
    radio: &'static Radio,
    mut system: BluetoothSystem,
    mut service: BluetoothHciService,
    public_address: BluetoothPublicDeviceAddress,
) -> Infallible {
    loop {
        let (operation, old) = {
            let hardware = pin!(select(system.run(radio), service.run()));
            match select(hardware, HANDOFFS.receive()).await {
                Either::First(Either::First(_)) => {
                    crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-runner-fault\r\n")
                }
                Either::First(Either::Second(_)) => {
                    crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-service\r\n")
                }
                Either::Second(handoff) => handoff,
            }
        };
        // Retire the old Host end once its undelivered packets are drained.
        match select(service.wait_retirement_ready(), drain(&old)).await {
            Either::First(Ok(())) => {}
            Either::First(Err(_)) => {
                crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-retirement\r\n")
            }
            Either::Second(never) => match never {},
        }
        if service.retire().is_err() {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-retirement\r\n");
        }
        let parked = {
            let mut stop = pin!(system.stop(radio));
            match stop.as_mut().await {
                Ok(parked) => parked,
                Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-stop\r\n"),
            }
        };
        let old_host_closed = closed(&old).await;
        if operation == BluetoothHciLifecycle::Retire || !old_host_closed {
            let evidence = BluetoothHciLifecycleEvidence {
                old_host_closed,
                restarted: false,
            };
            OUTCOMES.send((evidence, None)).await;
            retain((old, parked, service)).await;
        }
        system = {
            let mut restart = pin!(start(radio, parked, public_address));
            match restart.as_mut().await {
                Ok(system) => system,
                Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-restart\r\n"),
            }
        };
        let fresh = restart(&mut service);
        drop(old);
        let evidence = BluetoothHciLifecycleEvidence {
            old_host_closed,
            restarted: true,
        };
        OUTCOMES.send((evidence, Some(fresh))).await;
    }
}

/// End the current epoch on behalf of the HCI task: reset the Controller
/// through `host`, hand the Host end over and take back the next one, if
/// any. A failed Reset keeps the epoch running.
pub(super) async fn end_epoch(
    host: &mut Option<Host>,
    operation: BluetoothHciLifecycle,
) -> BluetoothHciResponse {
    let Some(current) = host.take() else {
        return BluetoothHciResponse::TransportFailed;
    };
    match with_timeout(OPERATION_TIMEOUT, current.command(RESET, &[])).await {
        // Command Complete: code, length, packets, opcode, status.
        Ok(Some(packet)) if packet.get(5) == Some(&0) => {}
        Ok(Some(packet)) => {
            *host = Some(current);
            return BluetoothHciResponse::Completed(packet);
        }
        Ok(None) => {
            *host = Some(current);
            return BluetoothHciResponse::TransportFailed;
        }
        Err(_) => {
            *host = Some(current);
            return BluetoothHciResponse::Timeout;
        }
    }
    // Packets of the ended epoch are never returned.
    QUEUE.lock(|queue| {
        let mut queue = queue.borrow_mut();
        queue.packets.clear();
        queue.dropped = 0;
    });
    HANDOFFS.send((operation, current.transport)).await;
    let (evidence, fresh) = OUTCOMES.receive().await;
    *host = fresh.map(|transport| Host { transport });
    BluetoothHciResponse::Lifecycle(evidence)
}

fn restart(service: &mut BluetoothHciService) -> BluetoothHostTransport {
    match service.restart() {
        Ok(transport) => transport,
        Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-restart\r\n"),
    }
}

/// Read and discard what the old Host end still receives; ends only when the
/// transport closes.
async fn drain(transport: &BluetoothHostTransport) -> Infallible {
    let mut buffer = [0; oer_esp32s31_bluetooth_system::PACKET];
    loop {
        if transport
            .read::<ControllerToHostPacket>(&mut buffer)
            .await
            .is_err()
        {
            return pending().await;
        }
    }
}

/// Whether both directions of the retired Host end report the transport
/// closed.
async fn closed(transport: &BluetoothHostTransport) -> bool {
    let commands = matches!(
        transport.write(&RawCommand { opcode: RESET, parameters: &[] }).await,
        Err(error) if error.kind() == ErrorKind::BrokenPipe
    );
    let mut buffer = [0; oer_esp32s31_bluetooth_system::PACKET];
    let events = matches!(
        transport.read::<ControllerToHostPacket>(&mut buffer).await,
        Err(error) if error.kind() == ErrorKind::BrokenPipe
    );
    commands && events
}

/// Keep terminal owners alive and never resume them.
async fn retain<T>(owners: T) -> ! {
    pending::<()>().await;
    drop(owners);
    unreachable!("terminal owners are never resumed")
}
