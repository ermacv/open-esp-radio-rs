//! Running one scenario instance with one executor.

use core::{fmt, future::Future};

use oer_hil_model::{Identity, Instance, Spec, Verdict, identity};

use crate::Recorder;

/// A failure of the stand, the link or the host, not of the subject. It
/// leaves the run without a trace and therefore without a verdict
/// (`broken/infrastructure`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Infrastructure(pub String);

impl fmt::Display for Infrastructure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Infrastructure {}

/// Execution of a scenario by executor `X`. A scenario implements it for
/// every executor it can run on, typically generically over its port's
/// harness trait, so that the host model and the stand run the same code.
pub trait Run<X>: Spec {
    /// Drives the subject through `executor` and returns what it recorded.
    /// It decides nothing: the verdict is [`Spec::verdict`] over the trace.
    fn run(
        params: &Self::Params,
        executor: &mut X,
        recorder: &mut Recorder,
    ) -> impl Future<Output = Result<Self::Trace, Infrastructure>>;
}

/// One executed instance.
#[derive(Debug)]
pub struct Outcome<S: Spec> {
    pub identity: Identity,
    pub trace: S::Trace,
    pub verdict: Verdict,
    pub recorder: Recorder,
}

/// An instance that recorded no trace, with what it counted before.
#[derive(Debug)]
pub struct Failed {
    pub infrastructure: Infrastructure,
    pub recorder: Recorder,
}

/// Runs `instance` with `executor` and judges its trace with `criteria`.
pub async fn execute<S, X>(
    instance: &Instance<S>,
    criteria: &S::Criteria,
    executor: &mut X,
) -> Result<Outcome<S>, Failed>
where
    S: Run<X>,
{
    let mut recorder = Recorder::default();
    let identity = match identity::<S>(instance.name, &instance.params, criteria) {
        Ok(identity) => identity,
        Err(error) => {
            return Err(Failed {
                infrastructure: Infrastructure(format!("identity: {error}")),
                recorder,
            });
        }
    };
    match S::run(&instance.params, executor, &mut recorder).await {
        Ok(trace) => {
            let verdict = S::verdict(&trace, criteria);
            Ok(Outcome {
                identity,
                trace,
                verdict,
                recorder,
            })
        }
        Err(infrastructure) => Err(Failed {
            infrastructure,
            recorder,
        }),
    }
}
