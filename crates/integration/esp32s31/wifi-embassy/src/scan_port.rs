//! Shared concrete scan-port facade.
//!
//! Cold startup scan and a later running rescan use different PAC and
//! interrupt owners, but the channel/RX/probe/dwell transaction is identical.
//! These neutral names keep callers from treating the production port as a
//! running-scan-only HIL helper.

pub use crate::running_scan::{
    EmbassyEsp32s31RunningScanTimer as EmbassyEsp32s31ScanTimer,
    Esp32s31RunningScanParts as Esp32s31ScanPortParts, Esp32s31RunningScanPort as Esp32s31ScanPort,
    Esp32s31RunningScanPortError as Esp32s31ScanPortError,
    Esp32s31RunningScanRadio as Esp32s31ScanRadio,
    Esp32s31RunningScanStation as Esp32s31ScanStation,
    Esp32s31RunningScanStorage as Esp32s31ScanStorage,
    Esp32s31RunningScanTelemetry as Esp32s31ScanTelemetry,
};
