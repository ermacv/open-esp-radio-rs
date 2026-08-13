use embassy_net::{
    Stack,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use static_cell::StaticCell;

pub const ECHO_PORT: u16 = 7;

static UDP_RX_METADATA: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static UDP_TX_METADATA: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static UDP_RX: StaticCell<[u8; 3072]> = StaticCell::new();
static UDP_TX: StaticCell<[u8; 3072]> = StaticCell::new();
static UDP_PACKET: StaticCell<[u8; 1472]> = StaticCell::new();
// TCP windows are static because frame-sized arrays inside this eternal task
// would unnecessarily enlarge the executor future.
static TCP_RX: StaticCell<[u8; 4096]> = StaticCell::new();
static TCP_TX: StaticCell<[u8; 4096]> = StaticCell::new();
static TCP_PACKET: StaticCell<[u8; 1460]> = StaticCell::new();

pub async fn udp_echo(stack: Stack<'static>) -> ! {
    let mut socket = UdpSocket::new(
        stack,
        UDP_RX_METADATA.init_with(|| [PacketMetadata::EMPTY; 4]),
        UDP_RX.init_with(|| [0; 3072]),
        UDP_TX_METADATA.init_with(|| [PacketMetadata::EMPTY; 4]),
        UDP_TX.init_with(|| [0; 3072]),
    );
    socket.bind(ECHO_PORT).expect("UDP echo port must be free");
    let packet = UDP_PACKET.init_with(|| [0; 1472]);
    loop {
        let Ok((length, remote)) = socket.recv_from(packet).await else {
            continue;
        };
        let _ = socket.send_to(&packet[..length], remote).await;
    }
}

pub async fn tcp_echo(stack: Stack<'static>) -> ! {
    let rx = TCP_RX.init_with(|| [0; 4096]);
    let tx = TCP_TX.init_with(|| [0; 4096]);
    let packet = TCP_PACKET.init_with(|| [0; 1460]);
    let mut socket = TcpSocket::new(stack, rx, tx);
    loop {
        socket.abort();
        if socket.accept(ECHO_PORT).await.is_err() {
            continue;
        }
        loop {
            match socket.read(packet).await {
                Ok(0) | Err(_) => break,
                Ok(length) => {
                    let mut written = 0;
                    while written < length {
                        match socket.write(&packet[written..length]).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => written += count,
                        }
                    }
                    if written != length {
                        break;
                    }
                }
            }
        }
    }
}
