//! Atomic handoff from prepared connected owners to the Embassy radio runner.
//!
//! Board applications and HIL select storage, sinks and observation policy,
//! but they must not independently choose the order in which the production
//! connected drivers and network runner are joined.  This transaction keeps
//! the network owner beside every failed driver handoff and exposes one
//! statically dispatched service-decoration hook for qualification faults.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};

use crate::{
    datapath::{DatapathRunner, DatapathServices, PinnedTxFrame},
    roles::station::connected::port::{
        Esp32s31ConnectedStaCompositionFailure, Esp32s31ConnectedStaControlResources,
        Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPort, Esp32s31ConnectedStaReport,
        Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxHandoffFailure,
        Esp32s31ConnectedStaTxResources,
    },
    roles::station::control::Esp32s31ConnectedControl,
    roles::station::rx_protocol::{ConnectedRxProtocolSink, Esp32s31ConnectedRxProtocol},
    roles::station::tx::Esp32s31ConnectedTx,
};

/// Complete running frontier returned by connected driver assembly.
pub struct Esp32s31ConnectedDriverAssembly<R> {
    pub runner: R,
    pub report: Esp32s31ConnectedStaReport,
}

/// Failed driver assembly retaining the persistent network owner and the
/// caller's not-yet-applied service decoration.
pub struct Esp32s31ConnectedDriverAssemblyFailure<N, C, F> {
    pub network: N,
    pub composition: C,
    pub map_services: F,
}

/// Coherent owner set consumed by one connected-driver assembly transaction.
///
/// Named ownership domains keep application and HIL call sites independent of
/// a positional argument sequence while the type system still proves the
/// exact network, IRQ, protocol and TX geometries.
pub struct Esp32s31ConnectedDriverAssemblyResources<'irq, M: RawMutex, N, H, R, P, T, C, F> {
    pub plan: Esp32s31ConnectedStaPlan,
    pub irq: &'irq crate::datapath::irq::EmbassyMacIrqRuntime<M>,
    pub network: N,
    pub hardware: H,
    pub rx: R,
    pub protocol: P,
    pub tx: T,
    pub control: C,
    pub map_services: F,
}

/// Compose the connected MAC graph and join it to the persistent network
/// owner in one owner-preserving transaction.
#[allow(clippy::type_complexity, clippy::result_large_err)]
#[inline]
pub fn assemble_esp32s31_connected_driver<
    'slot,
    'resources,
    'queue,
    'pool,
    'scratch,
    'irq,
    'control,
    M,
    S,
    P,
    E,
    T,
    H,
    R,
    B,
    F,
    N,
    const RX_DEPTH: usize,
    const RX_CAPACITY: usize,
    const RX_SLOTS: usize,
    const REORDER_SLOTS: usize,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const AGGREGATE_SLOTS: usize,
    const AGGREGATE_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
    const CONTROL_CAPACITY: usize,
>(
    resources: Esp32s31ConnectedDriverAssemblyResources<
        'irq,
        M,
        N,
        H,
        R,
        Esp32s31ConnectedStaRxProtocolResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            RX_DEPTH,
            RX_CAPACITY,
            RX_SLOTS,
            REORDER_SLOTS,
        >,
        Esp32s31ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Esp32s31ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
        F,
    >,
) -> Result<
    Esp32s31ConnectedDriverAssembly<
        DatapathRunner<
            'resources,
            'irq,
            M,
            N,
            B,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
            N::RxPublisher,
        >,
    >,
    Esp32s31ConnectedDriverAssemblyFailure<
        N,
        Esp32s31ConnectedStaCompositionFailure<
            H,
            R,
            Esp32s31ConnectedStaRxProtocolResources<
                'queue,
                'pool,
                'scratch,
                'irq,
                M,
                S,
                RX_DEPTH,
                RX_CAPACITY,
                RX_SLOTS,
                REORDER_SLOTS,
            >,
            Esp32s31ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
            Esp32s31ConnectedStaTxHandoffFailure<
                'slot,
                'resources,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
                P,
                E,
                T,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
        >,
        F,
    >,
>
where
    M: RawMutex,
    N: crate::datapath::network::DatapathNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    S: ConnectedRxProtocolSink<RX_CAPACITY, RX_SLOTS>,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: open_esp_radio_esp32s31_wifi_mac::init::StaEspNowRxPolicyHardware,
    F: FnOnce(
        crate::datapath::services::SingleRoleServices<
            H,
            crate::roles::station::connected::Esp32s31ConnectedStaRxService<
                R,
                Esp32s31ConnectedRxProtocol<
                    'queue,
                    'pool,
                    'scratch,
                    'irq,
                    M,
                    S,
                    RX_DEPTH,
                    RX_CAPACITY,
                    RX_SLOTS,
                    REORDER_SLOTS,
                >,
            >,
            Esp32s31ConnectedTx<
                'slot,
                'resources,
                'resources,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                TX_QUEUE_DEPTH,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Esp32s31ConnectedControl<'control, M, CONTROL_CAPACITY>,
        >,
    ) -> B,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
{
    let Esp32s31ConnectedDriverAssemblyResources {
        plan,
        irq,
        network,
        hardware,
        rx,
        protocol,
        tx,
        control,
        map_services,
    } = resources;
    let drivers = match Esp32s31ConnectedStaPort::compose(plan, hardware, rx, protocol, tx, control)
    {
        Ok(drivers) => drivers,
        Err(composition) => {
            return Err(Esp32s31ConnectedDriverAssemblyFailure {
                network,
                composition,
                map_services,
            });
        }
    };
    let services = map_services(drivers.services);
    Ok(Esp32s31ConnectedDriverAssembly {
        runner: DatapathRunner::new(
            irq,
            network,
            crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
            services,
        ),
        report: drivers.report,
    })
}
