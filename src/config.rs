use std::{env, fmt, time::Duration};

use anyhow::{bail, Context, Result};
use clap::Parser;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_MAX_TOKENS: u32 = 4096;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;

#[derive(Parser, Debug)]
#[command(name = "tinychat", version, about)]
pub struct Cli {
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long)]
    pub api_key: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub models: Option<Vec<String>>,
    #[arg(long)]
    pub system_prompt: Option<String>,
    #[arg(long)]
    pub temperature: Option<f32>,
    #[arg(long)]
    pub max_tokens: Option<u32>,
    #[arg(long)]
    pub timeout: Option<u64>,
}

#[derive(Clone)]
pub struct AppConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub models: Vec<String>,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout: Duration,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("models", &self.models)
            .field("system_prompt", &self.system_prompt)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl AppConfig {
    pub fn load(cli: Cli) -> Result<Self> {
        let base_url = cli
            .base_url
            .or_else(|| env::var("OPENAI_BASE_URL").ok())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let api_key = cli
            .api_key
            .or_else(|| env::var("OPENAI_API_KEY").ok())
            .unwrap_or_default();
        let model = cli
            .model
            .or_else(|| env::var("OPENAI_MODEL").ok())
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
            .trim()
            .to_owned();

        let raw_models = cli.models.or_else(|| {
            env::var("OPENAI_MODELS")
                .ok()
                .map(|value| value.split(',').map(str::to_owned).collect())
        });
        let mut models = parse_models(raw_models.unwrap_or_default());
        if model.is_empty() {
            bail!("model must not be empty");
        }
        if !models.iter().any(|candidate| candidate == &model) {
            models.insert(0, model.clone());
        }

        let system_prompt = cli
            .system_prompt
            .or_else(|| env::var("OPENAI_SYSTEM_PROMPT").ok())
            .unwrap_or_default();
        let temperature = value_or_env(cli.temperature, "OPENAI_TEMPERATURE", DEFAULT_TEMPERATURE)?;
        let max_tokens = value_or_env(cli.max_tokens, "OPENAI_MAX_TOKENS", DEFAULT_MAX_TOKENS)?;
        let timeout_seconds = value_or_env(
            cli.timeout,
            "OPENAI_TIMEOUT_SECONDS",
            DEFAULT_TIMEOUT_SECONDS,
        )?;

        if base_url.trim().is_empty() {
            bail!("base URL must not be empty");
        }
        if !temperature.is_finite() {
            bail!("temperature must be a finite number");
        }
        if max_tokens == 0 {
            bail!("max tokens must be greater than zero");
        }
        if timeout_seconds == 0 {
            bail!("timeout must be greater than zero");
        }

        Ok(Self {
            base_url: normalize_base_url(&base_url),
            api_key,
            model,
            models,
            system_prompt,
            temperature,
            max_tokens,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }

    pub fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

fn value_or_env<T>(cli: Option<T>, name: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match cli {
        Some(value) => Ok(value),
        None => match env::var(name) {
            Ok(value) => value
                .parse()
                .with_context(|| format!("{name} has an invalid value")),
            Err(_) => Ok(default),
        },
    }
}

pub fn normalize_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_owned()
}

pub fn parse_models(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for model in values {
        let model = model.trim();
        if !model.is_empty() && !result.iter().any(|existing| existing == model) {
            result.push(model.to_owned());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_url() {
        assert_eq!(
            normalize_base_url(" https://api.openai.com/v1/// "),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("http://127.0.0.1:1234/v1"),
            "http://127.0.0.1:1234/v1"
        );
    }

    #[test]
    fn parses_and_deduplicates_models() {
        assert_eq!(
            parse_models(vec![
                " alpha ".into(),
                String::new(),
                "beta".into(),
                "alpha".into(),
                "  ".into()
            ]),
            vec!["alpha", "beta"]
        );
    }
}
