use embassy_net::{
    Stack,
    tcp::{TcpListener, TcpSocket},
    udp::UdpSocket,
};
use static_cell::StaticCell;

pub const ECHO_PORT: u16 = 7;

static UDP_PACKET: StaticCell<[u8; 1472]> = StaticCell::new();
// TCP windows are static because frame-sized arrays inside this eternal task
// would unnecessarily enlarge the executor future.
static TCP_RX: StaticCell<[u8; 4096]> = StaticCell::new();
static TCP_TX: StaticCell<[u8; 4096]> = StaticCell::new();
static TCP_PACKET: StaticCell<[u8; 1460]> = StaticCell::new();

pub async fn udp_echo(stack: Stack<'static>) -> ! {
    let mut socket = UdpSocket::new(stack);
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
    let mut listener = TcpListener::new(stack);
    listener
        .listen(ECHO_PORT)
        .expect("TCP echo port must be free");
    loop {
        socket.abort();
        if listener.accept(&mut socket).await.is_err() {
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
