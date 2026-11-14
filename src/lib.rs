// Table headers keep the literal column names inline with their widths.
#![allow(clippy::print_literal)]

// One module per concern. Their items are re-exported at the crate root, so
// callers keep using `kubeloupe::get_pods`, `kubeloupe::LogOptions`, ...
mod access;
mod age;
mod cli;
mod client;
mod describe;
mod exec;
mod get;
mod logs;
mod metrics;
mod overview;
mod pods;
mod portforward;
mod shell_kubeconfig;

pub mod actions;
pub mod auth;
pub mod helm;
pub mod resources;
pub mod watch;
pub mod workload_logs;

pub use access::*;
pub(crate) use age::*;
pub use cli::*;
pub use client::*;
pub use describe::*;
pub use exec::*;
pub use get::*;
pub use logs::*;
pub use metrics::*;
pub use overview::*;
pub use pods::*;
pub use portforward::*;
pub use shell_kubeconfig::*;
pub use workload_logs::*;
