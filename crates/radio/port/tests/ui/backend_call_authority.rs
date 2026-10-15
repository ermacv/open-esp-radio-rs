#![no_std]
use oer_radio_port::{Call, PortBackend};
pub async fn enable<B: PortBackend>(backend: &mut B, call: Call) {
    #[cfg(ui_reject)]
    let _ = backend.enable().await; // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _ = backend.enable(call).await;
}
