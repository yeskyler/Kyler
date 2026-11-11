mod access;
mod chat;
mod configure;
mod eval;
mod run;
mod serve;
mod service;
mod skills;

use crate::{
    CliError, Result,
    args::{Args, Command, RunCommand, SessionCommand},
    output,
};

use kyler_client::Client;

pub async fn execute(args: Args) -> Result<()> {
    if args.json && matches!(&args.command, Some(Command::Tui { .. })) {
        return Err(CliError::new(2, "--json cannot be combined with tui"));
    }
    // Clap does not consistently apply child conflicts against global parent flags.
    if args.json
        && matches!(
            &args.command,
            Some(Command::Run {
                final_only: true,
                ..
            })
        )
    {
        return Err(CliError::new(
            2,
            "--final-only cannot be combined with --json",
        ));
    }
    if let Some(Command::Configure {
        non_interactive,
        web,
        addresses,
        output,
        apply,
    }) = &args.command
    {
        if let Some(candidate) = apply {
            return configure::apply(args.config.as_deref(), candidate);
        }
        return configure::configure(
            args.config.as_deref(),
            *non_interactive,
            *web,
            *addresses,
            output.as_deref(),
        );
    }
    if let Some(Command::Service { command }) = &args.command {
        return service::execute(args.config.as_deref(), command, args.json).await;
    }
    if let Some(Command::Serve(settings)) = &args.command {
        return serve::serve(args.config.as_deref(), settings).await;
    }
    if let Some(Command::Tui { session }) = &args.command {
        if args.server.is_some() {
            return Err(CliError::new(
                2,
                "tui runs locally; --server is not supported",
            ));
        }
        let config = kyler_config::loader::load_local(args.config.as_deref())
            .map_err(|error| CliError::new(2, error.to_string()))?;
        return kyler_tui::run(config, args.config.clone(), session.clone())
            .await
            .map_err(|error| CliError::new(3, format!("{error:#}")));
    }
    let settings =
        kyler_config::loader::load_client(args.config.as_deref(), args.server.as_deref())
            .map_err(|e| CliError::new(2, e.to_string()))?;
    let client = Client::new(settings)?;
    match args.command.unwrap_or(Command::Chat { session: None }) {
        Command::Skills { command } => skills::execute(&client, command, args.json).await,
        Command::Eval { suite, report } => eval::run(&client, &suite, &report).await,
        Command::Tui { .. } => unreachable!(),
        Command::Chat { session } => chat::chat(&client, session, args.json).await,
        Command::Run {
            session,
            request_id,
            final_only,
            input,
        } => {
            let session = run::session(&client, session).await?;
            let id = run::start(&client, &session, input.join(" "), request_id).await?;
            run::watch(&client, &id, 0, args.json, final_only).await
        }
        Command::Sessions { command } => match command {
            SessionCommand::List { cursor, limit } => {
                let page = client.sessions(cursor, limit).await?;
                if args.json {
                    output::json(&page)
                } else {
                    for s in page.sessions {
                        output::text(&format!(
                            "{}  turns={}  events={}  {:?}\n",
                            s.session_id, s.execution_count, s.event_count, s.status
                        ))?;
                    }
                    if let Some(cursor) = page.next_cursor {
                        eprintln!("Next cursor: {cursor}");
                    }
                    Ok(())
                }
            }
            SessionCommand::Show { id } => {
                let s = client.session(&id).await?;
                if args.json {
                    output::json(&s)
                } else {
                    output::text(&format!(
                        "Session: {}\nTurns: {}\nEvents: {}\nStatus: {:?}\n",
                        s.session_id, s.execution_count, s.event_count, s.status
                    ))
                }
            }
            SessionCommand::Messages { id, cursor, limit } => {
                let page = client.messages(&id, cursor, limit).await?;
                if args.json {
                    output::json(&page)
                } else {
                    for message in page.messages {
                        output::text(&format!("{}:\n{}\n\n", message.role, message.content))?;
                    }
                    if let Some(cursor) = page.next_cursor {
                        eprintln!("Next cursor: {cursor}");
                    }
                    Ok(())
                }
            }
            SessionCommand::Delete { id } => {
                client.delete_session(&id).await?;
                if args.json {
                    output::json(&serde_json::json!({"type":"session_deleted","session_id":id}))
                } else {
                    output::text(&format!("Deleted session {id}\n"))
                }
            }
        },
        Command::Runs { command } => match command {
            RunCommand::Show { id } => {
                let info = client.run(&id).await?;
                if args.json {
                    output::json(&info)
                } else {
                    output::text(&format!(
                        "Run: {}\nSession: {}\nStatus: {:?}\n",
                        info.run_id, info.session_id, info.status
                    ))?;
                    if let Some(answer) = info.output {
                        output::text(&format!("{answer}\n"))?;
                    }
                    if let Some(error) = info.error {
                        eprintln!("{}: {}", error.code, error.message);
                    }
                    Ok(())
                }
            }
            RunCommand::Watch { id, after } => {
                run::watch(&client, &id, after, args.json, false).await
            }
            RunCommand::Cancel { id } => {
                let info = client.cancel(&id).await?;
                if args.json {
                    output::json(&info)
                } else {
                    output::text(&format!("Run: {} ({:?})\n", info.run_id, info.status))
                }
            }
        },
        Command::Serve(_) | Command::Configure { .. } | Command::Service { .. } => unreachable!(),
    }
}
