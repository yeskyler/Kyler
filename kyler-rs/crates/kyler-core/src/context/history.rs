use serde::{Deserialize, Serialize};

use kyler_protocol::api::inspection::{CompactionSnapshot, ContextRequestSnapshot};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextMemory {
    #[serde(default)]
    pub last_request: Option<ContextRequestSnapshot>,
    #[serde(default)]
    pub last_compaction: Option<CompactionSnapshot>,
    #[serde(default)]
    pub estimation_scale: f64,
    #[serde(default)]
    pub last_provider_input_tokens: usize,
    pub summary: Option<String>,
    /// Events before this index are summarized except retained questions.
    pub window_start: usize,
    pub current_question_index: usize,
    /// Questions skipped by earlier compaction, awaiting the next summary.
    pub retained_questions: Vec<usize>,
    pub compression_count: usize,
    pub last_input_tokens: usize,
}

impl ContextMemory {
    /// Update the current request without calibrating against an intermediate stream snapshot.
    pub fn record_prompt_tokens(&mut self, actual: u32) {
        self.last_provider_input_tokens = actual as usize;
        if let Some(request) = &mut self.last_request {
            request.provider_input_tokens = (actual > 0).then_some(actual as usize);
        }
    }

    pub fn observe_prompt_tokens(&mut self, actual: u32) {
        self.record_prompt_tokens(actual);
        if self.last_input_tokens > 0 && actual > 0 {
            let observed =
                self.estimation_scale.max(1.0) * actual as f64 / self.last_input_tokens as f64;
            self.estimation_scale =
                (self.estimation_scale.max(1.0) * 0.75 + observed * 1.05 * 0.25).clamp(1.0, 4.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContextMemory;

    #[test]
    fn intermediate_snapshots_do_not_repeatedly_calibrate_estimates() {
        let mut memory = ContextMemory {
            last_input_tokens: 1000,
            estimation_scale: 1.0,
            ..Default::default()
        };
        memory.record_prompt_tokens(2000);
        memory.record_prompt_tokens(3000);
        assert_eq!(memory.estimation_scale, 1.0);
        memory.observe_prompt_tokens(3000);
        assert!((memory.estimation_scale - 1.5375).abs() < 0.00001);
    }
}
