//! Interactive setup prompts with local validation and hidden credential entry.
use crate::{CliError, Result};

use std::io::{self, Write};

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute, terminal,
};

use kyler_config::{SecretString, models::ProviderSettings};

/// Each prompt completes its validation loop before the next step is printed.
/// Plain terminal output remains in scrollback; setup never opens a full-screen view.
#[derive(Default)]
pub(super) struct Prompts {
    completed: usize,
}
impl Prompts {
    pub fn section(&mut self, title: &str) {
        self.completed = 0;
        eprintln!("\nKyler · {title}\n");
    }
    fn step<T>(&mut self, label: &str, read: impl FnOnce(&str) -> Result<T>) -> Result<T> {
        let label = format!("[{}] {label}", self.completed + 1);
        let value = read(&label)?;
        self.completed += 1;
        Ok(value)
    }
    pub fn model(&mut self, default: &str) -> Result<String> {
        self.step("Model", |label| model(label, default))
    }
    pub fn api_url(&mut self, label: &str, default: &str) -> Result<String> {
        self.step(label, |label| api_url(label, default))
    }
    pub fn access_scope(&mut self, lan: bool) -> Result<bool> {
        self.step("Access scope", |label| {
            loop {
                match prompt(label, if lan { "2" } else { "1" })?.as_str() {
                    "1" => return Ok(false),
                    "2" => return Ok(true),
                    _ => eprintln!("Choose 1 for this machine or 2 for local network access."),
                }
            }
        })
    }
    pub fn key(
        &mut self,
        label: &str,
        existing: &SecretString,
        optional: bool,
    ) -> Result<SecretString> {
        self.step(label, |label| key(label, existing, optional))
    }
}

fn prompt(label: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        eprint!("{label}: ");
    } else {
        eprint!("{label} [{default}]: ");
    }
    io::stderr().flush().map_err(CliError::io)?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line).map_err(CliError::io)? == 0 || line.trim() == "\u{1b}" {
        return Err(CliError::new(130, "Configuration cancelled."));
    }
    let line = line.trim();
    Ok(if line.is_empty() {
        default.into()
    } else {
        line.into()
    })
}

fn model(label: &str, default: &str) -> Result<String> {
    loop {
        let value = prompt(label, default)?;
        if !value.is_empty() && !value.chars().any(char::is_control) {
            return Ok(value);
        }
        eprintln!("Enter a model ID, such as deepseek-ai/DeepSeek-V4-Flash.");
    }
}

fn validate_url(value: &str) -> std::result::Result<(), &'static str> {
    let provider = ProviderSettings {
        base_url: value.into(),
        api_key: SecretString::new("validation-only"),
        ..Default::default()
    };
    if kyler_config::models::validate_provider(&provider).is_err() {
        return Err(
            "Use an http:// or https:// API base URL without credentials, query or fragment.",
        );
    }
    if value.trim_end_matches('/').ends_with("/chat/completions") {
        return Err(
            "Use the API base URL, without /chat/completions (for example https://api.siliconflow.cn/v1).",
        );
    }
    Ok(())
}

fn api_url(label: &str, default: &str) -> Result<String> {
    loop {
        let value = prompt(label, default)?;
        match validate_url(&value) {
            Ok(()) => return Ok(value.trim_end_matches('/').into()),
            Err(message) => eprintln!("{message} Try again."),
        }
    }
}

fn normalize_key(value: &str) -> std::result::Result<&str, &'static str> {
    let value = value.trim();
    if value
        .chars()
        .any(|c| !c.is_ascii() || c.is_whitespace() || c.is_control())
    {
        return Err("Paste one API key without spaces or line breaks.");
    }
    Ok(value)
}

fn key(label: &str, existing: &SecretString, optional: bool) -> Result<SecretString> {
    let hint = if !existing.is_empty() {
        "configured; hidden; Enter keeps"
    } else if optional {
        "hidden; Enter skips"
    } else {
        "hidden; required"
    };
    loop {
        let value = password(label, hint)?;
        match normalize_key(&value) {
            Ok("") if !existing.is_empty() => match normalize_key(existing.expose()) {
                Ok(value) if !value.is_empty() => return Ok(SecretString::new(value)),
                _ => eprintln!("Saved key is invalid. Paste a replacement."),
            },
            Ok("") if optional => return Ok(SecretString::default()),
            Ok("") => eprintln!("An API key is required. Paste it, or press Esc to cancel."),
            Ok(value) => return Ok(SecretString::new(value)),
            Err(message) => eprintln!("{message}"),
        }
    }
}

fn password(label: &str, hint: &str) -> Result<String> {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = execute!(io::stderr(), DisableBracketedPaste);
            let _ = terminal::disable_raw_mode();
            eprintln!();
        }
    }
    terminal::enable_raw_mode().map_err(CliError::io)?;
    let _guard = Guard;
    execute!(io::stderr(), EnableBracketedPaste).map_err(CliError::io)?;
    eprint!("{label} [{hint}]: ");
    io::stderr().flush().map_err(CliError::io)?;
    read_password()
}

#[cfg(unix)]
fn read_password() -> Result<String> {
    use std::{
        fs::File,
        io::Read,
        os::fd::{AsFd, AsRawFd},
    };

    // poll(2) supports /dev/tty on macOS, unlike the event reader's kqueue registration.
    let mut input = File::from(
        io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(CliError::io)?,
    );
    collect_password(|timeout| {
        if let Some(timeout) = timeout {
            let mut descriptor = libc::pollfd {
                fd: input.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
                if ready == 0 {
                    return Ok(None);
                }
                if ready > 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(CliError::io(error));
                }
            }
        }
        let mut byte = [0];
        match input.read_exact(&mut byte) {
            Ok(()) => Ok(Some(byte[0])),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(error) => Err(CliError::io(error)),
        }
    })
}

#[cfg(any(unix, test))]
fn collect_password(mut next: impl FnMut(Option<i32>) -> Result<Option<u8>>) -> Result<String> {
    let mut value = Vec::new();
    let mut paste = false;
    loop {
        let byte = next(None)?.ok_or_else(|| CliError::new(130, "Configuration cancelled."))?;
        if paste {
            value.push(byte);
            if value.ends_with(b"\x1b[201~") {
                value.truncate(value.len() - 6);
                paste = false;
            }
            continue;
        }
        match byte {
            b'\r' | b'\n' => {
                return String::from_utf8(value)
                    .map_err(|_| CliError::new(2, "Input must be valid UTF-8."));
            }
            3 | 4 => return Err(CliError::new(130, "Configuration cancelled.")),
            8 | 127 => {
                if !value.is_empty() {
                    let mut start = value.len() - 1;
                    while start > 0 && value[start] & 0xc0 == 0x80 {
                        start -= 1;
                    }
                    value.truncate(start);
                }
            }
            27 => {
                let Some(first) = next(Some(100))? else {
                    return Err(CliError::new(130, "Configuration cancelled."));
                };
                if first != b'[' && first != b'O' {
                    continue;
                }
                let mut sequence = vec![first];
                // Consume bounded CSI/SS3 keys without inserting escape sequences into a secret.
                for _ in 0..32 {
                    let Some(byte) = next(Some(100))? else {
                        break;
                    };
                    sequence.push(byte);
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
                if sequence == b"[200~" {
                    paste = true;
                }
            }
            byte if byte >= 32 => value.push(byte),
            _ => {}
        }
    }
}

#[cfg(not(unix))]
fn read_password() -> Result<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

    let mut value = String::new();
    loop {
        match event::read().map_err(CliError::io)? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => return Ok(value),
                KeyCode::Esc => return Err(CliError::new(130, "Configuration cancelled.")),
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(CliError::new(130, "Configuration cancelled."));
                }
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !c.is_control() =>
                {
                    value.push(c)
                }
                _ => {}
            },
            Event::Paste(text) => value.push_str(&text),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_key, validate_url};

    #[test]
    fn hidden_input_preserves_paste_and_handles_editing_without_an_event_reader() {
        let collect = |bytes: &[u8]| {
            let mut bytes = bytes.iter().copied();
            super::collect_password(|_| Ok(bytes.next()))
        };
        assert_eq!(collect(b"secret-x\x7fy\r").unwrap(), "secret-y");
        assert_eq!(collect(b"secret\x1b[D\r").unwrap(), "secret");
        assert_eq!(
            collect(b"\x1b[200~first\nsecond\x1b[201~\r").unwrap(),
            "first\nsecond"
        );
        assert_eq!(collect("ab\u{00e9}\u{7f}c\r".as_bytes()).unwrap(), "abc");
        for cancelled in [b"\x1b".as_slice(), b"\x03", b"\x04", b""] {
            assert_eq!(collect(cancelled).unwrap_err().code, 130);
        }
    }
    #[test]
    fn api_urls_reject_credentials_and_completion_endpoints() {
        for value in ["https://example.com/v1", "http://127.0.0.1:3000/v1/"] {
            assert!(validate_url(value).is_ok());
        }
        for value in [
            "example.com",
            "ftp://example.com",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1/chat/completions/",
        ] {
            assert!(validate_url(value).is_err());
        }
    }

    #[test]
    fn pasted_keys_trim_edges_but_never_join_multiple_lines() {
        assert_eq!(normalize_key("  sk-test-key\n").unwrap(), "sk-test-key");
        assert_eq!(
            normalize_key("test-\"quote\\slash").unwrap(),
            "test-\"quote\\slash"
        );
        assert!(normalize_key("first\nsecond").is_err());
        assert!(normalize_key("Bearer key").is_err());
        assert!(normalize_key("key\u{1b}").is_err());
    }
}
