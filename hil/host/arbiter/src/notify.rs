//! Best-effort desktop notifications for the user.

use std::{process::Command, time::Duration};

/// Set to `0` to disable desktop notifications.
const NOTIFY_ENV: &str = "OER_HIL_NOTIFY";

/// Send a desktop notification when `notify-send` is available. Failure never
/// affects arbitration.
pub(crate) fn send(summary: &str, body: &str) {
    if std::env::var(NOTIFY_ENV).is_ok_and(|value| value == "0") || cfg!(test) {
        return;
    }
    let mut command = Command::new("notify-send");
    command.args(["--app-name=HIL stand", summary, body]);
    let _ = oer_process::output(&mut command, Some(Duration::from_secs(5)));
}
