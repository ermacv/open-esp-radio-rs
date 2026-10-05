//! The HIL stand's host around the stand file: what is attached where
//! ([`discover`]), whether the host is set up for the stand ([`doctor`]),
//! which fixtures answer ([`fixtures`]), and [`ssh`], the one way the host
//! reaches the stand's OpenWrt hosts.
//!
//! Fixture software installation, which runs as root, stays in
//! `oer-hil-fixture-install`, whose whole dependency graph is the
//! root-executed surface.
#![forbid(unsafe_code)]

pub mod discover;
pub mod doctor;
pub mod fixtures;
pub mod ssh;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
