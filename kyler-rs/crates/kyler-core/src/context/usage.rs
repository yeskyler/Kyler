//! Provider-reported token usage; independent of context size estimates.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn add(&mut self, prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) {
        self.prompt_tokens += prompt_tokens;
        self.completion_tokens += completion_tokens;
        self.total_tokens += total_tokens;
    }
}
