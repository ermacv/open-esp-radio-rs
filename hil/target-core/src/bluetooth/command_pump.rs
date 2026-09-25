//! Finite HIL Host commands issued from the event-pump task.
//!
//! `ExternalController::exec` needs `Controller::read` to dispatch its reply.
//! Keep that reader live while the event-pump task itself waits for a command;
//! forward unrelated packets to the same observer instead of discarding them.

use bt_hci::{ControllerToHostPacket, controller::Controller};
use core::{future::Future, pin::pin};
use embassy_futures::select::{Either, select};

pub async fn with_event_pump<C: Controller, F: Future>(
    controller: &C,
    buffer: &mut C::Buffer<'_>,
    command: F,
    mut observe: impl FnMut(ControllerToHostPacket<'_>),
) -> Result<F::Output, C::Error> {
    let mut command = pin!(command);
    loop {
        match select(command.as_mut(), controller.read(buffer)).await {
            Either::First(result) => return Ok(result),
            Either::Second(packet) => observe(packet?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::{
        PacketKind, ReadHciError,
        cmd::{SyncCmd, controller_baseband::Reset, link_control::Disconnect},
        controller::ExternalController,
        event::EventKind,
        param::{ConnHandle, DisconnectReason},
        transport::{PacketToController, PacketToHost, Transport},
    };
    use core::{
        convert::Infallible,
        task::{Context, Poll, Waker},
    };
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel};

    struct Wire {
        response: &'static [u8],
        events: Channel<NoopRawMutex, &'static [u8], 2>,
    }

    impl embedded_io::ErrorType for Wire {
        type Error = ReadHciError<Infallible>;
    }

    impl Transport for Wire {
        async fn read<'a, P: PacketToHost<'a>>(&self, rx: &'a mut [u8]) -> Result<P, Self::Error> {
            let bytes = self.events.receive().await;
            P::read_hci(PacketKind::Event, &mut &bytes[..], rx)
        }

        async fn write<P: PacketToController>(&self, _: &P) -> Result<(), Self::Error> {
            assert_eq!(P::KIND, PacketKind::Cmd);
            // A real unrelated disconnection must survive before the reply.
            self.events
                .send(&[0x05, 0x04, 0x00, 0x01, 0x00, 0x16])
                .await;
            self.events.send(self.response).await;
            Ok(())
        }
    }

    fn exercise(response: &'static [u8], disconnect: bool) {
        let controller = ExternalController::<_, 1>::new(Wire {
            response,
            events: Channel::new(),
        });
        let mut buffer = controller.alloc_buf().unwrap();
        let mut observed = 0;
        let mut context = Context::from_waker(Waker::noop());
        let command = async {
            if disconnect {
                Disconnect::new(
                    ConnHandle::new(1),
                    DisconnectReason::RemoteUserTerminatedConn,
                )
                .exec(&controller)
                .await
            } else {
                Reset::new().exec(&controller).await
            }
        };
        let mut command = pin!(command);
        // This is the original deadlock: a queued reply cannot complete exec
        // while the sole Host reader is suspended inside that same exec.
        assert!(command.as_mut().poll(&mut context).is_pending());
        assert!(command.as_mut().poll(&mut context).is_pending());
        {
            let future = with_event_pump(&controller, &mut buffer, command.as_mut(), |packet| {
                let ControllerToHostPacket::Event(event) = packet else {
                    panic!("expected event")
                };
                assert_eq!(event.kind, EventKind::DisconnectionComplete);
                observed += 1;
            });
            let mut future = pin!(future);
            let mut completed = false;
            for _ in 0..4 {
                if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                    result.unwrap().unwrap();
                    completed = true;
                    break;
                }
            }
            assert!(completed, "command and event pump must both progress");
        }
        assert_eq!(observed, 1);
    }

    #[test]
    fn local_disconnect_pumps_command_status_and_retains_disconnection() {
        exercise(&[0x0f, 0x04, 0x00, 0x01, 0x06, 0x04], true);
    }

    #[test]
    fn local_reset_pumps_command_complete_and_retains_disconnection() {
        exercise(&[0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00], false);
    }
}
