//! Pure shared-modem-clock ownership planner for ESP32-S31.
//!
//! Every radio module (Wi-Fi, Bluetooth, IEEE 802.15.4, PHY, coexistence,
//! ...) requests a fixed set of modem clock dependencies. Each dependency has
//! one software refcount. Both acquisition and release visit a module's
//! dependencies from the lowest vendor dependency bit to the highest. A
//! physical acquire edge is emitted only for a refcount transition from zero
//! to one; a physical release edge only for a transition from one to zero.
//! Two vendor exceptions are reproduced exactly:
//!
//! - the analog-I2C master clock carries no refcount here: every module
//!   acquire and release emits its edge, because the vendor keeps that
//!   refcount in the platform analog-I2C owner;
//! - while Wi-Fi is initialized, the Wi-Fi clock dependencies keep their
//!   hardware enabled: their one-to-zero transitions emit no physical edge,
//!   and a later zero-to-one transition re-enables them (for the Wi-Fi
//!   baseband clock this includes its reset pulse).
//!
//! Source: reviewed evidence `ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK`, ESP-IDF
//! revision `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe` (Apache-2.0),
//! `components/esp_hw_support/modem/port/esp32s31/`
//! `modem_clock_impl.c` (`*_CLOCK_DEPS`, `modem_clock_get_module_deps`, the
//! per-device `modem_clock_*_configure` actions and the I2C-master refcount
//! exception in `modem_clock_device_context`), `modem/modem_clock.c`
//! (`config_wrapper`, `modem_clock_device_control`,
//! `modem_clock_configure_wifi_status`), the device order of
//! `modem/include/modem/modem_clock_impl.h` and the S31 `soc_caps.h`
//! selections (`SOC_MODEM_CLOCK_SOC_PLL_SOURCE_CG_SUPPORTED`,
//! `SOC_PHY_CALIBRATION_CLOCK_IS_INDEPENDENT`,
//! `SOC_MODEM_CLOCK_WIFI_BB_80X1_AS_APB`). The vendor `DATADUMP` device has no
//! module dependency and is omitted. Applies to ESP32-S31 only.
//!
//! This module deliberately performs no MMIO and issues no PLL, clock, PHY, or
//! radio-readiness proof. In particular, recording a physical edge as complete
//! is only transaction bookkeeping for a future sealed target executor. It is
//! not hardware evidence by itself. The monotonic modem ICG map that the vendor
//! ORs in on every module enable belongs to platform initialization.
//!
//! The current production baseline is externally retained and unknown: ESP-HAL
//! owns the upstream 160 MHz clock policy and existing Wi-Fi initialization may
//! already have enabled shared dependencies. Such a planner rejects acquisition
//! and release planning. The managed constructor remains test-only until every
//! radio client and the upstream PLL owner migrate to one common manager.

#![forbid(unsafe_code)]
#![allow(
    dead_code,
    reason = "the isolated planner is wired in a later iteration"
)]

use core::{fmt, ptr};

const DEPENDENCY_COUNT: usize = 16;
/// Maximum count representable by the reviewed vendor `int16_t` contract.
const MAX_REFCOUNT: u16 = i16::MAX as u16;

/// Internal allocation-free capacity, not a vendor hardware or ABI limit.
const MAX_ACTIVE_LEASES: usize = 16;

macro_rules! dependencies {
    ($($name:ident),+ $(,)?) => {
        /// Exact private low-bit-first dependency identities, in the vendor
        /// device order.
        ///
        /// Discriminants are internal planner indices, not public register masks.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        enum Dependency {
            $($name),+
        }

        impl Dependency {
            const LOW_BIT_FIRST: [Self; DEPENDENCY_COUNT] = [$(Self::$name),+];

            const fn acquire_edge(self) -> ModemClockAcquireEdge {
                match self {
                    $(Self::$name => ModemClockAcquireEdge::$name),+
                }
            }

            const fn release_edge(self) -> ModemClockReleaseEdge {
                match self {
                    $(Self::$name => ModemClockReleaseEdge::$name),+
                }
            }
        }

        /// One source-backed physical enable operation.
        ///
        /// The variants are semantic operations rather than register images.
        /// `Pll160AndModemSource` first acquires the upstream 160 MHz source,
        /// then opens the modem PLL-source gate; `WifiBb` pulses the Wi-Fi
        /// baseband reset before enabling its clock; `AnalogI2cMaster`
        /// acquires the platform analog-I2C clock reference. This planner
        /// performs none of them.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum ModemClockAcquireEdge {
            $($name),+
        }

        /// One source-backed physical disable operation.
        ///
        /// Release deliberately uses the same low-bit-first order as
        /// acquisition. `Pll160AndModemSource` closes the modem PLL-source
        /// gate before releasing the upstream 160 MHz source.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum ModemClockReleaseEdge {
            $($name),+
        }
    };
}

dependencies!(
    ModemAdcCommonFe,
    ModemPrivateFe,
    Pll160AndModemSource,
    Coexistence,
    AnalogI2cMaster,
    WifiApb,
    WifiBb44m,
    WifiMac,
    WifiBb,
    WifiBb80x1,
    Etm,
    BtMac,
    BtPeripheral,
    BtApbAndSecurity,
    BtIeee802154CommonBaseband,
    Ieee802154ApbAndMac,
);

impl Dependency {
    const fn index(self) -> usize {
        self as usize
    }

    const fn bit(self) -> u32 {
        1 << self.index()
    }

    /// The vendor keeps this refcount in the platform analog-I2C owner.
    const fn refcounted(self) -> bool {
        !matches!(self, Self::AnalogI2cMaster)
    }

    /// Hardware stays enabled while Wi-Fi is initialized.
    const fn retained_while_wifi_initialized(self) -> bool {
        matches!(
            self,
            Self::WifiApb | Self::WifiBb44m | Self::WifiMac | Self::WifiBb | Self::WifiBb80x1
        )
    }
}

/// Radio modules that request modem clocks, with their vendor dependency sets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockModule {
    AnalogI2cMaster,
    Phy,
    ModemAdcCommonFe,
    Coexistence,
    Wifi,
    Bluetooth,
    PhyCalibration,
    Ieee802154,
    ModemEtm,
    BluetoothApb,
}

impl ModemClockModule {
    const fn dependencies(self) -> DependencySet {
        use Dependency::*;
        const fn set(dependencies: &[Dependency]) -> DependencySet {
            let mut mask = 0;
            let mut index = 0;
            while index < dependencies.len() {
                mask |= dependencies[index].bit();
                index += 1;
            }
            DependencySet(mask)
        }
        match self {
            Self::AnalogI2cMaster => set(&[AnalogI2cMaster]),
            Self::Phy => set(&[
                ModemAdcCommonFe,
                ModemPrivateFe,
                Pll160AndModemSource,
                WifiBb80x1,
                AnalogI2cMaster,
            ]),
            Self::ModemAdcCommonFe => set(&[ModemAdcCommonFe, Pll160AndModemSource]),
            Self::Coexistence => set(&[Coexistence, Pll160AndModemSource]),
            Self::Wifi => set(&[
                WifiMac,
                WifiApb,
                WifiBb,
                WifiBb44m,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
            ]),
            Self::Bluetooth => set(&[
                BtMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
                BtPeripheral,
            ]),
            Self::PhyCalibration => set(&[
                WifiApb,
                WifiBb,
                WifiBb44m,
                WifiBb80x1,
                Pll160AndModemSource,
                BtIeee802154CommonBaseband,
                BtApbAndSecurity,
            ]),
            Self::Ieee802154 => set(&[
                Ieee802154ApbAndMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
            ]),
            Self::ModemEtm => set(&[Etm]),
            Self::BluetoothApb => set(&[BtApbAndSecurity, Etm, Pll160AndModemSource, BtMac]),
        }
    }
}

/// Private dependency membership. No raw mask crosses the module boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DependencySet(u32);

impl DependencySet {
    const fn contains(self, dependency: Dependency) -> bool {
        self.0 & dependency.bit() != 0
    }

    #[cfg(test)]
    fn from_dependencies(dependencies: &[Dependency]) -> Self {
        let mut mask = 0;
        for dependency in dependencies {
            mask |= dependency.bit();
        }
        Self(mask)
    }
}

/// Stable, non-zero-sized identity borrowed by one planner epoch.
///
/// A mutable borrow constructs the planner and remains live as long as any
/// lease from that epoch exists. Distinct live identities therefore have
/// distinct addresses without a global counter or unsafe code.
/// This is only a pure-model identity; it is not bound to a
/// `RegisteredPhyRadio` or any target hardware epoch.
pub(crate) struct ModemClockPlannerIdentity {
    _occupied: u8,
}

impl ModemClockPlannerIdentity {
    pub(crate) const fn new() -> Self {
        Self { _occupied: 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Baseline {
    ExternallyRetained,
    Managed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LeaseSlot {
    generation: u64,
    active: bool,
    dependencies: DependencySet,
}

impl LeaseSlot {
    const EMPTY: Self = Self {
        generation: 0,
        active: false,
        dependencies: DependencySet(0),
    };
}

/// Unique software owner of one shared-clock accounting epoch.
///
/// This type is intentionally neither `Clone` nor `Copy`. Counts and slots have
/// no public accessors or mutation API.
#[must_use = "dropping the planner abandons its complete software ownership epoch"]
pub(crate) struct ModemClockPlanner<'identity> {
    identity: &'identity ModemClockPlannerIdentity,
    baseline: Baseline,
    wifi_initialized: bool,
    counts: [u16; DEPENDENCY_COUNT],
    slots: [LeaseSlot; MAX_ACTIVE_LEASES],
}

impl<'identity> ModemClockPlanner<'identity> {
    /// Adopt the only currently honest production baseline.
    ///
    /// Unknown externally retained counts cannot produce a managed plan.
    pub(crate) fn externally_retained(identity: &'identity mut ModemClockPlannerIdentity) -> Self {
        Self {
            identity,
            baseline: Baseline::ExternallyRetained,
            wifi_initialized: false,
            counts: [0; DEPENDENCY_COUNT],
            slots: [LeaseSlot::EMPTY; MAX_ACTIVE_LEASES],
        }
    }

    /// Construct a zero-client managed epoch only in unit tests.
    ///
    /// Production must not gain an equivalent constructor until all clients and
    /// the upstream PLL ownership contract migrate together.
    #[cfg(test)]
    fn managed_for_test(identity: &'identity mut ModemClockPlannerIdentity) -> Self {
        Self {
            identity,
            baseline: Baseline::Managed,
            wifi_initialized: false,
            counts: [0; DEPENDENCY_COUNT],
            slots: [LeaseSlot::EMPTY; MAX_ACTIVE_LEASES],
        }
    }

    /// Record whether Wi-Fi is initialized; while it is, Wi-Fi clock
    /// dependencies keep their hardware enabled at a one-to-zero transition.
    pub(crate) fn set_wifi_initialized(&mut self, initialized: bool) {
        self.wifi_initialized = initialized;
    }

    /// Prepare acquisition of one module's exact vendor dependency set.
    ///
    /// Preparation is transactional: counts and lease slots remain unchanged
    /// until all emitted physical edges are acknowledged and commit is called.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner owner"
    )]
    pub(crate) fn prepare_module_acquire(
        self,
        module: ModemClockModule,
    ) -> Result<PreparedModemClockAcquire<'identity>, ModemClockAcquirePreparationFailure<'identity>>
    {
        self.prepare_acquire(module.dependencies())
    }

    /// Prepare acquisition of the exact reviewed IEEE 802.15.4 dependency set.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner owner"
    )]
    pub(crate) fn prepare_ieee802154_acquire(
        self,
    ) -> Result<PreparedModemClockAcquire<'identity>, ModemClockAcquirePreparationFailure<'identity>>
    {
        self.prepare_module_acquire(ModemClockModule::Ieee802154)
    }

    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner owner"
    )]
    fn prepare_acquire(
        self,
        dependencies: DependencySet,
    ) -> Result<PreparedModemClockAcquire<'identity>, ModemClockAcquirePreparationFailure<'identity>>
    {
        if self.baseline != Baseline::Managed {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::UnknownBaseline,
            });
        }

        for dependency in Dependency::LOW_BIT_FIRST {
            if dependencies.contains(dependency) {
                let Some(next_count) = self.counts[dependency.index()].checked_add(1) else {
                    return Err(ModemClockAcquirePreparationFailure {
                        planner: self,
                        error: ModemClockAcquirePreparationError::RefcountOverflow(
                            dependency.acquire_edge(),
                        ),
                    });
                };
                if next_count > MAX_REFCOUNT {
                    return Err(ModemClockAcquirePreparationFailure {
                        planner: self,
                        error: ModemClockAcquirePreparationError::RefcountOverflow(
                            dependency.acquire_edge(),
                        ),
                    });
                }
            }
        }

        let Some(slot) = self.slots.iter().position(|slot| !slot.active) else {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::LeaseCapacityReached,
            });
        };
        let Some(generation) = self.slots[slot].generation.checked_add(1) else {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::LeaseGenerationExhausted,
            });
        };

        Ok(PreparedModemClockAcquire {
            planner: self,
            dependencies,
            slot: slot as u8,
            generation,
            next_dependency: 0,
            completed_edges: 0,
        })
    }

    /// Validate an opaque lease and prepare its complete release transaction.
    ///
    /// Every validation failure returns both unchanged opaque owners. An unknown
    /// baseline is rejected before any lease interpretation or count change.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner and lease owners"
    )]
    pub(crate) fn prepare_release<'lease>(
        self,
        lease: ModemClockLease<'lease>,
    ) -> Result<
        PreparedModemClockRelease<'identity, 'lease>,
        ModemClockReleasePreparationFailure<'identity, 'lease>,
    > {
        let error = if self.baseline != Baseline::Managed {
            Some(ModemClockReleasePreparationError::UnknownBaseline)
        } else if !ptr::eq(self.identity, lease.identity) {
            Some(ModemClockReleasePreparationError::CrossManagerLease)
        } else if usize::from(lease.slot) >= MAX_ACTIVE_LEASES {
            Some(ModemClockReleasePreparationError::InvalidLeaseSlot)
        } else {
            let slot = self.slots[usize::from(lease.slot)];
            if slot.generation != lease.generation {
                Some(ModemClockReleasePreparationError::StaleLease)
            } else if !slot.active {
                Some(ModemClockReleasePreparationError::DuplicateRelease)
            } else if slot.dependencies != lease.dependencies {
                Some(ModemClockReleasePreparationError::LeaseRecordMismatch)
            } else {
                let mut underflow = None;
                for dependency in Dependency::LOW_BIT_FIRST {
                    if lease.dependencies.contains(dependency)
                        && self.counts[dependency.index()] == 0
                    {
                        underflow = Some(ModemClockReleasePreparationError::RefcountUnderflow(
                            dependency.release_edge(),
                        ));
                        break;
                    }
                }
                underflow
            }
        };

        if let Some(error) = error {
            return Err(ModemClockReleasePreparationFailure {
                planner: self,
                lease,
                error,
            });
        }

        Ok(PreparedModemClockRelease {
            planner: self,
            lease,
            next_dependency: 0,
            completed_edges: 0,
        })
    }
}

/// Error found before an acquire transaction exposes any physical edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockAcquirePreparationError {
    UnknownBaseline,
    RefcountOverflow(ModemClockAcquireEdge),
    LeaseCapacityReached,
    LeaseGenerationExhausted,
}

/// Failed acquire preparation retaining the unchanged manager.
#[must_use = "the failed preparation still owns the complete planner"]
pub(crate) struct ModemClockAcquirePreparationFailure<'identity> {
    planner: ModemClockPlanner<'identity>,
    error: ModemClockAcquirePreparationError,
}

impl fmt::Debug for ModemClockAcquirePreparationFailure<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModemClockAcquirePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<'identity> ModemClockAcquirePreparationFailure<'identity> {
    pub(crate) const fn error(&self) -> ModemClockAcquirePreparationError {
        self.error
    }

    /// Recover the unchanged planner; preparation has exposed no MMIO edge.
    pub(crate) fn into_planner(self) -> ModemClockPlanner<'identity> {
        self.planner
    }
}

/// Prepared but uncommitted acquire transaction.
///
/// No count or slot is modified while this owner exists.
#[must_use = "an acquire transaction must advance, commit, or remain abandoned"]
pub(crate) struct PreparedModemClockAcquire<'identity> {
    planner: ModemClockPlanner<'identity>,
    dependencies: DependencySet,
    slot: u8,
    generation: u64,
    next_dependency: u8,
    completed_edges: u8,
}

/// Next ownership state of an acquire transaction.
#[must_use = "the next acquire transaction owner is unique"]
pub(crate) enum ModemClockAcquireStep<'identity> {
    Physical(PendingModemClockAcquireEdge<'identity>),
    CommitReady(ModemClockAcquireCommitReady<'identity>),
}

impl<'identity> PreparedModemClockAcquire<'identity> {
    /// Move to the next boundary edge or the explicit commit state.
    pub(crate) fn advance(mut self) -> ModemClockAcquireStep<'identity> {
        while usize::from(self.next_dependency) < DEPENDENCY_COUNT {
            let dependency = Dependency::LOW_BIT_FIRST[usize::from(self.next_dependency)];
            self.next_dependency += 1;
            if self.dependencies.contains(dependency)
                && (!dependency.refcounted() || self.planner.counts[dependency.index()] == 0)
            {
                return ModemClockAcquireStep::Physical(PendingModemClockAcquireEdge {
                    transaction: self,
                    edge: dependency.acquire_edge(),
                });
            }
        }

        ModemClockAcquireStep::CommitReady(ModemClockAcquireCommitReady { transaction: self })
    }
}

/// Acquire transaction owner while one physical edge is outstanding.
///
/// It has no method which returns the planner. Failure becomes an opaque
/// poisoned owner, so partial physical execution cannot be represented as a
/// clean rollback.
#[must_use = "the outstanding physical edge retains the complete transaction"]
pub(crate) struct PendingModemClockAcquireEdge<'identity> {
    transaction: PreparedModemClockAcquire<'identity>,
    edge: ModemClockAcquireEdge,
}

impl<'identity> PendingModemClockAcquireEdge<'identity> {
    pub(crate) const fn edge(&self) -> ModemClockAcquireEdge {
        self.edge
    }

    /// Record bookkeeping completion and continue the same transaction.
    ///
    /// Only a future sealed target executor may treat this as hardware-backed.
    fn complete(mut self) -> PreparedModemClockAcquire<'identity> {
        self.transaction.completed_edges += 1;
        self.transaction
    }

    /// Retain the complete owner after an uncertain or failed physical edge.
    pub(crate) fn fail(self) -> PoisonedModemClockAcquire<'identity> {
        PoisonedModemClockAcquire { pending: self }
    }
}

/// Opaque owner after a physical acquire edge did not complete cleanly.
#[must_use = "a poisoned acquire still owns its planner and pending lease"]
pub(crate) struct PoisonedModemClockAcquire<'identity> {
    pending: PendingModemClockAcquireEdge<'identity>,
}

impl<'identity> PoisonedModemClockAcquire<'identity> {
    pub(crate) const fn edge(&self) -> ModemClockAcquireEdge {
        self.pending.edge
    }

    pub(crate) const fn completed_edges(&self) -> u8 {
        self.pending.transaction.completed_edges
    }

    /// Re-expose the same pure-model edge to unit tests.
    ///
    /// This is not a hardware retry contract. A production outcome-unknown
    /// composite edge remains terminal until a target executor can distinguish
    /// not-started from partially completed effects.
    #[cfg(test)]
    fn reexpose_for_test(self) -> PendingModemClockAcquireEdge<'identity> {
        self.pending
    }
}

/// Acquire transaction whose required physical edge sequence is complete.
#[must_use = "logical refcounts change only through explicit commit"]
pub(crate) struct ModemClockAcquireCommitReady<'identity> {
    transaction: PreparedModemClockAcquire<'identity>,
}

impl<'identity> ModemClockAcquireCommitReady<'identity> {
    /// Atomically publish logical counts and the new opaque lease.
    ///
    /// This is a software accounting commit, not a hardware-readiness witness.
    fn commit(mut self) -> (ModemClockPlanner<'identity>, ModemClockLease<'identity>) {
        for dependency in Dependency::LOW_BIT_FIRST {
            if self.transaction.dependencies.contains(dependency) {
                self.transaction.planner.counts[dependency.index()] += 1;
            }
        }

        let slot = &mut self.transaction.planner.slots[usize::from(self.transaction.slot)];
        slot.generation = self.transaction.generation;
        slot.active = true;
        slot.dependencies = self.transaction.dependencies;

        let lease = ModemClockLease {
            identity: self.transaction.planner.identity,
            slot: self.transaction.slot,
            generation: self.transaction.generation,
            dependencies: self.transaction.dependencies,
        };
        (self.transaction.planner, lease)
    }
}

/// Opaque ownership of one committed dependency acquisition.
///
/// This type is intentionally neither `Clone` nor `Copy` and exposes no slot,
/// generation, dependency set, or constructor.
#[must_use = "the shared-clock lease must remain owned until an explicit release"]
pub(crate) struct ModemClockLease<'identity> {
    identity: &'identity ModemClockPlannerIdentity,
    slot: u8,
    generation: u64,
    dependencies: DependencySet,
}

/// Error found before a release transaction exposes any physical edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockReleasePreparationError {
    UnknownBaseline,
    CrossManagerLease,
    InvalidLeaseSlot,
    StaleLease,
    DuplicateRelease,
    LeaseRecordMismatch,
    RefcountUnderflow(ModemClockReleaseEdge),
}

/// Failed release preparation retaining the unchanged planner and exact lease.
#[must_use = "both opaque owners remain in this failed release"]
pub(crate) struct ModemClockReleasePreparationFailure<'planner, 'lease> {
    planner: ModemClockPlanner<'planner>,
    lease: ModemClockLease<'lease>,
    error: ModemClockReleasePreparationError,
}

impl fmt::Debug for ModemClockReleasePreparationFailure<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModemClockReleasePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<'planner, 'lease> ModemClockReleasePreparationFailure<'planner, 'lease> {
    pub(crate) const fn error(&self) -> ModemClockReleasePreparationError {
        self.error
    }

    /// Recover only the two unchanged opaque owners, never counts or IDs.
    pub(crate) fn into_owners(self) -> (ModemClockPlanner<'planner>, ModemClockLease<'lease>) {
        (self.planner, self.lease)
    }
}

/// Prepared but uncommitted release transaction.
///
/// The lease remains active and counts remain unchanged until explicit commit.
#[must_use = "a release transaction must advance, commit, or remain abandoned"]
pub(crate) struct PreparedModemClockRelease<'planner, 'lease> {
    planner: ModemClockPlanner<'planner>,
    lease: ModemClockLease<'lease>,
    next_dependency: u8,
    completed_edges: u8,
}

/// Next ownership state of a release transaction.
#[must_use = "the next release transaction owner is unique"]
pub(crate) enum ModemClockReleaseStep<'planner, 'lease> {
    Physical(PendingModemClockReleaseEdge<'planner, 'lease>),
    CommitReady(ModemClockReleaseCommitReady<'planner, 'lease>),
}

impl<'planner, 'lease> PreparedModemClockRelease<'planner, 'lease> {
    /// Move to the next one-to-zero edge or the explicit commit state.
    pub(crate) fn advance(mut self) -> ModemClockReleaseStep<'planner, 'lease> {
        while usize::from(self.next_dependency) < DEPENDENCY_COUNT {
            let dependency = Dependency::LOW_BIT_FIRST[usize::from(self.next_dependency)];
            self.next_dependency += 1;
            let last = !dependency.refcounted() || self.planner.counts[dependency.index()] == 1;
            let retained =
                self.planner.wifi_initialized && dependency.retained_while_wifi_initialized();
            if self.lease.dependencies.contains(dependency) && last && !retained {
                return ModemClockReleaseStep::Physical(PendingModemClockReleaseEdge {
                    transaction: self,
                    edge: dependency.release_edge(),
                });
            }
        }

        ModemClockReleaseStep::CommitReady(ModemClockReleaseCommitReady { transaction: self })
    }
}

/// Release transaction owner while one physical edge is outstanding.
#[must_use = "the outstanding physical edge retains the planner and lease"]
pub(crate) struct PendingModemClockReleaseEdge<'planner, 'lease> {
    transaction: PreparedModemClockRelease<'planner, 'lease>,
    edge: ModemClockReleaseEdge,
}

impl<'planner, 'lease> PendingModemClockReleaseEdge<'planner, 'lease> {
    pub(crate) const fn edge(&self) -> ModemClockReleaseEdge {
        self.edge
    }

    /// Record bookkeeping completion and continue the same transaction.
    fn complete(mut self) -> PreparedModemClockRelease<'planner, 'lease> {
        self.transaction.completed_edges += 1;
        self.transaction
    }

    /// Retain the complete owner after an uncertain or failed physical edge.
    pub(crate) fn fail(self) -> PoisonedModemClockRelease<'planner, 'lease> {
        PoisonedModemClockRelease { pending: self }
    }
}

/// Opaque owner after a physical release edge did not complete cleanly.
#[must_use = "a poisoned release still owns its planner and exact lease"]
pub(crate) struct PoisonedModemClockRelease<'planner, 'lease> {
    pending: PendingModemClockReleaseEdge<'planner, 'lease>,
}

impl<'planner, 'lease> PoisonedModemClockRelease<'planner, 'lease> {
    pub(crate) const fn edge(&self) -> ModemClockReleaseEdge {
        self.pending.edge
    }

    pub(crate) const fn completed_edges(&self) -> u8 {
        self.pending.transaction.completed_edges
    }

    /// Re-expose the same pure-model edge to unit tests.
    ///
    /// This is not a hardware retry contract. A production outcome-unknown
    /// composite edge remains terminal until a target executor can distinguish
    /// not-started from partially completed effects.
    #[cfg(test)]
    fn reexpose_for_test(self) -> PendingModemClockReleaseEdge<'planner, 'lease> {
        self.pending
    }
}

/// Release transaction whose required physical edge sequence is complete.
#[must_use = "logical refcounts change only through explicit commit"]
pub(crate) struct ModemClockReleaseCommitReady<'planner, 'lease> {
    transaction: PreparedModemClockRelease<'planner, 'lease>,
}

impl<'planner, 'lease> ModemClockReleaseCommitReady<'planner, 'lease> {
    /// Atomically decrement counts and retire the exact lease slot.
    fn commit(mut self) -> ModemClockPlanner<'planner> {
        for dependency in Dependency::LOW_BIT_FIRST {
            if self.transaction.lease.dependencies.contains(dependency) {
                self.transaction.planner.counts[dependency.index()] -= 1;
            }
        }

        let slot = &mut self.transaction.planner.slots[usize::from(self.transaction.lease.slot)];
        slot.active = false;
        slot.dependencies = DependencySet(0);
        self.transaction.planner
    }
}

#[cfg(test)]
mod tests;
