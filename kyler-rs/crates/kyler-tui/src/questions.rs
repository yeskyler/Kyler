//! Explicit clarification above the composer. Enter confirms; Escape dismisses.
use crate::agent::AgentHandle;
use crate::{
    components::{
        composer::clean,
        panel::{self, Position},
        transcript,
    },
    effects::{Effects, Sender},
    state::{Entry, State},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kyler_protocol::api::questions::*;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
pub struct QuestionUi {
    pub records: Vec<QuestionRecord>,
    pub index: usize,
    pub selected: usize,
    pub checked: HashSet<String>,
    pub text: String,
    pub answers: BTreeMap<String, QuestionAnswer>,
    pub busy: bool,
    pub error: Option<String>,
    shown: HashSet<String>,
}
impl QuestionUi {
    pub fn reset_history(&mut self) {
        self.shown.clear();
    }
    pub fn pending(&self) -> Option<&QuestionRecord> {
        self.records
            .iter()
            .find(|record| record.status == QuestionStatus::Pending)
    }
    pub fn replace(&mut self, records: Vec<QuestionRecord>) {
        let old = self.pending().map(|record| record.id.clone());
        let mut records = records;
        for record in &mut records {
            if record.status == QuestionStatus::Pending {
                if let Some(previous) = self.records.iter().find(|previous| {
                    previous.id == record.id && previous.status != QuestionStatus::Pending
                }) {
                    *record = previous.clone();
                }
            }
        }
        self.records = records;
        if old != self.pending().map(|record| record.id.clone()) {
            self.index = 0;
            self.selected = 0;
            self.checked.clear();
            self.text.clear();
            self.answers.clear();
            self.busy = false;
            self.error = None;
        }
    }
    pub fn update(&mut self, record: QuestionRecord) {
        let mut records = self.records.clone();
        records.retain(|item| item.id != record.id);
        records.push(record);
        self.replace(records);
    }
}
pub fn history(state: &mut State) {
    if state.questions.pending().is_some() {
        state.tool_details = None;
        state.help = false;
        state.model_open = false;
        state.permissions_open = false;
        state.mcp_open = false;
        state.skills_open = false;
        state.approval_details = None;
    }
    for record in &state.questions.records {
        if record.status == QuestionStatus::Pending || state.questions.shown.contains(&record.id) {
            continue;
        }
        let Some(index) = state.entries.iter().rposition(|entry| matches!(entry, Entry::Tool { id, name, .. } if id == &record.tool_call_id && name == "ask_question")) else { continue; };
        let text = if record.status == QuestionStatus::Answered {
            let answers = record
                .questions
                .iter()
                .filter_map(|question| {
                    let answer = record.answers.get(&question.id)?;
                    let mut parts = question
                        .options
                        .iter()
                        .filter(|option| answer.selected.contains(&option.id))
                        .map(|option| option.label.clone())
                        .collect::<Vec<_>>();
                    if !answer.text.trim().is_empty() {
                        parts.push(answer.text.clone());
                    }
                    Some(format!("{}: {}", question.question, parts.join(", ")))
                })
                .collect::<Vec<_>>()
                .join(" · ");
            format!("Answered · {answers}")
        } else {
            format!(
                "Question {}",
                match record.status {
                    QuestionStatus::Dismissed => "dismissed",
                    QuestionStatus::Interrupted => "interrupted",
                    _ => "cancelled",
                }
            )
        };
        state.entries.insert(index + 1, Entry::Notice(text));
        state.questions.shown.insert(record.id.clone());
        if state.assistant.is_some_and(|assistant| assistant > index) {
            state.assistant = state.assistant.map(|assistant| assistant + 1);
        }
        state.transcript_version += 1;
    }
}
fn send_response(
    response: QuestionResponse,
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
) {
    if state.questions.busy {
        return;
    }
    if let (Some(session), Some(record)) = (state.session.clone(), state.questions.pending()) {
        let id = record.id.clone();
        state.questions.busy = true;
        state.questions.error = None;
        effects.answer_question(
            client.clone(),
            session,
            id,
            response,
            state.generation,
            send.clone(),
        );
    }
}
pub fn keyboard(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
) -> bool {
    if state.pending_approval().is_some()
        || state.questions.pending().is_none()
        || key.modifiers.contains(KeyModifiers::CONTROL)
        || matches!(key.code, KeyCode::F(5 | 10))
    {
        return false;
    }
    if state.questions.busy {
        return true;
    }
    if key.code == KeyCode::Esc {
        send_response(QuestionResponse::Cancel, state, effects, client, send);
        return true;
    }
    let record = state.questions.pending().unwrap().clone();
    let question = &record.questions[state.questions.index];
    let custom = state.questions.selected == question.options.len();
    match key.code {
        KeyCode::Up => state.questions.selected = state.questions.selected.saturating_sub(1),
        KeyCode::Down => {
            state.questions.selected = (state.questions.selected + 1).min(question.options.len())
        }
        KeyCode::Backspace if custom => {
            state.questions.text.pop();
        }
        KeyCode::Char(ch) if custom => {
            if state.questions.text.len() + ch.len_utf8() <= 4000 {
                state.questions.text.push(ch);
            }
        }
        KeyCode::Char(' ') if question.kind == QuestionType::MultiSelect => {
            let id = question.options[state.questions.selected].id.clone();
            if !state.questions.checked.remove(&id) {
                state.questions.checked.insert(id);
            }
        }
        KeyCode::Enter => {
            let answer = if custom {
                if state.questions.text.trim().is_empty() {
                    state.questions.error = Some("Enter a custom answer.".into());
                    return true;
                }
                QuestionAnswer {
                    selected: Vec::new(),
                    text: state.questions.text.clone(),
                }
            } else if question.kind == QuestionType::MultiSelect {
                if state.questions.checked.is_empty() {
                    state.questions.error =
                        Some("Select at least one option or enter a custom answer.".into());
                    return true;
                }
                QuestionAnswer {
                    selected: question
                        .options
                        .iter()
                        .filter(|option| state.questions.checked.contains(&option.id))
                        .map(|option| option.id.clone())
                        .collect(),
                    text: String::new(),
                }
            } else {
                QuestionAnswer {
                    selected: vec![question.options[state.questions.selected].id.clone()],
                    text: String::new(),
                }
            };
            state.questions.answers.insert(question.id.clone(), answer);
            if state.questions.index + 1 == record.questions.len() {
                send_response(
                    QuestionResponse::Submit {
                        answers: state.questions.answers.clone(),
                    },
                    state,
                    effects,
                    client,
                    send,
                );
            } else {
                state.questions.index += 1;
                state.questions.selected = 0;
                state.questions.checked.clear();
                state.questions.text.clear();
                state.questions.error = None;
            }
        }
        _ => {}
    }
    true
}
pub fn paste(text: &str, state: &mut State) {
    if let Some(record) = state.questions.pending() {
        let question = &record.questions[state.questions.index];
        if state.questions.selected == question.options.len() && !state.questions.busy {
            for ch in clean(text).chars() {
                if state.questions.text.len() + ch.len_utf8() > 4000 {
                    break;
                }
                state.questions.text.push(ch);
            }
        }
    }
}
pub fn render(frame: &mut Frame, state: &mut State, input: Rect) -> Rect {
    let Some(record) = state.questions.pending() else {
        return Rect::default();
    };
    let question = &record.questions[state.questions.index];
    let width = input.width.saturating_sub(2);
    let mut lines = transcript::wrap(vec![Line::from(clean(&question.question))], width);
    let mut selected_row = lines.len();
    for (index, option) in question.options.iter().enumerate() {
        if state.questions.selected == index {
            selected_row = lines.len();
        }
        let mark = if question.kind == QuestionType::MultiSelect {
            if state.questions.checked.contains(&option.id) {
                "[x]"
            } else {
                "[ ]"
            }
        } else {
            "○"
        };
        lines.extend(transcript::wrap(
            vec![panel::selected(
                format!(
                    "{mark} {}{}",
                    clean(&option.label),
                    if option.recommended {
                        " (Recommended)"
                    } else {
                        ""
                    }
                ),
                state.questions.selected == index,
            )],
            width,
        ));
    }
    if state.questions.selected == question.options.len() {
        selected_row = lines.len();
    }
    lines.extend(transcript::wrap(
        vec![panel::selected(
            format!("Custom answer: {}", clean(&state.questions.text)),
            state.questions.selected == question.options.len(),
        )],
        width,
    ));
    if let Some(error) = &state.questions.error {
        lines.extend(transcript::wrap(
            vec![Line::styled(error.clone(), Style::default().fg(Color::Red))],
            width,
        ));
        selected_row = lines.len().saturating_sub(1);
    }
    let title = format!(
        "Question {} / {}",
        state.questions.index + 1,
        record.questions.len()
    );
    let hint = if state.questions.busy {
        "Submitting…"
    } else if question.kind == QuestionType::MultiSelect {
        "↑↓ Select · Space Toggle · Enter Confirm · Esc Cancel"
    } else {
        "↑↓ Select · Enter Confirm · Esc Cancel"
    };
    panel::draw(
        frame,
        input,
        &title,
        lines,
        hint,
        true,
        Position::Selected(selected_row),
    )
    .0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> QuestionRecord {
        QuestionRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            execution_id: "execution".into(),
            tool_call_id: "call".into(),
            questions: vec![Question {
                id: "format".into(),
                question: "Choose a format".into(),
                kind: QuestionType::SingleSelect,
                options: vec![QuestionOption {
                    id: "pdf".into(),
                    label: "PDF".into(),
                    recommended: true,
                }],
            }],
            status: QuestionStatus::Pending,
            answers: BTreeMap::new(),
            created_at: 100,
            resolved_at: None,
        }
    }
    #[tokio::test]
    async fn enter_submits_custom_answer_and_escape_cancels_without_touching_draft() {
        let client = AgentHandle::new(Default::default(), None);
        let mut effects = Effects::default();
        let (send, _receive) = tokio::sync::mpsc::channel(16);
        let mut state = State::new(None);
        let question = record();
        state.session = Some(question.session_id.clone());
        state.composer.insert_str("existing draft");
        state.questions.replace(vec![question]);
        keyboard(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut state,
            &mut effects,
            &client,
            &send,
        );
        paste("PowerPoint", &mut state);
        keyboard(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut effects,
            &client,
            &send,
        );
        assert!(state.questions.busy);
        assert_eq!(state.questions.answers["format"].text, "PowerPoint");
        assert_eq!(state.composer.lines(), &["existing draft"]);
        state.questions.busy = false;
        keyboard(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut state,
            &mut effects,
            &client,
            &send,
        );
        assert!(state.questions.busy);
    }
    #[test]
    fn resolved_questions_are_monotone_and_render_above_composer() {
        use ratatui::{Terminal, backend::TestBackend};
        let question = record();
        let mut state = State::new(Some(question.session_id.clone()));
        state.phase = crate::state::Phase::Running;
        state.questions.replace(vec![question.clone()]);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut view = crate::layout::View::default();
        terminal
            .draw(|frame| view.render(frame, &mut state))
            .unwrap();
        assert_eq!(view.panel.bottom(), view.input.y);
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            text.contains("Question 1 / 1")
                && text.contains("Custom answer")
                && text.contains("Enter Confirm")
                && text.contains("Esc Cancel")
        );
        let mut answered = question.clone();
        answered.status = QuestionStatus::Answered;
        state.questions.update(answered);
        state.questions.replace(vec![question]);
        assert!(state.questions.pending().is_none());
    }
}
