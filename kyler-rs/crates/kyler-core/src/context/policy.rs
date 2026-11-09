use anyhow::ensure;

use kyler_config::ContextSettings;
use kyler_protocol::prompts::SUMMARY;

#[derive(Debug, Clone)]
pub struct ContextPolicy {
    /// Must match the deployed model; no model-name inference is performed.
    pub context_tokens: usize,
    pub window_ratio: f64,
    pub trigger_ratio: f64,
    pub output_tokens: u32,
    pub safety_tokens: usize,
    /// Token budget for the stored summary.
    pub summary_tokens: usize,
}
impl Default for ContextPolicy {
    fn default() -> Self {
        Self::from(&ContextSettings::default())
    }
}

impl From<&ContextSettings> for ContextPolicy {
    fn from(c: &ContextSettings) -> Self {
        Self {
            context_tokens: c.context_tokens,
            window_ratio: c.window_ratio,
            trigger_ratio: c.trigger_ratio,
            output_tokens: c.output_tokens,
            safety_tokens: c.safety_tokens,
            summary_tokens: c.summary_tokens,
        }
    }
}

impl ContextPolicy {
    /// Target history capacity retained by the sliding window, in tokens.
    pub fn sliding_window_tokens(&self) -> usize {
        (self.context_tokens as f64 * self.window_ratio) as usize
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.window_ratio > 0.0
                && self.window_ratio < self.trigger_ratio
                && self.trigger_ratio < 1.0,
            "context ratios must satisfy 0 < window < trigger < 1"
        );
        ensure!(
            self.output_tokens > 0
                && self.summary_tokens > 0
                && self.summary_tokens <= u32::MAX as usize,
            "output and summary budgets must be positive and fit u32"
        );
        let reserved = (self.output_tokens as usize)
            .checked_add(self.safety_tokens)
            .and_then(|n| {
                self.summary_tokens
                    .checked_mul(2)
                    .and_then(|summary| n.checked_add(summary))
            })
            .and_then(|n| n.checked_add(SUMMARY.len()))
            .and_then(|n| n.checked_add(1024));
        ensure!(
            reserved.is_some_and(|n| self.context_tokens > n),
            "context budget is too small for output, safety margin and summary"
        );
        Ok(())
    }
    pub fn input_limit(&self) -> usize {
        ((self.context_tokens as f64 * self.trigger_ratio) as usize)
            .min(self.context_tokens - self.output_tokens as usize - self.safety_tokens)
    }
}
