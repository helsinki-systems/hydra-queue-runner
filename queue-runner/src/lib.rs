#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]
#![recursion_limit = "256"]

pub mod config;
pub mod io;
pub mod server;
pub mod state;
pub mod utils;
