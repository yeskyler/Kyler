use std::time::Duration;

use serde::{Deserialize, Serialize};

use kyler_protocol::api::inspection::{
    CompactionSnapshot, ContextRequestSnapshot, ModelResponseMetrics,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextMemory {
    #[serde(default)]
    pub last_model_response: Option<ModelResponseMetrics>,
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
    pub fn record_model_response(&mut self, model: &str, output_tokens: u32, elapsed: Duration) {
        let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
        self.last_model_response =
            (output_tokens > 0 && elapsed_ms > 0).then(|| ModelResponseMetrics {
                model: model.into(),
                output_tokens,
                elapsed_ms,
            });
    }

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
    fn model_metrics_preserve_request_scope_and_clear_missing_usage() {
        let mut memory = ContextMemory::default();
        memory.record_model_response("fixture", 120, std::time::Duration::from_secs(2));
        let encoded = serde_json::to_vec(&memory).unwrap();
        let restored: ContextMemory = serde_json::from_slice(&encoded).unwrap();
        let metrics = restored.last_model_response.unwrap();
        assert_eq!(metrics.model, "fixture");
        assert_eq!(metrics.output_tokens, 120);
        assert_eq!(metrics.elapsed_ms, 2000);
        memory.record_model_response("fixture", 0, std::time::Duration::from_secs(1));
        assert!(memory.last_model_response.is_none());
    }

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
