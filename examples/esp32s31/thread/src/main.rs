#![no_main]
#![no_std]

//! A Thread end device on the ESP32-S31 IEEE 802.15.4 radio.
//!
//! The IEEE 802.15.4 client starts on the shared radio system, the radio
//! system runs the periodic PHY tracking, and OpenThread runs over the
//! client's runtime through the OpenThread radio adapter. The device joins
//! the network of the active dataset as a minimal end device and echoes UDP
//! datagrams on port 1212.

use core::net::{Ipv6Addr, SocketAddrV6};
use core::pin::pin;

use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse,
    interrupt::software::SoftwareInterrupt,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};
use log::{error, info};
use oer::systems::esp32s31::embassy::ieee802154::{
    EspHalRadioPlatform, IEEE802154_EVENT_CAPACITY, Ieee802154MacOwners, Ieee802154Parked,
    Ieee802154PibDefaults, Ieee802154System,
    openthread::{
        OPEN_THREAD_RADIO_CAPABILITIES, OpenThreadRadio, OpenThreadRadioDefaults,
        frames::role_coex_config,
    },
    start,
};
use oer::systems::esp32s31::embassy::radio::{self as shared_radio, RadioStart, SharedRadio};
use oer_esp32s31_executor_embassy::{self as platform_executor, Executor};
use openthread::{OpenThread, OtResources, OtUdpResources, SimpleRamSettings, UdpSocket};
use static_cell::{ConstStaticCell, StaticCell};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use tinyrlibc as _;

/// Frames OpenThread has not taken yet while it transmits or scans.
const RX_QUEUE: usize = 8;
type ThreadRadio = OpenThreadRadio<
    'static,
    'static,
    CriticalSectionRawMutex,
    Ieee802154MacOwners,
    IEEE802154_EVENT_CAPACITY,
    RX_QUEUE,
>;

const UDP_PORT: u16 = 1212;
const UDP_BUFFER: usize = 1280;
const UDP_SOCKETS: usize = 2;

/// The CSL period in microseconds, from the build environment: with one,
/// the device runs as a synchronized sleepy end device that samples its
/// parent's channel in CSL windows; without one, as a minimal end device.
const THREAD_CSL_PERIOD_US: Option<&str> = option_env!("THREAD_CSL_PERIOD_US");

/// The active operational dataset as TLV hex, from the build environment.
const THREAD_DATASET: &str = match option_env!("THREAD_DATASET") {
    Some(dataset) => dataset,
    None => "",
};

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
// The entropy source must outlive every `Trng` OpenThread draws from.
static TRNG_SOURCE: StaticCell<TrngSource<'static>> = StaticCell::new();
static TRNG: StaticCell<Trng> = StaticCell::new();
static SYSTEM: StaticCell<Ieee802154System> = StaticCell::new();
static OT_RESOURCES: StaticCell<OtResources> = StaticCell::new();
static OT_UDP: StaticCell<OtUdpResources<UDP_SOCKETS, UDP_BUFFER>> = StaticCell::new();
static OT_SETTINGS_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
static OT_SETTINGS: StaticCell<SimpleRamSettings> = StaticCell::new();
static UDP_RECEIVE: ConstStaticCell<[u8; UDP_BUFFER]> = ConstStaticCell::new([0; UDP_BUFFER]);

/// The IEEE 802.15.4 EUI-64 as ESP-IDF derives it
/// (`esp_read_mac(ESP_MAC_IEEE802154)`): the base MAC's first three bytes,
/// the eFuse MAC extension, then the base MAC's last three bytes.
fn ieee_eui64() -> [u8; 8] {
    let base = efuse::base_mac_address();
    let base = base.as_bytes();
    let extension = efuse::read_field_le::<u16>(efuse::MAC_EXT).to_le_bytes();
    [
        base[0],
        base[1],
        base[2],
        extension[0],
        extension[1],
        base[3],
        base[4],
        base[5],
    ]
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // SAFETY: the common stage-two entry runs after the board bootstrap,
    // with global interrupts disabled and the PSRAM mapping intact.
    let _psram = unsafe { oer_esp32s31_platform_runtime::adopt_psram(peripherals.PSRAM) };

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    platform_executor::init(OneShotTimer::new(timer_group.timer0));
    TRNG_SOURCE.init(TrngSource::new(peripherals.RNG));
    let trng = Trng::try_new().expect("ESP32-S31 TRNG must have a unique owner");
    let platform = EspHalRadioPlatform::new(
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    // SAFETY: timer and executor handlers are now bound on CPU0, and the staged
    // handoff has kept MIE clear since `adopt_psram`.
    unsafe { oer_esp32s31_platform_runtime::enable_interrupts_after_handoff() };
    executor.run(|spawner| {
        spawner.spawn(
            thread_task(spawner, platform, trng)
                .expect("thread task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn thread_task(
    spawner: embassy_executor::Spawner,
    platform: EspHalRadioPlatform,
    trng: Trng,
) {
    let (radio, partitions) = shared_radio::start(spawner, platform, RadioStart::new())
        .expect("the shared radio starts once");
    let defaults = Ieee802154PibDefaults::default();
    let parked = Ieee802154Parked::new(partitions.ieee802154, defaults)
        .expect("the IEEE 802.15.4 engine frames are taken once");
    // Bring-up registers and calibrates the shared PHY; pin its future.
    let started = {
        let started = pin!(start(radio, parked, defaults));
        started.await
    };
    let Ok(system) = started else {
        error!("the IEEE 802.15.4 client did not start");
        return;
    };
    let system = SYSTEM.init(system);

    let ot_settings = OT_SETTINGS.init(SimpleRamSettings::new(OT_SETTINGS_BUFFER.take()));
    // OpenThread's `SubMac` reads the radio capabilities when the instance
    // is built: transmit security then stays with the radio.
    let ot_resources = OT_RESOURCES.init(OtResources::new());
    ot_resources.set_radio_caps(OPEN_THREAD_RADIO_CAPABILITIES);
    let ot = OpenThread::new_with_udp(
        ieee_eui64(),
        TRNG.init(trng),
        ot_settings,
        ot_resources,
        OT_UDP.init(OtUdpResources::new()),
    )
    .expect("OpenThread must initialize once");
    let thread_radio = OpenThreadRadio::new(
        system.runtime(),
        system.recent_rssi_reader(),
        OpenThreadRadioDefaults::ESP_IDF,
    );
    spawner.spawn(openthread_task(ot.clone(), thread_radio).expect("OpenThread task storage"));
    spawner.spawn(role_task(ot.clone(), system, radio).expect("role task storage"));

    if THREAD_DATASET.is_empty() {
        error!("build with THREAD_DATASET set to the active operational dataset TLV hex");
        return;
    }
    ot.set_active_dataset_tlv_hexstr(THREAD_DATASET)
        .expect("THREAD_DATASET must be a valid dataset");
    // An end device without the router role, with stable network data only:
    // minimal (receiver on when idle), or synchronized sleepy with a CSL
    // period, sampling in the windows its radio schedules.
    let csl_period = THREAD_CSL_PERIOD_US.map(|period| {
        period
            .parse::<u32>()
            .expect("THREAD_CSL_PERIOD_US must be a period in microseconds")
    });
    ot.set_link_mode(csl_period.is_none(), false, false)
        .expect("the MTD link mode must be accepted");
    if let Some(period) = csl_period {
        ot.set_csl_period(period)
            .expect("THREAD_CSL_PERIOD_US must be a valid CSL period");
        info!("CSL period {period} us");
    }
    ot.enable_ipv6(true).expect("IPv6 must come up");
    ot.enable_thread(true).expect("Thread must start");

    let socket = UdpSocket::bind(
        ot,
        &SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, UDP_PORT, 0, 0),
    )
    .expect("the UDP port must be free");
    info!("echoing UDP on port {UDP_PORT}");
    let buffer = UDP_RECEIVE.take();
    loop {
        let Ok((length, local, remote)) = socket.recv(buffer).await else {
            continue;
        };
        info!("{length} bytes from {remote}");
        if socket
            .send(&buffer[..length], Some(&local), &remote)
            .await
            .is_err()
        {
            error!("the echo to {remote} failed");
        }
    }
}

#[embassy_executor::task]
async fn openthread_task(ot: OpenThread<'static>, radio: ThreadRadio) -> ! {
    // The stack's radio, alarm and tasklet loops share one large future.
    pin!(ot.run(radio)).await
}

/// Log the device's role and addresses when OpenThread's state changes, and
/// set the IEEE 802.15.4 coexistence level of a role change as ESP-IDF's
/// `handle_ot_role_change` does.
#[embassy_executor::task]
async fn role_task(
    ot: OpenThread<'static>,
    system: &'static mut Ieee802154System,
    radio: &'static SharedRadio,
) -> ! {
    let mut role = None;
    loop {
        ot.wait_changed().await;
        let current = ot.device_role();
        if role != Some(current) {
            role = Some(current);
            let config = role_coex_config(system.coex_config(), ot.rx_on_when_idle());
            if system.update_coexistence(radio, config).await.is_err() {
                error!("the coexistence level of role {current:?} was not applied");
            }
        }
        info!("role {:?}, rloc16 {:#06x}", current, ot.rloc16());
        let _ = ot.ipv6_addrs(|address| {
            if let Some((address, prefix)) = address {
                info!("address {address}/{prefix}");
            }
            Ok(())
        });
    }
}
