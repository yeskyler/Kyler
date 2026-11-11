use super::{model_failure, ready, render};
use crate::agent::AgentHandle;
use crate::{
    app,
    attachments::Paste,
    effects::{Effects, Event},
    layout::View,
    state::{Entry, Phase},
};

use std::time::Duration;

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn image_only_draft_renders_in_composer_and_is_retained_for_retry() {
    let mut state = ready();
    let image = crate::attachments::from_rgba(1, 1, &[0, 0, 0, 255]).unwrap();
    let path = image.path.clone();
    state.attachments.push(image);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let screen = render(&mut terminal, &mut view, &mut state);
    assert!(screen.contains("[Attachment: screenshot.png]"));
    assert_eq!(view.input.width, view.transcript.width);
    let submission = state.begin().unwrap();
    assert!(submission.input.is_empty());
    assert_eq!(submission.attachments.len(), 1);
    assert!(state.attachments.is_empty());
    assert!(path.exists());
    assert!(
        matches!(&state.entries[0], Entry::Message { content, .. } if content.contains("[Attachment: screenshot.png]"))
    );
    drop(submission);
    assert!(
        state.pending.as_ref().unwrap().attachments[0]
            .read_bytes()
            .is_ok()
    );
    state.pending = None;
    assert!(!path.exists());
}

#[test]
fn stale_image_results_are_dropped_and_images_can_be_removed() {
    let client = AgentHandle::new(Default::default(), None);
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(128);
    let mut state = ready();
    let image = crate::attachments::from_rgba(1, 1, &[0, 0, 0, 255]).unwrap();
    let path = image.path.clone();
    app::apply(
        0,
        Event::ImagePaste(99, Ok(Paste::Image(image))),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    assert!(state.attachments.is_empty());
    assert!(!path.exists());
    state
        .attachments
        .push(crate::attachments::from_rgba(1, 1, &[0, 0, 0, 255]).unwrap());
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &View::default(),
    );
    assert!(state.attachments.is_empty());
    state.image_busy = true;
    state.composer.insert_str("wait");
    assert!(state.begin().is_none());
}

#[test]
fn model_failure_is_rendered_and_restores_text_and_images_for_retry() {
    let mut state = ready();
    state.composer.insert_str("Describe this screenshot");
    state
        .attachments
        .push(crate::attachments::from_rgba(1, 1, &[0, 0, 0, 255]).unwrap());
    let first = state.begin().unwrap();
    state.run = Some("run".into());
    state.stream(model_failure());
    assert_eq!(state.phase, Phase::Ready);
    assert_eq!(state.composer.lines().join("\n"), first.input);
    assert_eq!(state.attachments.len(), 1);
    assert!(state.pending.is_none());
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let screen = render(&mut terminal, &mut view, &mut state);
    assert!(screen.contains("Model error:"));
    assert!(screen.contains("does not support image input"));
    assert!(!screen.contains("tool execution failed"));
    let retry = state.begin().unwrap();
    assert_eq!(
        first.attachments[0].read_bytes().unwrap(),
        retry.attachments[0].read_bytes().unwrap()
    );
}

#[test]
fn model_failure_preserves_a_new_draft_and_retains_failed_images_until_restored() {
    let mut state = ready();
    state
        .attachments
        .push(crate::attachments::from_rgba(1, 1, &[0, 0, 0, 255]).unwrap());
    let first = state.begin().unwrap();
    state.run = Some("run".into());
    state.composer.insert_str("New draft");
    state.stream(model_failure());
    assert_eq!(state.composer.lines().join("\n"), "New draft");
    assert!(state.attachments.is_empty());
    assert_eq!(
        state.failed_submission.as_ref().unwrap().attachments.len(),
        1
    );
    assert!(!state.restore_failed_submission());
    state.composer = crate::components::composer::new();
    assert!(state.restore_failed_submission());
    assert_eq!(state.attachments[0].path, first.attachments[0].path);
    assert!(state.failed_submission.is_none());
}

#[tokio::test]
async fn at_picker_attaches_a_local_document_without_sending_a_model_request() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("report.txt");
    std::fs::write(&file, "report contents").unwrap();
    let client = AgentHandle::new(Default::default(), None);
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, mut receive) = tokio::sync::mpsc::channel(8);
    state
        .composer
        .insert_str(format!("Analyze @{}/rep", root.path().display()));
    let items = crate::components::attachment_picker::candidates(&state);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "report.txt");
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut view = View::default();
    let screen = render(&mut terminal, &mut view, &mut state);
    assert!(screen.contains("Attachments") && screen.contains("report.txt"));
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    let (generation, event) = tokio::time::timeout(Duration::from_secs(2), receive.recv())
        .await
        .unwrap()
        .unwrap();
    app::apply(generation, event, &mut state, &mut effects, &client, &send);
    assert_eq!(state.composer.lines().join("\n"), "Analyze ");
    assert_eq!(state.attachments[0].name, "report.txt");
    assert!(state.pending.is_none());
    assert_eq!(
        state.attachments[0].read_bytes().unwrap(),
        b"report contents"
    );
}
