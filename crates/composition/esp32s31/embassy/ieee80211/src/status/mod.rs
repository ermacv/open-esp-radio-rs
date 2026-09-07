//! Application-visible role status snapshots and internal publishers.

mod access_point;
mod station;

pub use access_point::{AccessPointStatus, AccessPointStatusSnapshot};

pub use station::{StationLinkState, StationStatus, StationStatusSnapshot};

pub(crate) use access_point::{publish_access_point_status, publish_access_point_stopped};

pub(crate) use station::{
    publish_station_connected, publish_station_disconnected, publish_station_tx_block_ack,
};
