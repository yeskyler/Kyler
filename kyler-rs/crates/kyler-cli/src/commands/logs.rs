use super::configure::config_path;
use crate::{CliError, Result, args::LogTarget};

use std::{
    fs::{File, Metadata},
    io::{ErrorKind, Read, Seek, SeekFrom, Write},
    path::Path,
    time::Duration,
};

#[cfg(unix)]
type Identity = (u64, u64);
#[cfg(not(unix))]
type Identity = Option<std::time::SystemTime>;

fn identity(metadata: &Metadata) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        metadata.created().ok()
    }
}

// Scan backwards in bounded blocks instead of loading the entire log.
fn last_lines(file: &mut File, length: u64) -> std::io::Result<u64> {
    if length == 0 {
        return Ok(0);
    }
    let mut byte = [0];
    file.seek(SeekFrom::Start(length - 1))?;
    file.read_exact(&mut byte)?;
    let mut remaining = 100 + usize::from(byte[0] == b'\n');
    let mut position = length;
    let mut block = [0u8; 8192];
    while position > 0 {
        let count = position.min(block.len() as u64) as usize;
        position -= count as u64;
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut block[..count])?;
        for index in (0..count).rev() {
            if block[index] == b'\n' {
                remaining -= 1;
                if remaining == 0 {
                    return Ok(position + index as u64 + 1);
                }
            }
        }
    }
    Ok(0)
}

fn open(path: &Path) -> Result<File> {
    let metadata = path.metadata().map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            CliError::new(
                2,
                format!(
                    "Log file not found: {}. Start the managed service to create its log.",
                    path.display()
                ),
            )
        } else {
            CliError::new(3, format!("Cannot read log {}: {error}", path.display()))
        }
    })?;
    if !metadata.is_file() {
        return Err(CliError::new(2, "Log path must be a regular file"));
    }
    File::open(path).map_err(CliError::io)
}

pub async fn execute(explicit: Option<&Path>, target: LogTarget) -> Result<()> {
    let config = config_path(explicit)?;
    let directory = config
        .parent()
        .ok_or_else(|| CliError::new(2, "Invalid configuration path"))?;
    let filename = match target {
        LogTarget::Backend => "server.log",
        LogTarget::Web => "web.log",
    };
    let path = directory.join("logs").join(filename);
    let mut file = open(&path)?;
    let metadata = file.metadata().map_err(CliError::io)?;
    let mut current = identity(&metadata);
    let mut offset = last_lines(&mut file, metadata.len()).map_err(CliError::io)?;
    drop(file);
    eprintln!(
        "Following {} (last 100 lines). Press Ctrl+C to exit.",
        path.display()
    );
    let mut delay = Duration::ZERO;
    let mut block = [0u8; 64 * 1024];
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            signal = &mut interrupt => return signal.map_err(CliError::io),
            _ = tokio::time::sleep(delay) => {}
        }
        // Reopen by name so service restarts and log rotation do not leave us on an old inode.
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                delay = Duration::from_millis(250);
                continue;
            }
            Err(error) => return Err(CliError::io(error)),
        };
        let metadata = file.metadata().map_err(CliError::io)?;
        if !metadata.is_file() {
            return Err(CliError::new(2, "Log path must be a regular file"));
        }
        let next = identity(&metadata);
        if next != current || metadata.len() < offset {
            current = next;
            offset = 0;
        }
        file.seek(SeekFrom::Start(offset)).map_err(CliError::io)?;
        let count = file.read(&mut block).map_err(CliError::io)?;
        if count > 0 {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&block[..count]).map_err(CliError::io)?;
            stdout.flush().map_err(CliError::io)?;
            offset += count as u64;
            delay = Duration::ZERO;
        } else {
            delay = Duration::from_millis(250);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_handles_empty_unterminated_and_large_lines() {
        for (text, expected) in [
            (String::new(), String::new()),
            ("one\ntwo".into(), "one\ntwo".into()),
            (
                (0..120).map(|i| format!("{i}\n")).collect(),
                (20..120).map(|i| format!("{i}\n")).collect(),
            ),
            (
                format!("{}\n", "x".repeat(20_000)),
                format!("{}\n", "x".repeat(20_000)),
            ),
        ] {
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(text.as_bytes()).unwrap();
            let start = last_lines(&mut file, text.len() as u64).unwrap();
            file.seek(SeekFrom::Start(start)).unwrap();
            let mut actual = String::new();
            file.read_to_string(&mut actual).unwrap();
            assert_eq!(actual, expected);
        }
    }
}
