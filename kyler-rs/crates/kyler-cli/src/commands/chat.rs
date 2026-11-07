use super::run;
use crate::{CliError, Result, output};

use std::io::{self, IsTerminal};

use kyler_client::Client;

pub async fn chat(client: &Client, session: Option<String>, json: bool) -> Result<()> {
    let mut session = run::session(client, session).await?;
    let terminal = io::stdin().is_terminal();
    // A detached OS thread avoids Tokio's uncancellable stdin read blocking runtime shutdown.
    let (send, mut lines) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || {
        loop {
            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if send.blocking_send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = send.blocking_send(Err(error));
                    break;
                }
            }
        }
    });
    if terminal {
        eprintln!("Kyler — /help for commands, /exit to leave.");
    }
    loop {
        if terminal {
            eprint!("you> ");
        }
        let line = tokio::select! {
            line=lines.recv()=>match line {Some(line)=>line.map_err(CliError::io)?,None=>return Ok(())},
            _=tokio::signal::ctrl_c()=>return Ok(()),
        };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            "/exit" | "/quit" => return Ok(()),
            "/help" => {
                eprintln!(
                    "/new       Start a new session\n/session   Show current session\n/exit      Leave chat\nCtrl+C     Cancel the current run; at the prompt, exit"
                );
                continue;
            }
            "/session" => {
                if json {
                    output::json(
                        &serde_json::json!({"type":"client_session","session_id":session}),
                    )?;
                } else {
                    eprintln!("Session: {session}");
                }
                continue;
            }
            "/new" => {
                session = run::session(client, None).await?;
                continue;
            }
            _ if input.starts_with('/') => {
                eprintln!("Unknown command. Use /help.");
                continue;
            }
            _ => {}
        }
        let id = run::start(client, &session, input.into(), None).await?;
        match run::watch(client, &id, 0, json, false).await {
            Ok(()) => {}
            Err(error)
                if error.code == 1
                    || (error.code == 130 && error.message.ends_with(" cancelled.")) =>
            {
                if json {
                    output::json(
                        &serde_json::json!({"type":"client_error","code":error.code,"message":error.message}),
                    )?;
                } else {
                    eprintln!("{error}");
                }
            }
            Err(error) => return Err(error),
        }
    }
}
