//! Transport-neutral session admission, controls and event projection above Agent Runtime.
pub mod security;
pub mod thread;
pub use thread::{
    HostedThread, HostedTurn, NewThread, ThreadEvent, ThreadHandle, ThreadManager,
    ThreadNotification, ThreadOp,
};

fn private_file(path: &std::path::Path, create_new: bool) -> anyhow::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
