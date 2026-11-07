use crate::{
    context::{Summarizer, SummaryResult},
    models::error::ModelError,
};

use std::time::Duration;

use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
};

use kyler_config::OutputTokenParameter;
use kyler_protocol::prompts;

pub(crate) struct ModelSummarizer<'a> {
    pub(crate) observer: Option<&'a dyn crate::runtime::observation::RuntimeObserver>,
    pub(crate) client: &'a Client<OpenAIConfig>,
    pub(crate) output_tokens: u32,
    pub(crate) output_token_parameter: OutputTokenParameter,
    pub(crate) model: &'a str,
}

#[async_trait::async_trait]
impl Summarizer for ModelSummarizer<'_> {
    async fn summarize(&mut self, input: &str, max_tokens: u32) -> anyhow::Result<SummaryResult> {
        let max_tokens = max_tokens.min(self.output_tokens);
        let mut request = CreateChatCompletionRequestArgs::default()
            .model(self.model)
            .messages(vec![
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(prompts::SUMMARY)
                    .build()?
                    .into(),
                ChatCompletionRequestUserMessageArgs::default()
                    .content(prompts::summary_request(input, max_tokens))
                    .build()?
                    .into(),
            ])
            .build()?;
        crate::models::set_output_limit(&mut request, self.output_token_parameter, max_tokens);
        if let Some(observer) = self.observer {
            observer.observe(crate::runtime::observation::RuntimeObservation::ModelRequestStarted);
        }
        let response =
            tokio::time::timeout(Duration::from_secs(60), self.client.chat().create(request))
                .await
                .map_err(|error| ModelError::wrap(error.into()))?
                .map_err(|error| ModelError::wrap(error.into()))?;
        if let Some(observer) = self.observer {
            observer.observe(crate::runtime::observation::RuntimeObservation::FirstModelEvent);
        }
        let usage = response
            .usage
            .map(|u| (u.prompt_tokens, u.completion_tokens, u.total_tokens));
        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("empty context summary"))?;
        Ok(SummaryResult { text, usage })
    }
}
