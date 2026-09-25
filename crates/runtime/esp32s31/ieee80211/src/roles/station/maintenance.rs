//! Connected-station PHY maintenance: request protocol, automatic tracking
//! control, pause timeline and failure classification.
//!
//! These owners are executor-neutral and hold no hardware. The final
//! composition supplies the physical pause round trip, the static request
//! owner and the platform stop mechanism.

pub mod automatic;
pub mod policy;
pub mod request;
pub mod timeline;

pub use automatic::{Report as TrackingReport, Status as TrackingStatus};
pub use oer_esp32s31_phy::tracking::service::Config as TrackingConfig;
pub use policy::{PauseError, StoppedFailure};
pub use request::{Availability, PauseOperation, PauseReport, Requests};
pub use timeline::PauseTimeline;
