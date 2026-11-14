// Table headers keep the literal column names inline with their widths.
#![allow(clippy::print_literal)]

// One module per concern. Their items are re-exported at the crate root, so
// callers keep using `kubeloupe::get_pods`, `kubeloupe::LogOptions`, ...
mod age;
mod cli;
mod client;
mod exec;
mod logs;
mod metrics;
mod overview;
mod pods;
mod portforward;

pub mod actions;
pub mod auth;
pub mod resources;
pub mod watch;
pub mod workload_logs;

pub(crate) use age::*;
pub use cli::*;
pub use client::*;
pub use exec::*;
pub use logs::*;
pub use metrics::*;
pub use overview::*;
pub use pods::*;
pub use portforward::*;
pub use workload_logs::*;
