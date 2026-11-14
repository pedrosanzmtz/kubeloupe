//! The Tauri commands, one module per concern. `specta_builder` in
//! `main.rs` registers them.

pub(crate) mod actions;
pub(crate) mod cluster;
pub(crate) mod exec;
pub(crate) mod helm;
pub(crate) mod local_shell;
pub(crate) mod logs;
pub(crate) mod overview;
pub(crate) mod portforward;
pub(crate) mod resources;
pub(crate) mod watch;
