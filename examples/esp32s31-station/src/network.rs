//! Application-owned stack setup and one UDP echo workload across contracts.
#[cfg(feature = "upstream-network")]
mod upstream;
#[cfg(feature = "upstream-network")]
pub use upstream::run;
#[cfg(not(feature = "upstream-network"))]
mod embassy;
#[cfg(not(feature = "upstream-network"))]
pub use embassy::run;

use crate::embassy_net::{Stack, udp::UdpSocket};
use static_cell::StaticCell;

async fn echo(stack: Stack<'static>) -> ! {
    #[cfg(feature = "upstream-network")]
    let mut udp = UdpSocket::new(stack).expect("one UDP socket fits");
    #[cfg(feature = "owned-network")]
    let mut udp = UdpSocket::new(stack);
    #[cfg(feature = "compat-network")]
    let mut udp = {
        use crate::embassy_net::udp::PacketMetadata;
        static RX_META: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
        static TX_META: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
        static RX: StaticCell<[u8; 4 * 1472]> = StaticCell::new();
        static TX: StaticCell<[u8; 4 * 1472]> = StaticCell::new();
        UdpSocket::new(
            stack,
            RX_META.init([PacketMetadata::EMPTY; 4]),
            RX.init_with(|| [0; 4 * 1472]),
            TX_META.init([PacketMetadata::EMPTY; 4]),
            TX.init_with(|| [0; 4 * 1472]),
        )
    };
    static PAYLOAD: StaticCell<[u8; 1472]> = StaticCell::new();
    let payload = PAYLOAD.init_with(|| [0; 1472]);
    udp.bind(4321).expect("UDP echo port is available");
    loop {
        match udp.recv_from(payload).await {
            Ok((length, metadata)) => {
                if let Err(error) = udp.send_to(&payload[..length], metadata.endpoint).await {
                    esp_println::println!("open-radio: UDP echo send failed: {:?}", error);
                }
            }
            Err(error) => esp_println::println!("open-radio: UDP echo receive failed: {:?}", error),
        }
    }
}
