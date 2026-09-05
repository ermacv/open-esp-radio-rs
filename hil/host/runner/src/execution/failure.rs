//! Convert execution errors without changing the public run-record schema.

use super::{Failure, FailureKind};

pub(crate) fn classify(error: &(dyn std::error::Error + 'static)) -> Failure {
    let mut cause = Some(error);
    let mut kind = FailureKind::Scenario;
    while let Some(error) = cause {
        if error.is::<crate::session::error::LinkError>()
            || error.is::<crate::fixture::Error>()
            || error.is::<std::io::Error>()
            || error.is::<serialport::Error>()
            || error.is::<oer_process::Cancelled>()
            || error.is::<oer_process::owned::DeadlineExceeded>()
        {
            kind = FailureKind::Infrastructure;
            break;
        }
        cause = error.source();
    }
    Failure::new(kind, error.to_string())
}
