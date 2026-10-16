//! Executors and which purposes each admits.

use oer_hil_model::{Conformance, Interop, Investigation, Measurement, OpenQuestion};
use serde::Serialize;

/// Which executor ran an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Executed {
    Model,
    Stand,
}

mod sealed {
    pub trait Sealed {}
}

/// The kind of an executor: the host model or the stand.
pub trait ExecutorKind: sealed::Sealed {
    const VALUE: Executed;
}

/// The host model (`ModelAir`, a port's model backend).
pub enum OnModel {}

/// The stand: the target, real peers and the air.
pub enum OnStand {}

impl sealed::Sealed for OnModel {}
impl sealed::Sealed for OnStand {}

impl ExecutorKind for OnModel {
    const VALUE: Executed = Executed::Model;
}

impl ExecutorKind for OnStand {
    const VALUE: Executed = Executed::Stand;
}

/// A purpose an executor kind admits. The model executes conformance
/// only: interop needs real peers, measurements real hardware, and an
/// investigation asks about the stand.
pub trait RunsOn<K: ExecutorKind> {}

impl RunsOn<OnModel> for Conformance {}
impl RunsOn<OnStand> for Conformance {}
impl RunsOn<OnStand> for Interop {}
impl RunsOn<OnStand> for Measurement {}
impl<Q: OpenQuestion> RunsOn<OnStand> for Investigation<Q> {}

/// Something that executes scenarios: its kind, and the driver a scenario
/// uses (for a conformance scenario, its port's harness).
pub trait Executor {
    type Kind: ExecutorKind;
    type Driver;
    fn driver(&mut self) -> &mut Self::Driver;
}

/// The host model executor around a driver.
pub struct Model<D>(pub D);

/// The stand executor around a driver: for a conformance scenario, the
/// proxy that asks the target to exercise a case and decodes the trace it
/// reports.
pub struct Stand<D>(pub D);

impl<D> Executor for Model<D> {
    type Kind = OnModel;
    type Driver = D;

    fn driver(&mut self) -> &mut D {
        &mut self.0
    }
}

impl<D> Executor for Stand<D> {
    type Kind = OnStand;
    type Driver = D;

    fn driver(&mut self) -> &mut D {
        &mut self.0
    }
}
