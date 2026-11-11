use crate::agent::AgentHandle;
use crate::{
    effects::Effects,
    layout::View,
    state::{Phase, State},
    terminal::TerminalGuard,
};

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::EventStream;
use futures::StreamExt;
use tokio::time::MissedTickBehavior;

pub(crate) use crate::{input::handle_input, update::apply};

pub async fn run(
    config: kyler_config::AppConfig,
    source: Option<std::path::PathBuf>,
    session: Option<String>,
) -> Result<()> {
    let client = AgentHandle::new(config, source);
    let result = run_terminal(client.clone(), session).await;
    let cleanup = client.shutdown().await;
    result.and(cleanup)
}

async fn run_terminal(client: AgentHandle, session: Option<String>) -> Result<()> {
    let (guard, mut terminal) = TerminalGuard::enter()?;
    let mut state = State::new(session);
    let mut view = View::default();
    let mut effects = Effects::default();
    let (send, mut receive) = tokio::sync::mpsc::channel(128);
    effects.load(
        client.clone(),
        state.session.clone(),
        state.generation,
        send.clone(),
    );
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event=input.next()=>match event {
                Some(Ok(event))=>if handle_input(event,&mut state,&mut effects,&client,&send,&view) {break;},
                Some(Err(error))=>return Err(error.into()),None=>break,
            },
            Some((generation,event))=receive.recv()=>{
                apply(generation,event,&mut state,&mut effects,&client,&send);
                for _ in 0..63 {match receive.try_recv(){Ok((generation,event))=>apply(generation,event,&mut state,&mut effects,&client,&send),Err(_)=>break}}
            },
            _=tick.tick()=>{
                state.advance_messages();
                if state.cancel_deadline.is_some_and(|until|Instant::now()>=until) {
                    state.cancel_deadline=None;state.phase=Phase::Detached;
                    state.notice("The local task is still stopping. Wait for cleanup or press F10 to exit.");
                }
                if matches!(state.phase, Phase::Submitting | Phase::Running) { state.dirty = true; }
                if state.dirty {terminal.draw(|frame|view.render(frame,&mut state))?;state.dirty=false;}
            }
        }
    }
    // Restore the terminal before joining local agent cleanup.
    drop(effects);
    drop(terminal);
    drop(guard);
    Ok(())
}
