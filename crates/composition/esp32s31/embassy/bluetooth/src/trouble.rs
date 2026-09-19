//! Affine composition of a Trouble Host stack and its Controller runner.

use trouble_host::{PacketPool, Stack};

#[cfg(any(test, target_arch = "riscv32"))]
use trouble_host::{Controller, HostResources};

/// One Trouble Host stack paired with the hardware owner for the same
/// Controller epoch.
///
/// Applications obtain the Host runner and peripheral/GATT handles from
/// [`stack`](Self::stack), and must concurrently poll the executor-side
/// [`hardware`](Self::hardware) owner supplied by the platform composition.
#[must_use = "the Trouble Host stack and hardware runner must both be retained and polled"]
pub struct BluetoothTroubleSystem<'resources, C, P: PacketPool, H> {
    /// Pinned Trouble Host stack over the direct typed HCI boundary.
    pub stack: Stack<'resources, C, P>,
    /// Executor-side owner for the exact Controller epoch used by `stack`.
    pub hardware: H,
}

/// Consume one Controller facade and its hardware owner into a Trouble Host
/// composition.
///
/// The caller owns the bounded Host storage and chooses its connection, L2CAP
/// channel, advertising-set and bond capacities. The returned value keeps the
/// Controller-facing and hardware-facing halves together without adding an
/// intermediate transport or allocating memory.
#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) fn compose_trouble_bluetooth_system<
    'resources,
    C: Controller,
    P: PacketPool,
    H,
    const CONNECTIONS: usize,
    const L2CAP_CHANNELS: usize,
    const ADVERTISING_SETS: usize,
    const BONDS: usize,
>(
    controller: C,
    hardware: H,
    resources: &'resources mut HostResources<
        P,
        CONNECTIONS,
        L2CAP_CHANNELS,
        ADVERTISING_SETS,
        BONDS,
    >,
) -> BluetoothTroubleSystem<'resources, C, P, H> {
    BluetoothTroubleSystem {
        stack: trouble_host::new(controller, resources).build(),
        hardware,
    }
}

#[cfg(test)]
mod tests {
    use core::{
        convert::Infallible,
        fmt,
        future::pending,
        sync::atomic::{AtomicU8, Ordering},
    };

    use bt_hci::{
        ReadHciError,
        controller::ExternalController,
        transport::{PacketToController, PacketToHost, Transport},
    };
    use embedded_io::{Error, ErrorKind, ErrorType};
    use trouble_host::{HostResources, Packet, PacketPool};

    use super::compose_trouble_bluetooth_system;

    static DROPS: AtomicU8 = AtomicU8::new(0);

    #[derive(Debug)]
    struct TestError;

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test transport error")
        }
    }

    impl core::error::Error for TestError {}

    impl Error for TestError {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    impl From<ReadHciError<Infallible>> for TestError {
        fn from(_error: ReadHciError<Infallible>) -> Self {
            Self
        }
    }

    struct TestPacket([u8; 27]);

    impl AsRef<[u8]> for TestPacket {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    impl AsMut<[u8]> for TestPacket {
        fn as_mut(&mut self) -> &mut [u8] {
            &mut self.0
        }
    }

    impl Packet for TestPacket {}

    struct TestPacketPool;

    impl PacketPool for TestPacketPool {
        type Packet = TestPacket;

        const MTU: usize = 27;

        fn allocate() -> Option<Self::Packet> {
            Some(TestPacket([0; 27]))
        }

        fn capacity() -> usize {
            1
        }
    }

    struct TestTransport;

    impl Drop for TestTransport {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl ErrorType for TestTransport {
        type Error = TestError;
    }

    impl Transport for TestTransport {
        async fn read<'a, P: PacketToHost<'a>>(&self, _rx: &'a mut [u8]) -> Result<P, Self::Error> {
            pending().await
        }

        async fn write<P: PacketToController>(&self, _tx: &P) -> Result<(), Self::Error> {
            pending().await
        }
    }

    struct TestHardware;

    impl Drop for TestHardware {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn composition_retains_both_halves_of_one_host_controller_epoch() {
        DROPS.store(0, Ordering::Relaxed);
        let mut resources = HostResources::<TestPacketPool, 1, 1>::new();

        {
            let controller = ExternalController::<_, 1>::new(TestTransport);
            let system = compose_trouble_bluetooth_system(controller, TestHardware, &mut resources);
            assert_eq!(DROPS.load(Ordering::Relaxed), 0);

            let _runner = system.stack.runner();
            let _peripheral = system.stack.peripheral();
        }

        assert_eq!(DROPS.load(Ordering::Relaxed), 2);
    }
}
