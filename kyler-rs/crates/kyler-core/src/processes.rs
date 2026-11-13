//! Session-owned sandboxed background processes.
mod interaction;
mod manager;
mod output;
mod store;

pub use crate::tools::exec_command::ProcessTool;
pub use manager::ProcessManager;
