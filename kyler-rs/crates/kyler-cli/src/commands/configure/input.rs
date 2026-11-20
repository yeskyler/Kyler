//! Inline setup controls with typed navigation and credential-safe terminal input.
use crate::{CliError, Result};

use std::{
    future::{Future, poll_fn},
    io::{self, IsTerminal, Write},
    pin::Pin,
    task::Poll,
    time::Duration,
};

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute, terminal,
};
use inquire::{
    Select, Text,
    error::InquireError,
    ui::{Color, RenderConfig, Styled},
};

use kyler_config::{SecretString, models::ProviderSettings};
use kyler_core::context::ContextPolicy;

pub(super) const BACK: u8 = 129;
fn back() -> CliError {
    CliError::new(BACK, "Return to the previous step")
}
pub(super) fn cancelled() -> CliError {
    CliError::new(130, "Configuration cancelled.")
}
fn prompt_error(error: InquireError) -> CliError {
    match error {
        InquireError::OperationCanceled => back(),
        InquireError::OperationInterrupted => cancelled(),
        InquireError::IO(e) if e.kind() == io::ErrorKind::Interrupted => cancelled(),
        InquireError::IO(e) => CliError::io(e),
        _ => CliError::new(
            2,
            "Could not open the terminal wizard. Use --non-interactive for unattended setup.",
        ),
    }
}
fn theme() -> RenderConfig<'static> {
    let mut config = RenderConfig::default();
    let colored = std::env::var_os("NO_COLOR").is_none() && io::stderr().is_terminal();
    config.prompt_prefix = if colored {
        Styled::new("◆").with_fg(Color::LightCyan)
    } else {
        Styled::new("◆")
    };
    config.answered_prompt_prefix = if colored {
        Styled::new("◇").with_fg(Color::LightGreen)
    } else {
        Styled::new("◇")
    };
    config.highlighted_option_prefix = Styled::new("›");
    config.canceled_prompt_indicator = Styled::new("Back");
    config
}
const HELP: &str = "↑↓ Select · Type to search · Enter Continue · Esc Back · Ctrl+C Cancel";
pub(super) struct Prompts {
    cancel: Pin<Box<dyn Future<Output = io::Result<()>> + Send>>,
}
#[derive(Clone)]
pub(super) enum ModelChoice {
    Selected(String),
    Manual,
    Retry,
    Cached,
}
#[derive(Clone)]
struct ModelOption {
    label: String,
    choice: ModelChoice,
}
impl std::fmt::Display for ModelOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}
impl Prompts {
    pub async fn new() -> Result<Self> {
        // Register cancellation before a directory fetch can run in canonical terminal mode.
        let mut cancel: Pin<Box<dyn Future<Output = io::Result<()>> + Send>> =
            Box::pin(tokio::signal::ctrl_c());
        poll_fn(|cx| {
            if let Poll::Ready(result) = cancel.as_mut().poll(cx) {
                result.map_err(CliError::io)?;
                return Poll::Ready(Err(cancelled()));
            }
            Poll::Ready(Ok::<_, CliError>(()))
        })
        .await?;
        Ok(Self { cancel })
    }
    pub fn section(&mut self, title: &str) {
        eprintln!("\n┌  Kyler · {title}\n│  Enter Continue · Esc Back · Ctrl+C Cancel\n│");
    }
    pub fn select(&mut self, label: &str, options: Vec<String>, current: usize) -> Result<usize> {
        Select::new(label, options)
            .with_starting_cursor(current)
            .with_page_size(7)
            .with_help_message(HELP)
            .with_render_config(theme())
            .raw_prompt()
            .map(|v| v.index)
            .map_err(prompt_error)
    }
    pub fn provider(
        &mut self,
        label: &str,
        default: kyler_provider::AdapterKind,
    ) -> Result<kyler_provider::AdapterKind> {
        use kyler_provider::AdapterKind::*;
        let kinds = [Openai, Siliconflow, Generic];
        let index = self.select(
            label,
            vec![
                "OpenAI".into(),
                "SiliconFlow".into(),
                "Custom (OpenAI-compatible API)".into(),
            ],
            kinds.iter().position(|k| *k == default).unwrap_or(0),
        )?;
        Ok(kinds[index])
    }
    pub fn connection(&mut self, names: &[String], default: &str) -> Result<String> {
        let n = self.select(
            "Provider connection",
            names.to_vec(),
            names.iter().position(|n| n == default).unwrap_or(0),
        )?;
        Ok(names[n].clone())
    }
    pub fn model_choice(
        &mut self,
        default: &str,
        models: &[kyler_provider::discovery::DiscoveredModel],
        cached: bool,
    ) -> Result<ModelChoice> {
        let mut options = models
            .iter()
            .map(|m| ModelOption {
                label: m.id.clone(),
                choice: ModelChoice::Selected(m.id.clone()),
            })
            .collect::<Vec<_>>();
        if !default.is_empty() && !models.iter().any(|m| m.id == default) {
            options.insert(
                0,
                ModelOption {
                    label: format!("Keep current model · {default}"),
                    choice: ModelChoice::Selected(default.into()),
                },
            );
        }
        options.push(ModelOption {
            label: "Enter model ID manually…".into(),
            choice: ModelChoice::Manual,
        });
        options.push(ModelOption {
            label: "Retry discovery".into(),
            choice: ModelChoice::Retry,
        });
        if cached {
            options.push(ModelOption {
                label: "Use cached models".into(),
                choice: ModelChoice::Cached,
            });
        }
        let current = options
            .iter()
            .position(|o| matches!(&o.choice,ModelChoice::Selected(id) if id==default))
            .unwrap_or(0);
        let scorer = |query: &str, option: &ModelOption, label: &str, _: usize| {
            if !matches!(option.choice, ModelChoice::Selected(_)) {
                return Some(if label.to_lowercase().contains(&query.to_lowercase()) {
                    0
                } else {
                    -1
                });
            }
            label
                .to_lowercase()
                .contains(&query.to_lowercase())
                .then_some(1)
        };
        Select::new("Model", options)
            .with_starting_cursor(current)
            .with_scorer(&scorer)
            .with_page_size(7)
            .with_help_message(HELP)
            .with_render_config(theme())
            .prompt()
            .map(|o| o.choice)
            .map_err(prompt_error)
    }
    pub fn manual_model(&mut self, default: &str) -> Result<String> {
        Text::new("Model ID")
            .with_default(default)
            .with_help_message("Enter a model ID · Esc Back · Ctrl+C Cancel")
            .with_validator(|v: &str| {
                Ok(if valid_model_id(v.trim()) {
                    inquire::validator::Validation::Valid
                } else {
                    inquire::validator::Validation::Invalid(
                        "Enter a model ID with at most 256 characters.".into(),
                    )
                })
            })
            .with_render_config(theme())
            .prompt()
            .map(|id| id.trim().to_owned())
            .map_err(prompt_error)
    }
    pub fn context_window(
        &mut self,
        label: &str,
        default: Option<usize>,
        policy: &ContextPolicy,
    ) -> Result<Option<usize>> {
        let default = default.map_or_else(|| "auto".into(), |n| n.to_string());
        let value = Text::new(label)
            .with_default(&default)
            .with_help_message("auto follows verified model capacity · Esc Back")
            .with_validator(|v: &str| {
                let v = v.trim();
                Ok(if v.eq_ignore_ascii_case("auto") {
                    inquire::validator::Validation::Valid
                } else {
                    match parse_context_window(v, policy) {
                        Ok(_) => inquire::validator::Validation::Valid,
                        Err(e) => inquire::validator::Validation::Invalid(e.into()),
                    }
                })
            })
            .with_render_config(theme())
            .prompt()
            .map_err(prompt_error)?;
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            Ok(None)
        } else {
            parse_context_window(&value, policy)
                .map(Some)
                .map_err(|e| CliError::new(2, e))
        }
    }
    pub fn api_url(&mut self, label: &str, default: &str) -> Result<String> {
        Text::new(label)
            .with_default(default)
            .with_help_message("API base URL · Enter keeps current · Esc Back")
            .with_validator(|v: &str| {
                Ok(match validate_url(v.trim()) {
                    Ok(()) => inquire::validator::Validation::Valid,
                    Err(e) => inquire::validator::Validation::Invalid(e.into()),
                })
            })
            .with_render_config(theme())
            .prompt()
            .map(|s| s.trim().trim_end_matches('/').into())
            .map_err(prompt_error)
    }
    pub fn access_scope(&mut self, lan: bool) -> Result<bool> {
        self.select(
            "Access scope",
            vec![
                "This machine only · 127.0.0.1".into(),
                "Local network · all interfaces".into(),
            ],
            usize::from(lan),
        )
        .map(|n| n == 1)
    }
    pub fn key(
        &mut self,
        label: &str,
        existing: &SecretString,
        optional: bool,
    ) -> Result<SecretString> {
        key(label, existing, optional)
    }
    pub async fn loading<T>(&mut self, label: &str, future: impl Future<Output = T>) -> Result<T> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct LoadingGuard(Arc<AtomicBool>);
        impl Drop for LoadingGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
                let _ = terminal::disable_raw_mode();
                eprint!("\r\x1b[2K");
                let _ = io::stderr().flush();
            }
        }
        terminal::enable_raw_mode().map_err(CliError::io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let _guard = LoadingGuard(stop.clone());
        let key_stop = stop.clone();
        let mut keyboard = tokio::task::spawn_blocking(move || loading_key(key_stop));
        tokio::pin!(future);
        let mut ticks = tokio::time::interval(Duration::from_millis(100));
        let mut frame = 0;
        let mut key_finished = false;
        let result = loop {
            tokio::select! {
                value=&mut future=>break Ok(value),
                _=&mut self.cancel=>break Err(cancelled()),
                exit=&mut keyboard=>{key_finished=true;break Err(exit.unwrap_or_else(|_|CliError::new(3,"Terminal input stopped")));},
                _=ticks.tick()=>{let frames=["◐","◓","◑","◒"];eprint!("\r\x1b[2K{} {label} · Esc Back · Ctrl+C Cancel",frames[frame%frames.len()]);if let Err(e)=io::stderr().flush(){break Err(CliError::io(e));}frame+=1;}
            }
        };
        stop.store(true, Ordering::Relaxed);
        if !key_finished {
            let _ = keyboard.await;
        }
        result
    }
}
/// macOS /dev/tty readiness requires select(2); poll can report a non-readable
/// event there. Check actual readability so no input worker blocks on a false event.
#[cfg(unix)]
fn input_ready(fd: std::os::fd::RawFd, timeout: i32) -> io::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        if fd < 0 || fd as usize >= libc::FD_SETSIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Terminal descriptor is out of range",
            ));
        }
        let mut reads: libc::fd_set = unsafe { std::mem::zeroed() };
        unsafe {
            libc::FD_SET(fd, &mut reads);
        }
        let mut duration = libc::timeval {
            tv_sec: (timeout / 1000) as _,
            tv_usec: ((timeout % 1000) * 1000) as _,
        };
        let ready = unsafe {
            libc::select(
                fd + 1,
                &mut reads,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut duration,
            )
        };
        if ready < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ready > 0 && unsafe { libc::FD_ISSET(fd, &reads) })
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLNVAL | libc::POLLHUP) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Terminal input closed",
            ));
        }
        Ok(ready > 0 && descriptor.revents & libc::POLLIN != 0)
    }
}
#[cfg(unix)]
fn loading_key(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> CliError {
    use std::os::fd::AsFd;
    let descriptor = match io::stdin().as_fd().try_clone_to_owned() {
        Ok(fd) => fd,
        Err(e) => return CliError::io(e),
    };
    loading_key_from_file(std::fs::File::from(descriptor), stop)
}
#[cfg(unix)]
fn loading_key_from_file(
    mut input: std::fs::File,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> CliError {
    use std::{io::Read, os::fd::AsRawFd, sync::atomic::Ordering};
    while !stop.load(Ordering::Relaxed) {
        let ready = match input_ready(input.as_raw_fd(), 100) {
            Ok(ready) => ready,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return CliError::io(e),
        };
        if ready {
            let mut byte = [0];
            if let Err(e) = input.read_exact(&mut byte) {
                return CliError::io(e);
            }
            match byte[0] {
                27 => {
                    match input_ready(input.as_raw_fd(), 80) {
                        Ok(false) => return back(),
                        Err(_) => return cancelled(),
                        Ok(true) => {}
                    }
                    if input.read_exact(&mut byte).is_err() {
                        return cancelled();
                    }
                    if matches!(byte[0], b'[' | b'O') {
                        for _ in 0..32 {
                            if !input_ready(input.as_raw_fd(), 80).unwrap_or(false) {
                                break;
                            }
                            if input.read_exact(&mut byte).is_err() {
                                return cancelled();
                            }
                            if (0x40..=0x7e).contains(&byte[0]) {
                                break;
                            }
                        }
                    }
                }
                3 | 4 => return cancelled(),
                _ => {}
            }
        }
    }
    cancelled()
}
#[cfg(not(unix))]
fn loading_key(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> CliError {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use std::sync::atomic::Ordering;
    while !stop.load(Ordering::Relaxed) {
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.code == KeyCode::Esc => return back(),
                Ok(Event::Key(k))
                    if matches!(k.code, KeyCode::Char('c' | 'd'))
                        && k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return cancelled();
                }
                Err(e) => return CliError::io(e),
                _ => {}
            },
            Err(e) => return CliError::io(e),
            _ => {}
        }
    }
    cancelled()
}
fn valid_model_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn parse_context_window(value: &str, policy: &ContextPolicy) -> std::result::Result<usize, String> {
    let value = value.trim();
    let (number, multiplier) = match value.strip_suffix(['k', 'K']) {
        Some(number) => (number.trim_end(), 1000usize),
        None => (value, 1),
    };
    let tokens = (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| number.parse::<usize>().ok())
        .flatten()
        .and_then(|value| value.checked_mul(multiplier))
        .filter(|value| *value > 0 && i64::try_from(*value).is_ok())
        .ok_or_else(|| {
            "Enter a positive whole token count, such as 128000 or 128k (1k = 1000 tokens)."
                .to_owned()
        })?;
    let mut candidate = policy.clone();
    candidate.context_tokens = tokens;
    candidate.validate().map_err(|error|format!("Invalid context window: {error}. Current output budget: {}; safety margin: {}; summary budget: {} tokens. For a smaller model, reduce these reservations in the configuration before running setup again",policy.output_tokens,policy.safety_tokens,policy.summary_tokens))?;
    Ok(tokens)
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
                Ok(value) if !value.is_empty() => {
                    key_done(label);
                    return Ok(SecretString::new(value));
                }
                _ => eprintln!("Saved key is invalid. Paste a replacement."),
            },
            Ok("") if optional => {
                key_done(label);
                return Ok(SecretString::default());
            }
            Ok("") => eprintln!("An API key is required. Paste it, or press Esc to go back."),
            Ok(value) => {
                key_done(label);
                return Ok(SecretString::new(value));
            }
            Err(message) => eprintln!("{message}"),
        }
    }
}

fn key_done(label: &str) {
    eprint!("\x1b[1A\r\x1b[2K◇ {label}: Configured\n");
    let _ = io::stderr().flush();
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
    eprint!("◆ {label} [{hint}]: ");
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

    let mut input = File::from(
        io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(CliError::io)?,
    );
    collect_password(|timeout| {
        if let Some(timeout) = timeout {
            loop {
                match input_ready(input.as_raw_fd(), timeout) {
                    Ok(false) => return Ok(None),
                    Ok(true) => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(CliError::io(e)),
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
                    return Err(back());
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
                KeyCode::Esc => return Err(back()),
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

    #[cfg(unix)]
    #[test]
    fn loading_arrows_do_not_leak_into_the_next_prompt() {
        use std::{
            io::Write,
            os::{fd::OwnedFd, unix::net::UnixStream},
            sync::{Arc, atomic::AtomicBool},
        };
        for (bytes, expected) in [
            (b"\x1b[B\x03".as_slice(), 130),
            (b"\x1b".as_slice(), super::BACK),
        ] {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            writer.write_all(bytes).unwrap();
            let fd: OwnedFd = reader.into();
            assert_eq!(
                super::loading_key_from_file(
                    std::fs::File::from(fd),
                    Arc::new(AtomicBool::new(false))
                )
                .code,
                expected
            );
        }
    }
    #[test]
    fn context_window_accepts_decimal_k_and_enforces_the_runtime_budget() {
        let policy = kyler_core::context::ContextPolicy::default();
        for (input, expected) in [
            ("128000", 128000),
            ("128k", 128000),
            ("64K", 64000),
            (" 256k ", 256000),
        ] {
            assert_eq!(
                super::parse_context_window(input, &policy).unwrap(),
                expected
            );
        }
        for invalid in [
            "",
            "0",
            "-128000",
            "+128000",
            "1.5k",
            "128KiB",
            "abc",
            "184467440737095516160k",
            "8000",
        ] {
            assert!(
                super::parse_context_window(invalid, &policy).is_err(),
                "{invalid}"
            );
        }
        let mut smaller = policy.clone();
        smaller.output_tokens = 512;
        assert_eq!(super::parse_context_window("32k", &smaller).unwrap(), 32000);
    }

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
        assert_eq!(collect(b"\x1b").unwrap_err().code, super::BACK);
        for cancelled in [b"\x03".as_slice(), b"\x04", b""] {
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
