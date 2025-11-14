#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::match_wildcard_for_single_variants)]
#![allow(clippy::missing_errors_doc)]

pub mod config;
pub mod grpc;
pub mod metrics;
pub mod state;
pub mod system;
