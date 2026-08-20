//! One physical WDEV services owner shared by two logical interfaces.

use core::future::{Future, pending};

use open_esp_radio_embassy_net::{
    NetworkInterfaceId, PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex,
};

use super::{
    WdevControlContext, WdevControlProgress, WdevNetworkRx, WdevNetworkRxSet, WdevRxProgress,
    WdevRxServiceContext, WdevServices, WdevStopProgress, WifiTxProgress, WifiTxWake,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairRole {
    First,
    Second,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairedPhysicalTxError {
    AlreadyLent(WdevPairRole),
    NotLent,
    WrongRole {
        expected: WdevPairRole,
        actual: WdevPairRole,
    },
}

enum WdevPairedPhysicalTxState<Ordinary, Aggregate> {
    Available {
        ordinary: Ordinary,
        aggregate: Aggregate,
    },
    Lent(WdevPairRole),
}

/// Sole dynamic owner of the ordinary and aggregate physical TX resources.
///
/// A role may materialize its local protocol state only after taking both
/// resources together. It must return the same pair at the terminal TX edge;
/// another role cannot observe an available capability in the meantime.
pub struct WdevPairedPhysicalTx<Ordinary, Aggregate> {
    state: WdevPairedPhysicalTxState<Ordinary, Aggregate>,
}

enum WdevPairedRoleState<Active, Parked> {
    Active(Active),
    Parked(Parked),
    Transitioning,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairedRoleTransitionError<E> {
    AlreadyActive,
    AlreadyParked,
    Conversion(E),
}

/// Role-local state whose active form temporarily contains physical TX.
///
/// The internal transition sentinel is never observable. A failed conversion
/// must return the exact input state, preventing `Option::take` holes from
/// becoming a second, implicit ownership protocol.
pub struct WdevPairedRoleOwner<Active, Parked> {
    state: WdevPairedRoleState<Active, Parked>,
}

impl<Active, Parked> WdevPairedRoleOwner<Active, Parked> {
    pub const fn from_active(active: Active) -> Self {
        Self {
            state: WdevPairedRoleState::Active(active),
        }
    }

    pub const fn parked(parked: Parked) -> Self {
        Self {
            state: WdevPairedRoleState::Parked(parked),
        }
    }

    pub fn active(&self) -> Option<&Active> {
        match &self.state {
            WdevPairedRoleState::Active(active) => Some(active),
            WdevPairedRoleState::Parked(_) => None,
            WdevPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn active_mut(&mut self) -> Option<&mut Active> {
        match &mut self.state {
            WdevPairedRoleState::Active(active) => Some(active),
            WdevPairedRoleState::Parked(_) => None,
            WdevPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn parked_state(&self) -> Option<&Parked> {
        match &self.state {
            WdevPairedRoleState::Active(_) => None,
            WdevPairedRoleState::Parked(parked) => Some(parked),
            WdevPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn parked_state_mut(&mut self) -> Option<&mut Parked> {
        match &mut self.state {
            WdevPairedRoleState::Active(_) => None,
            WdevPairedRoleState::Parked(parked) => Some(parked),
            WdevPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn is_parked(&self) -> bool {
        matches!(self.state, WdevPairedRoleState::Parked(_))
    }

    pub fn try_activate<E>(
        &mut self,
        activate: impl FnOnce(Parked) -> Result<Active, (E, Parked)>,
    ) -> Result<(), WdevPairedRoleTransitionError<E>> {
        let state = core::mem::replace(&mut self.state, WdevPairedRoleState::Transitioning);
        let WdevPairedRoleState::Parked(parked) = state else {
            self.state = state;
            return Err(WdevPairedRoleTransitionError::AlreadyActive);
        };
        match activate(parked) {
            Ok(active) => {
                self.state = WdevPairedRoleState::Active(active);
                Ok(())
            }
            Err((error, parked)) => {
                self.state = WdevPairedRoleState::Parked(parked);
                Err(WdevPairedRoleTransitionError::Conversion(error))
            }
        }
    }

    pub fn try_park<E>(
        &mut self,
        park: impl FnOnce(Active) -> Result<Parked, (E, Active)>,
    ) -> Result<(), WdevPairedRoleTransitionError<E>> {
        let state = core::mem::replace(&mut self.state, WdevPairedRoleState::Transitioning);
        let WdevPairedRoleState::Active(active) = state else {
            self.state = state;
            return Err(WdevPairedRoleTransitionError::AlreadyParked);
        };
        match park(active) {
            Ok(parked) => {
                self.state = WdevPairedRoleState::Parked(parked);
                Ok(())
            }
            Err((error, active)) => {
                self.state = WdevPairedRoleState::Active(active);
                Err(WdevPairedRoleTransitionError::Conversion(error))
            }
        }
    }

    /// Consume a quiescent role while retaining the complete owner on error.
    ///
    /// Returning only the active payload would discard the role-state
    /// boundary and make a higher-level rollback reconstruct it.  Paired
    /// teardown therefore either receives the parked payload or the exact
    /// original owner.
    pub fn try_into_parked(self) -> Result<Parked, Self> {
        match self.state {
            WdevPairedRoleState::Parked(parked) => Ok(parked),
            WdevPairedRoleState::Active(active) => Err(Self {
                state: WdevPairedRoleState::Active(active),
            }),
            WdevPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    /// Consume an active role while retaining the complete owner when it is
    /// already parked.
    pub fn try_into_active(self) -> Result<Active, Self> {
        match self.state {
            WdevPairedRoleState::Active(active) => Ok(active),
            WdevPairedRoleState::Parked(parked) => Err(Self {
                state: WdevPairedRoleState::Parked(parked),
            }),
            WdevPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }
}

impl<Ordinary, Aggregate> WdevPairedPhysicalTx<Ordinary, Aggregate> {
    pub const fn new(ordinary: Ordinary, aggregate: Aggregate) -> Self {
        Self {
            state: WdevPairedPhysicalTxState::Available {
                ordinary,
                aggregate,
            },
        }
    }

    pub const fn lent_to(&self) -> Option<WdevPairRole> {
        match &self.state {
            WdevPairedPhysicalTxState::Available { .. } => None,
            WdevPairedPhysicalTxState::Lent(role) => Some(*role),
        }
    }

    pub fn try_lend(
        &mut self,
        role: WdevPairRole,
    ) -> Result<(Ordinary, Aggregate), WdevPairedPhysicalTxError> {
        let WdevPairedPhysicalTxState::Available { .. } = &self.state else {
            return Err(WdevPairedPhysicalTxError::AlreadyLent(
                self.lent_to().expect("lent state carries its role"),
            ));
        };
        let state = core::mem::replace(&mut self.state, WdevPairedPhysicalTxState::Lent(role));
        match state {
            WdevPairedPhysicalTxState::Available {
                ordinary,
                aggregate,
            } => Ok((ordinary, aggregate)),
            WdevPairedPhysicalTxState::Lent(_) => unreachable!("availability checked above"),
        }
    }

    pub fn restore(
        &mut self,
        role: WdevPairRole,
        ordinary: Ordinary,
        aggregate: Aggregate,
    ) -> Result<(), (WdevPairedPhysicalTxError, Ordinary, Aggregate)> {
        match &self.state {
            WdevPairedPhysicalTxState::Available { .. } => {
                Err((WdevPairedPhysicalTxError::NotLent, ordinary, aggregate))
            }
            WdevPairedPhysicalTxState::Lent(actual) if *actual != role => Err((
                WdevPairedPhysicalTxError::WrongRole {
                    expected: *actual,
                    actual: role,
                },
                ordinary,
                aggregate,
            )),
            WdevPairedPhysicalTxState::Lent(_) => {
                self.state = WdevPairedPhysicalTxState::Available {
                    ordinary,
                    aggregate,
                };
                Ok(())
            }
        }
    }

    /// Consume an available physical pair while retaining it unchanged when
    /// a role still owns the finite hardware transaction.
    pub fn try_into_resources(self) -> Result<(Ordinary, Aggregate), Self> {
        match self.state {
            WdevPairedPhysicalTxState::Available {
                ordinary,
                aggregate,
            } => Ok((ordinary, aggregate)),
            WdevPairedPhysicalTxState::Lent(role) => Err(Self {
                state: WdevPairedPhysicalTxState::Lent(role),
            }),
        }
    }
}

#[cfg(test)]
mod physical_tx_tests {
    use super::*;

    #[test]
    fn one_role_lends_the_exact_pair_until_terminal_restore() {
        let mut owner = WdevPairedPhysicalTx::new(0x11_u8, 0x2222_u16);
        let (ordinary, aggregate) = owner.try_lend(WdevPairRole::Second).unwrap();

        assert_eq!(ordinary, 0x11);
        assert_eq!(aggregate, 0x2222);
        assert_eq!(owner.lent_to(), Some(WdevPairRole::Second));
        assert_eq!(
            owner.try_lend(WdevPairRole::First),
            Err(WdevPairedPhysicalTxError::AlreadyLent(WdevPairRole::Second))
        );

        owner
            .restore(WdevPairRole::Second, ordinary, aggregate)
            .unwrap();
        let resources = match owner.try_into_resources() {
            Ok(resources) => resources,
            Err(_) => panic!("both roles returned the physical pair"),
        };
        assert_eq!(resources, (0x11, 0x2222));
    }

    #[test]
    fn wrong_role_cannot_return_another_roles_capabilities() {
        let mut owner = WdevPairedPhysicalTx::new(7_u8, 9_u16);
        let (ordinary, aggregate) = owner.try_lend(WdevPairRole::First).unwrap();

        let (error, ordinary, aggregate) = owner
            .restore(WdevPairRole::Second, ordinary, aggregate)
            .unwrap_err();
        assert_eq!(
            error,
            WdevPairedPhysicalTxError::WrongRole {
                expected: WdevPairRole::First,
                actual: WdevPairRole::Second,
            }
        );
        owner
            .restore(WdevPairRole::First, ordinary, aggregate)
            .unwrap();
    }

    #[test]
    fn failed_role_transition_restores_the_exact_state() {
        let mut role = WdevPairedRoleOwner::<u16, u8>::parked(7);

        assert_eq!(
            role.try_activate(|parked| Err((11_u32, parked))),
            Err(WdevPairedRoleTransitionError::Conversion(11))
        );
        assert!(role.is_parked());
        role.try_activate(|parked| Ok::<_, (u32, u8)>(u16::from(parked) + 1))
            .unwrap();
        assert_eq!(role.active(), Some(&8));
        assert_eq!(
            role.try_activate(|parked| Ok::<_, (u32, u8)>(u16::from(parked))),
            Err(WdevPairedRoleTransitionError::AlreadyActive)
        );
        assert_eq!(
            role.try_park(|active| Err((13_u32, active))),
            Err(WdevPairedRoleTransitionError::Conversion(13))
        );
        assert_eq!(role.active(), Some(&8));
        role.try_park(|active| Ok::<_, (u32, u16)>(active as u8))
            .unwrap();
        assert_eq!(
            role.try_park(|active| Ok::<_, (u32, u16)>(active as u8)),
            Err(WdevPairedRoleTransitionError::AlreadyParked)
        );
        assert!(matches!(role.try_into_parked(), Ok(8)));
    }
}

fn unique_prepared_role(first: bool, second: bool) -> Option<WdevPairRole> {
    match (first, second) {
        (false, false) => None,
        (true, false) => Some(WdevPairRole::First),
        (false, true) => Some(WdevPairRole::Second),
        (true, true) => panic!("both VIFs cannot retain prepared TX ownership"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairedControlProgress<E> {
    Idle,
    More,
    TxPending(WdevPairRole),
    Exit(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairedRxProgress {
    Rx(WdevRxProgress),
    TxPending(WdevPairRole),
}

impl From<WdevRxProgress> for WdevPairedRxProgress {
    fn from(progress: WdevRxProgress) -> Self {
        Self::Rx(progress)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairedStopProgress {
    More,
    TxPending(WdevPairRole),
    Stopped,
}

/// Failure at the paired WDEV ownership boundary.
///
/// Role-local error vocabularies remain owned by their implementations. The
/// composition records which unique service produced the failure instead of
/// requiring unrelated DMA, STA, AP and control code to share an artificial
/// common error type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevPairedServiceError<RxError, FirstTxError, SecondTxError, ControlError> {
    Rx(RxError),
    FirstTx(FirstTxError),
    SecondTx(SecondTxError),
    Control(ControlError),
}

/// Common RX owner for a two-interface WDEV composition.
///
/// Implementations own the sole physical DMA producer and ordered VIF
/// dispatcher. The two publication arguments are already narrowed logical
/// endpoints; neither role processor can publish through the other one.
pub trait WdevPairedRxService<H, PhysicalTx, FirstRole, SecondRole> {
    type Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        first_role: &'a mut FirstRole,
        second_role: &'a mut SecondRole,
        first: &'a mut dyn WdevNetworkRx,
        second: &'a mut dyn WdevNetworkRx,
        context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevPairedRxProgress, Self::Error>> + 'a;

    fn service_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        first_role: &'a mut FirstRole,
        second_role: &'a mut SecondRole,
        first: &'a mut dyn WdevNetworkRx,
        second: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a;

    fn can_service_during_tx(&self) -> bool {
        true
    }

    fn has_work(&self, _first_role: &FirstRole, _second_role: &SecondRole) -> bool {
        false
    }

    fn serviced_frames(&self) -> u64 {
        0
    }
}

/// Control/lifecycle arbiter for two roles sharing one hardware transaction.
///
/// Every pending TX carries its role explicitly. The outer WDEV uses that
/// identity to narrow completion, deadline and standby ownership.
pub trait WdevPairedControlService<H, PhysicalTx, FirstTx, SecondTx> {
    type Error;
    type Exit;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        first_tx: &'a mut FirstTx,
        second_tx: &'a mut SecondTx,
        context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevPairedControlProgress<Self::Exit>, Self::Error>> + 'a;

    fn stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        first_tx: &mut FirstTx,
        second_tx: &mut SecondTx,
    ) -> Result<WdevPairedStopProgress, Self::Error>;

    fn wait_ready<'a>(
        &'a mut self,
        physical_tx: &'a mut PhysicalTx,
        first_tx: &'a mut FirstTx,
        second_tx: &'a mut SecondTx,
    ) -> impl Future<Output = ()> + 'a;
}

/// Unique paired-epoch owner of hardware, common RX, two role TX services and
/// their typed control arbiter.
pub trait WdevPairedNetworkTxService<
    'resources,
    M: RawMutex,
    H,
    PhysicalTx,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
>
{
    type Error;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    fn wait_deadline<'a>(
        &'a mut self,
        physical_tx: &'a mut PhysicalTx,
    ) -> impl Future<Output = ()> + 'a;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    fn has_prepared(&self) -> bool {
        false
    }

    fn preferred_batch_size(&self) -> usize {
        1
    }

    fn prepared_frame_count(&self) -> usize {
        0
    }

    fn start_prepared<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _physical_tx: &'a mut PhysicalTx,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        core::future::ready(Ok(WifiTxProgress::Complete))
    }

    fn cancel_prepared(&mut self, _physical_tx: &mut PhysicalTx) -> Result<(), Self::Error> {
        Ok(())
    }

    fn can_prepare(&self, _physical_tx: &PhysicalTx) -> bool {
        false
    }

    fn prepare<'a>(
        &'a mut self,
        _physical_tx: &'a mut PhysicalTx,
        _frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
    {
        core::future::ready(Ok(()))
    }
}

pub struct WdevPairedServiceSet<H, PhysicalTx, R, FirstTx, SecondTx, C> {
    first_interface: NetworkInterfaceId,
    second_interface: NetworkInterfaceId,
    hardware: H,
    physical_tx: PhysicalTx,
    rx: R,
    first_tx: FirstTx,
    second_tx: SecondTx,
    control: C,
    active: Option<WdevPairRole>,
    prepared: Option<WdevPairRole>,
}

impl<H, PhysicalTx, R, FirstTx, SecondTx, C>
    WdevPairedServiceSet<H, PhysicalTx, R, FirstTx, SecondTx, C>
{
    pub fn new(
        first_interface: NetworkInterfaceId,
        second_interface: NetworkInterfaceId,
        hardware: H,
        physical_tx: PhysicalTx,
        rx: R,
        first_tx: FirstTx,
        second_tx: SecondTx,
        control: C,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "paired WDEV services require distinct interfaces"
        );
        Self {
            first_interface,
            second_interface,
            hardware,
            physical_tx,
            rx,
            first_tx,
            second_tx,
            control,
            active: None,
            prepared: None,
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
    }

    pub const fn rx(&self) -> &R {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut R {
        &mut self.rx
    }

    pub fn into_parts(self) -> (H, PhysicalTx, R, FirstTx, SecondTx, C) {
        (
            self.hardware,
            self.physical_tx,
            self.rx,
            self.first_tx,
            self.second_tx,
            self.control,
        )
    }

    fn role_for(&self, interface: NetworkInterfaceId) -> WdevPairRole {
        if interface == self.first_interface {
            WdevPairRole::First
        } else if interface == self.second_interface {
            WdevPairRole::Second
        } else {
            panic!("TX interface does not belong to paired WDEV services")
        }
    }

    const fn interface_for(&self, role: WdevPairRole) -> NetworkInterfaceId {
        match role {
            WdevPairRole::First => self.first_interface,
            WdevPairRole::Second => self.second_interface,
        }
    }
}

impl<
    'resources,
    M,
    H,
    PhysicalTx,
    R,
    FirstTx,
    SecondTx,
    C,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for WdevPairedServiceSet<H, PhysicalTx, R, FirstTx, SecondTx, C>
where
    M: RawMutex,
    R: WdevPairedRxService<H, PhysicalTx, FirstTx, SecondTx>,
    FirstTx: WdevPairedNetworkTxService<
            'resources,
            M,
            H,
            PhysicalTx,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    SecondTx: WdevPairedNetworkTxService<
            'resources,
            M,
            H,
            PhysicalTx,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    C: WdevPairedControlService<H, PhysicalTx, FirstTx, SecondTx>,
{
    type Error = WdevPairedServiceError<R::Error, FirstTx::Error, SecondTx::Error, C::Error>;
    type Exit = C::Exit;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRxSet,
        context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            let (first, second) = network_rx
                .pair_mut(self.first_interface, self.second_interface)
                .expect("paired WDEV runner must retain both addressed RX endpoints");
            let progress = self
                .rx
                .service(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                    first,
                    second,
                    context,
                )
                .await
                .map_err(WdevPairedServiceError::Rx)?;
            Ok(match progress {
                WdevPairedRxProgress::Rx(progress) => progress,
                WdevPairedRxProgress::TxPending(role) => {
                    self.active = Some(role);
                    WdevRxProgress::ProbePending
                }
            })
        }
    }

    fn can_service_rx_during_tx(&self) -> bool {
        self.rx.can_service_during_tx()
    }

    fn service_rx_during_tx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRxSet,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            let (first, second) = network_rx
                .pair_mut(self.first_interface, self.second_interface)
                .expect("paired WDEV runner must retain both addressed RX endpoints");
            self.rx
                .service_during_tx(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                    first,
                    second,
                )
                .await
                .map_err(WdevPairedServiceError::Rx)
        }
    }

    fn has_rx_work(&self) -> bool {
        self.rx.has_work(&self.first_tx, &self.second_tx)
    }

    fn serviced_rx_frames(&self) -> u64 {
        self.rx.serviced_frames()
    }

    fn service_control<'a>(
        &'a mut self,
        context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        async move {
            let progress = self
                .control
                .service(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                    context,
                )
                .await
                .map_err(WdevPairedServiceError::Control)?;
            Ok(match progress {
                WdevPairedControlProgress::Idle => WdevControlProgress::Idle,
                WdevPairedControlProgress::More => WdevControlProgress::More,
                WdevPairedControlProgress::TxPending(role) => {
                    self.active = Some(role);
                    WdevControlProgress::TxPending
                }
                WdevPairedControlProgress::Exit(exit) => WdevControlProgress::Exit(exit),
            })
        }
    }

    fn active_tx_interface(&self) -> Option<NetworkInterfaceId> {
        self.active.map(|role| self.interface_for(role))
    }

    fn prepared_tx_interface(&self) -> Option<NetworkInterfaceId> {
        self.prepared
            .or_else(|| {
                unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
            })
            .map(|role| self.interface_for(role))
    }

    fn service_stop(&mut self) -> Result<WdevStopProgress, Self::Error> {
        Ok(
            match self
                .control
                .stop(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                )
                .map_err(WdevPairedServiceError::Control)?
            {
                WdevPairedStopProgress::More => WdevStopProgress::More,
                WdevPairedStopProgress::TxPending(role) => {
                    self.active = Some(role);
                    WdevStopProgress::TxPending
                }
                WdevPairedStopProgress::Stopped => WdevStopProgress::Stopped,
            },
        )
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        self.control.wait_ready(
            &mut self.physical_tx,
            &mut self.first_tx,
            &mut self.second_tx,
        )
    }

    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let role = self.role_for(network.interface());
            assert_eq!(*frame.tag(), network.interface());
            let progress = match role {
                WdevPairRole::First => self
                    .first_tx
                    .start(&mut self.hardware, &mut self.physical_tx, frame, network)
                    .await
                    .map_err(WdevPairedServiceError::FirstTx)?,
                WdevPairRole::Second => self
                    .second_tx
                    .start(&mut self.hardware, &mut self.physical_tx, frame, network)
                    .await
                    .map_err(WdevPairedServiceError::SecondTx)?,
            };
            self.active = (progress == WifiTxProgress::Pending).then_some(role);
            self.prepared =
                unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
            Ok(progress)
        }
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        async move {
            match self.active {
                Some(WdevPairRole::First) => {
                    self.first_tx.wait_deadline(&mut self.physical_tx).await
                }
                Some(WdevPairRole::Second) => {
                    self.second_tx.wait_deadline(&mut self.physical_tx).await
                }
                None => pending().await,
            }
        }
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let role = self.active.expect("paired TX completion requires one role");
            let progress = match role {
                WdevPairRole::First => self
                    .first_tx
                    .service(&mut self.hardware, &mut self.physical_tx, wake)
                    .await
                    .map_err(WdevPairedServiceError::FirstTx)?,
                WdevPairRole::Second => self
                    .second_tx
                    .service(&mut self.hardware, &mut self.physical_tx, wake)
                    .await
                    .map_err(WdevPairedServiceError::SecondTx)?,
            };
            if progress == WifiTxProgress::Complete {
                self.active = None;
            }
            self.prepared =
                unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
            Ok(progress)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared()).is_some()
    }

    fn preferred_tx_batch_size(&self) -> usize {
        match self.prepared.or_else(|| {
            unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
        }) {
            Some(WdevPairRole::First) => self.first_tx.preferred_batch_size(),
            Some(WdevPairRole::Second) => self.second_tx.preferred_batch_size(),
            None => 1,
        }
    }

    fn prepared_tx_frame_count(&self) -> usize {
        match self.prepared.or_else(|| {
            unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
        }) {
            Some(WdevPairRole::First) => self.first_tx.prepared_frame_count(),
            Some(WdevPairRole::Second) => self.second_tx.prepared_frame_count(),
            None => 0,
        }
    }

    fn start_prepared_tx<'a>(
        &'a mut self,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let role = self
                .prepared
                .or_else(|| {
                    unique_prepared_role(
                        self.first_tx.has_prepared(),
                        self.second_tx.has_prepared(),
                    )
                })
                .expect("prepared TX requires one retained role");
            assert_eq!(self.interface_for(role), network.interface());
            let progress = match role {
                WdevPairRole::First => self
                    .first_tx
                    .start_prepared(&mut self.hardware, &mut self.physical_tx, network)
                    .await
                    .map_err(WdevPairedServiceError::FirstTx)?,
                WdevPairRole::Second => self
                    .second_tx
                    .start_prepared(&mut self.hardware, &mut self.physical_tx, network)
                    .await
                    .map_err(WdevPairedServiceError::SecondTx)?,
            };
            self.prepared = None;
            self.active = (progress == WifiTxProgress::Pending).then_some(role);
            self.prepared =
                unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
            Ok(progress)
        }
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        let prepared = self.prepared.take().or_else(|| {
            unique_prepared_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
        });
        match prepared {
            Some(WdevPairRole::First) => self
                .first_tx
                .cancel_prepared(&mut self.physical_tx)
                .map_err(WdevPairedServiceError::FirstTx),
            Some(WdevPairRole::Second) => self
                .second_tx
                .cancel_prepared(&mut self.physical_tx)
                .map_err(WdevPairedServiceError::SecondTx),
            None => Ok(()),
        }
    }

    fn can_prepare_tx(&self) -> bool {
        match self.active.or(self.prepared) {
            Some(WdevPairRole::First) => self.first_tx.can_prepare(&self.physical_tx),
            Some(WdevPairRole::Second) => self.second_tx.can_prepare(&self.physical_tx),
            None => {
                self.first_tx.can_prepare(&self.physical_tx)
                    || self.second_tx.can_prepare(&self.physical_tx)
            }
        }
    }

    fn prepare_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            let role = self.role_for(network.interface());
            assert_eq!(*frame.tag(), network.interface());
            if let Some(retained) = self.prepared {
                assert_eq!(retained, role, "prepared aggregate cannot cross VIFs");
            }
            match role {
                WdevPairRole::First => {
                    self.first_tx
                        .prepare(&mut self.physical_tx, frame, network)
                        .await
                        .map_err(WdevPairedServiceError::FirstTx)?;
                    if self.first_tx.has_prepared() {
                        self.prepared = Some(role);
                    }
                }
                WdevPairRole::Second => {
                    self.second_tx
                        .prepare(&mut self.physical_tx, frame, network)
                        .await
                        .map_err(WdevPairedServiceError::SecondTx)?;
                    if self.second_tx.has_prepared() {
                        self.prepared = Some(role);
                    }
                }
            }
            Ok(())
        }
    }
}
