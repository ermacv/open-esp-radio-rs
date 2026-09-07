//! Compatibility exports for the former combined WMM module.
//!
//! Shared traffic intent lives in `qos`; WMM element parsing lives in
//! `extensions::wmm`. Existing imports retain the same types and functions.

pub use crate::extensions::wmm::{WmmAcParameters, WmmParameterSet, parse_wmm_parameter_element};
pub use crate::qos::{
    Dscp, WmmAccessCategory, WmmClassificationSource, WmmTrafficClass, WmmUserPriority,
    classify_ethernet_wmm,
};
