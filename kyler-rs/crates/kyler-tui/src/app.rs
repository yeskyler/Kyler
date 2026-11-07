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

use kyler_client::Client;

pub(crate) use crate::{input::handle_input, update::apply};

pub async fn run(client: Client, session: Option<String>) -> Result<()> {
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
                    state.notice("Cancellation not confirmed. F5 to check or F10 to exit; the task may still be running.");
                }
                if matches!(state.phase, Phase::Submitting | Phase::Running) { state.dirty = true; }
                if state.dirty {terminal.draw(|frame|view.render(frame,&mut state))?;state.dirty=false;}
            }
        }
    }
    // Restore before printing diagnostics; dropping subscriptions never cancels server work.
    drop(effects);
    drop(terminal);
    drop(guard);
    if state.pending.is_some() || matches!(state.phase, Phase::Running | Phase::Cancelling) {
        eprintln!(
            "Exited TUI; task completion / cancellation unconfirmed. Session: {} · Run: {} · Request: {}",
            state.session.as_deref().unwrap_or("—"),
            state.run.as_deref().unwrap_or("unconfirmed"),
            state
                .pending
                .as_ref()
                .map(|p| p.key.as_str())
                .unwrap_or("—")
        );
    }
    Ok(())
}
