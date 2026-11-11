//! Retain source context across network deltas and publish a bounded amount per UI tick.
use std::collections::VecDeque;

use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Default)]
pub struct MarkdownStreamCollector {
    source: String,
    complete_lines: VecDeque<usize>,
}
impl MarkdownStreamCollector {
    fn push(&mut self, delta: &str) {
        let offset = self.source.len();
        self.complete_lines.extend(
            delta
                .match_indices('\n')
                .map(|(index, _)| offset + index + 1),
        );
        self.source.push_str(delta);
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentMessageCell {
    collector: MarkdownStreamCollector,
    visible: usize,
    finished: bool,
    pub final_separator: bool,
}
impl AgentMessageCell {
    pub fn complete(content: String) -> Self {
        let mut cell = Self::default();
        cell.finish(Some(content));
        cell
    }
    pub fn push(&mut self, delta: &str) {
        self.finished = false;
        self.collector.push(delta);
    }
    pub fn source(&self) -> &str {
        &self.collector.source
    }
    pub fn displayed(&self) -> &str {
        &self.source()[..self.visible]
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Commit up to four complete lines. An unfinished line stays an active preview;
    /// advancing it in grapheme batches keeps long single-line replies responsive.
    pub fn advance(&mut self) -> bool {
        let previous = self.visible;
        for _ in 0..4 {
            let Some(&end) = self.collector.complete_lines.front() else {
                break;
            };
            if end.saturating_sub(previous) > 8192 {
                break;
            }
            self.collector.complete_lines.pop_front();
            self.visible = self.visible.max(end);
        }
        if self.visible == previous {
            let remaining = &self.collector.source[self.visible..];
            self.visible += remaining
                .grapheme_indices(true)
                .nth(160)
                .map(|(index, _)| index)
                .unwrap_or(remaining.len());
            while self
                .collector
                .complete_lines
                .front()
                .is_some_and(|end| *end <= self.visible)
            {
                self.collector.complete_lines.pop_front();
            }
        }
        self.visible != previous
    }
    /// Completion, activity boundaries and cancellation must never lose the tail.
    /// The final snapshot reconciles this same cell instead of adding a duplicate.
    pub fn finish(&mut self, authoritative: Option<String>) {
        if let Some(content) = authoritative {
            self.collector.source = content;
        }
        self.visible = self.collector.source.len();
        self.collector.complete_lines.clear();
        self.finished = true;
    }
}

/// Reasoning has a bounded live preview and retains its complete text for inspection.
#[derive(Debug, Clone)]
pub struct ReasoningCell {
    pub content: String,
    pub streaming: bool,
    pub expanded: Option<bool>,
    pub seconds: Option<u64>,
    started: Option<std::time::Instant>,
}
impl ReasoningCell {
    pub fn streaming(content: String) -> Self {
        Self {
            content,
            streaming: true,
            expanded: None,
            seconds: None,
            started: Some(std::time::Instant::now()),
        }
    }
    pub fn complete(content: String) -> Self {
        Self {
            content,
            streaming: false,
            expanded: None,
            seconds: None,
            started: None,
        }
    }
    pub fn open(&self) -> bool {
        self.expanded.unwrap_or(self.streaming)
    }
    pub fn toggle(&mut self) {
        self.expanded = Some(!self.open());
    }
    pub fn finish(&mut self) {
        self.streaming = false;
        if let Some(started) = self.started.take() {
            self.seconds = Some(started.elapsed().as_secs());
        }
    }
}
