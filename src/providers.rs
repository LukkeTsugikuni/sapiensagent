use crate::config::{AppConfig, ProviderConfig, find_provider};
use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::time::sleep;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// An image supplied to a multimodal provider. The payload is kept in memory
/// only for the duration of the request and is never written to logs or
/// receipts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageInput {
    pub mime_type: String,
    pub data_base64: String,
}

const MAX_IMAGES: usize = 4;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

pub fn image_input_from_path(path: &Path) -> Result<ImageInput> {
    let metadata =
        fs::metadata(path).with_context(|| format!("read image metadata {}", path.display()))?;
    if !metadata.is_file() {
        bail!("image path is not a regular file");
    }
    if metadata.len() > MAX_IMAGE_BYTES as u64 {
        bail!("image exceeds the 10 MiB limit");
    }
    let mime_type = image_mime_type(path)?;
    let bytes = fs::read(path).with_context(|| format!("read image {}", path.display()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!("image exceeds the 10 MiB limit");
    }
    Ok(ImageInput {
        mime_type,
        data_base64: base64_encode(&bytes),
    })
}

fn image_mime_type(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => bail!("unsupported image extension; use png, jpeg, webp or gif"),
    };
    Ok(mime.into())
}

fn validate_images(images: &[ImageInput]) -> Result<()> {
    if images.len() > MAX_IMAGES {
        bail!("a request cannot contain more than {MAX_IMAGES} images");
    }
    for image in images {
        if !matches!(
            image.mime_type.trim().to_ascii_lowercase().as_str(),
            "image/jpeg" | "image/jpg" | "image/png" | "image/webp" | "image/gif"
        ) {
            bail!("unsupported image MIME type");
        }
        if image.data_base64.is_empty() || image.data_base64.len() > MAX_IMAGE_BYTES * 2 {
            bail!("image data is empty or exceeds the encoded size limit");
        }
        if !valid_base64(&image.data_base64) {
            bail!("image data is not valid Base64");
        }
    }
    Ok(())
}

fn valid_base64(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return false;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 || bytes[..bytes.len() - padding].contains(&b'=') {
        return false;
    }
    bytes[..bytes.len() - padding]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/'))
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderAttempt {
    pub alias: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatOutcome {
    pub answer: String,
    pub provider: String,
    pub latency_ms: u128,
    pub estimated_cost_usd: f64,
    pub cost_source: String,
    pub attempts: Vec<ProviderAttempt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealth {
    pub alias: String,
    pub protocol: String,
    pub ok: bool,
    pub latency_ms: u128,
    pub status: Option<u16>,
}

#[derive(Debug, Clone)]
struct CircuitState {
    failures: u8,
    open_until: Option<Instant>,
}

#[derive(Debug, Clone)]
struct CompletionResult {
    text: String,
    estimated_cost_usd: f64,
    cost_source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub protocols: &'static str,
    pub credential_hint: &'static str,
    pub adapter: &'static str,
}

const CATALOG: &[ProviderSpec] = &[
    ProviderSpec {
        id: "openai",
        label: "OpenAI",
        protocols: "responses,chat_completions",
        credential_hint: "OPENAI_API_KEY",
        adapter: "real (Responses + compatível)",
    },
    ProviderSpec {
        id: "anthropic",
        label: "Anthropic/Claude",
        protocols: "anthropic_messages",
        credential_hint: "ANTHROPIC_API_KEY",
        adapter: "real (Messages API)",
    },
    ProviderSpec {
        id: "gemini",
        label: "Google Gemini",
        protocols: "gemini",
        credential_hint: "GEMINI_API_KEY",
        adapter: "real (Generate Content API)",
    },
    ProviderSpec {
        id: "glm",
        label: "Zhipu/GLM",
        protocols: "chat_completions",
        credential_hint: "GLM_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "deepseek",
        label: "DeepSeek",
        protocols: "chat_completions",
        credential_hint: "DEEPSEEK_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "qwen",
        label: "Qwen/DashScope",
        protocols: "chat_completions",
        credential_hint: "DASHSCOPE_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "openrouter",
        label: "OpenRouter",
        protocols: "chat_completions",
        credential_hint: "OPENROUTER_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "groq",
        label: "Groq",
        protocols: "chat_completions",
        credential_hint: "GROQ_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "cerebras",
        label: "Cerebras Inference",
        protocols: "chat_completions",
        credential_hint: "CEREBRAS_API_KEY",
        adapter: "real (OpenAI-compatible)",
    },
    ProviderSpec {
        id: "mistral",
        label: "Mistral",
        protocols: "chat_completions",
        credential_hint: "MISTRAL_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "moonshot",
        label: "Moonshot/Kimi",
        protocols: "chat_completions",
        credential_hint: "MOONSHOT_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "minimax",
        label: "MiniMax",
        protocols: "chat_completions",
        credential_hint: "MINIMAX_API_KEY",
        adapter: "opcional",
    },
    ProviderSpec {
        id: "nvidia",
        label: "NVIDIA NIM",
        protocols: "chat_completions",
        credential_hint: "NVIDIA_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "azure",
        label: "Azure OpenAI",
        protocols: "chat_completions",
        credential_hint: "AZURE_OPENAI_API_KEY",
        adapter: "opcional",
    },
    ProviderSpec {
        id: "bedrock",
        label: "AWS Bedrock",
        protocols: "bedrock",
        credential_hint: "AWS_PROFILE/cofre",
        adapter: "opcional",
    },
    ProviderSpec {
        id: "ollama",
        label: "Ollama",
        protocols: "ollama",
        credential_hint: "nenhuma",
        adapter: "real (API local)",
    },
    ProviderSpec {
        id: "llama-cpp",
        label: "llama.cpp server",
        protocols: "chat_completions",
        credential_hint: "nenhuma",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "vllm",
        label: "vLLM",
        protocols: "chat_completions",
        credential_hint: "variável opcional",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "litellm",
        label: "LiteLLM",
        protocols: "chat_completions",
        credential_hint: "LITELLM_API_KEY",
        adapter: "compatível",
    },
    ProviderSpec {
        id: "custom",
        label: "OpenAI-compatible custom",
        protocols: "chat_completions",
        credential_hint: "variável definida pelo usuário",
        adapter: "compatível",
    },
];

pub fn catalog() -> &'static [ProviderSpec] {
    CATALOG
}

pub fn find_spec(kind: &str) -> Option<&'static ProviderSpec> {
    let normalized = kind.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    catalog().iter().find(|spec| spec.id == normalized)
}

pub fn provider_defaults(kind: &str) -> Option<(&'static str, &'static str)> {
    let normalized = kind.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    match normalized.as_str() {
        "cerebras" => Some(("https://api.cerebras.ai/v1", "gpt-oss-120b")),
        _ => None,
    }
}
#[derive(Clone)]
pub struct ProviderRegistry {
    config: AppConfig,
    client: Client,
    circuits: Arc<Mutex<HashMap<String, CircuitState>>>,
    spent_usd: Arc<Mutex<HashMap<String, f64>>>,
}

impl ProviderRegistry {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            client: Client::new(),
            circuits: Arc::new(Mutex::new(HashMap::new())),
            spent_usd: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn estimate_cost(&self, task: &str, prompt: &str) -> Option<f64> {
        let mut aliases = Vec::new();
        if let Some(alias) = self.config.routes.get(task) {
            aliases.push(alias.clone());
        }
        if let Some(alias) = &self.config.active_provider
            && !aliases.contains(alias)
        {
            aliases.push(alias.clone());
        }
        for alias in &self.config.fallback {
            if !aliases.contains(alias) {
                aliases.push(alias.clone());
            }
        }
        aliases.into_iter().find_map(|alias| {
            self.config
                .providers
                .iter()
                .find(|provider| provider.alias == alias)
                .map(|provider| estimate_cost(provider, prompt.chars().count()))
        })
    }

    pub async fn chat(&self, explicit: Option<&str>, task: &str, prompt: &str) -> Result<String> {
        Ok(self.chat_detailed(explicit, task, prompt).await?.answer)
    }

    pub async fn chat_with_context(
        &self,
        explicit: Option<&str>,
        task: &str,
        prompt: &str,
        system_context: Option<&str>,
    ) -> Result<String> {
        self.chat_with_context_and_images(explicit, task, prompt, system_context, &[])
            .await
    }

    pub async fn chat_with_context_and_images(
        &self,
        explicit: Option<&str>,
        task: &str,
        prompt: &str,
        system_context: Option<&str>,
        images: &[ImageInput],
    ) -> Result<String> {
        Ok(self
            .chat_detailed_with_context_and_images(explicit, task, prompt, system_context, images)
            .await?
            .answer)
    }

    pub async fn chat_detailed(
        &self,
        explicit: Option<&str>,
        task: &str,
        prompt: &str,
    ) -> Result<ChatOutcome> {
        self.chat_detailed_with_context(explicit, task, prompt, None)
            .await
    }

    pub async fn chat_detailed_with_context(
        &self,
        explicit: Option<&str>,
        task: &str,
        prompt: &str,
        system_context: Option<&str>,
    ) -> Result<ChatOutcome> {
        self.chat_detailed_with_context_and_images(explicit, task, prompt, system_context, &[])
            .await
    }

    pub async fn chat_detailed_with_context_and_images(
        &self,
        explicit: Option<&str>,
        task: &str,
        prompt: &str,
        system_context: Option<&str>,
        images: &[ImageInput],
    ) -> Result<ChatOutcome> {
        validate_images(images)?;
        let mut aliases = Vec::new();
        if let Some(alias) = explicit {
            aliases.push(alias.to_string());
        }
        if let Some(alias) = self.config.routes.get(task)
            && !aliases.contains(alias)
        {
            aliases.push(alias.clone());
        }
        if let Some(alias) = &self.config.active_provider
            && !aliases.contains(alias)
        {
            aliases.push(alias.clone());
        }
        for alias in &self.config.fallback {
            if !aliases.contains(alias) {
                aliases.push(alias.clone());
            }
        }
        if aliases.is_empty() {
            bail!(
                "no provider configured; run: sapiens-agent provider add custom --alias principal --base-url https://host.example/v1 --model modelo"
            );
        }
        let mut failures = Vec::new();
        for alias in aliases {
            let provider = find_provider(&self.config, &alias)?;
            if !self.circuit_allows(&alias).await {
                failures.push(ProviderAttempt {
                    alias,
                    error: "circuit breaker aberto; aguardando cooldown".into(),
                });
                continue;
            }
            let started = Instant::now();
            let mut messages = Vec::with_capacity(2);
            if let Some(context) = system_context.filter(|value| !value.trim().is_empty()) {
                messages.push(Message {
                    role: "system".into(),
                    content: context.into(),
                });
            }
            messages.push(Message {
                role: "user".into(),
                content: prompt.into(),
            });
            match self.complete(&alias, messages, images).await {
                Ok(result) => {
                    self.record_success(&alias).await;
                    self.record_spend(&alias, result.estimated_cost_usd).await;
                    return Ok(ChatOutcome {
                        answer: result.text,
                        provider: alias,
                        latency_ms: started.elapsed().as_millis(),
                        estimated_cost_usd: result.estimated_cost_usd,
                        cost_source: result.cost_source,
                        attempts: failures,
                    });
                }
                Err(error) => {
                    self.record_failure(&alias, provider).await;
                    failures.push(ProviderAttempt {
                        alias,
                        error: crate::observability::redact(&error.to_string()),
                    });
                }
            }
        }
        let details = failures
            .iter()
            .map(|attempt| format!("{}: {}", attempt.alias, attempt.error))
            .collect::<Vec<_>>();
        bail!("all providers failed: {}", details.join("; "))
    }
    async fn complete(
        &self,
        alias: &str,
        messages: Vec<Message>,
        images: &[ImageInput],
    ) -> Result<CompletionResult> {
        let provider = find_provider(&self.config, alias)?;
        if provider.protocol != "chat_completions"
            && provider.protocol != "openai"
            && provider.protocol != "anthropic_messages"
            && provider.protocol != "gemini"
            && provider.protocol != "ollama"
            && provider.protocol != "responses"
        {
            bail!(
                "protocol '{}' is declared but no validated adapter is available",
                provider.protocol
            );
        }
        if provider.base_url.is_empty() || provider.model.is_empty() {
            bail!("provider requires base_url and model");
        }
        let input_chars: usize = messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum();
        if input_chars > provider.max_input_chars as usize {
            bail!(
                "provider input exceeds max_input_chars ({})",
                provider.max_input_chars
            );
        }
        let estimated_cost_usd = estimate_cost(provider, input_chars);
        if provider.budget_usd > 0.0 {
            let spent = self
                .spent_usd
                .lock()
                .await
                .get(alias)
                .copied()
                .unwrap_or_default();
            if estimated_cost_usd > provider.budget_usd {
                bail!(
                    "estimated request cost ${estimated_cost_usd:.6} exceeds provider budget ${:.6}",
                    provider.budget_usd
                );
            }
            if spent + estimated_cost_usd > provider.budget_usd {
                bail!(
                    "provider budget exhausted: spent ${spent:.6} of ${:.6}",
                    provider.budget_usd
                );
            }
        }
        let key = if provider.api_key_env.is_empty() {
            None
        } else {
            Some(std::env::var(&provider.api_key_env).with_context(|| {
                format!(
                    "missing credential environment variable {}",
                    provider.api_key_env
                )
            })?)
        };
        let attempts = provider.retries.min(5) as usize + 1;
        if provider.protocol == "anthropic_messages" {
            return self
                .complete_anthropic(
                    provider,
                    &key,
                    &messages,
                    images,
                    input_chars,
                    estimated_cost_usd,
                    attempts,
                )
                .await;
        }
        if provider.protocol == "gemini" {
            return self
                .complete_gemini(
                    provider,
                    &key,
                    &messages,
                    images,
                    input_chars,
                    estimated_cost_usd,
                    attempts,
                )
                .await;
        }
        if provider.protocol == "ollama" {
            return self
                .complete_ollama(
                    provider,
                    &key,
                    &messages,
                    images,
                    input_chars,
                    estimated_cost_usd,
                    attempts,
                )
                .await;
        }
        if provider.protocol == "responses" {
            return self
                .complete_responses(
                    provider,
                    &key,
                    &messages,
                    images,
                    input_chars,
                    estimated_cost_usd,
                    attempts,
                )
                .await;
        }
        let payload = json!({
            "model": provider.model,
            "messages": chat_completion_messages(&messages, images),
            "max_tokens": provider.max_tokens,
            "temperature": provider.temperature,
            "stream": provider.streaming
        });
        let mut last_error = String::from("provider request failed");
        for attempt in 0..attempts {
            let mut request = self
                .client
                .post(endpoint(&provider.base_url))
                .json(&payload);
            if let Some(key) = &key {
                request = request.bearer_auth(key);
            }
            if provider.kind.eq_ignore_ascii_case("cerebras") {
                request = request.header("X-Cerebras-3rd-Party-Integration", "sapiens-agent");
            }
            match request
                .timeout(Duration::from_secs(provider.timeout_secs))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    if provider.streaming {
                        return Ok(CompletionResult {
                            text: streamed_text(response).await?,
                            estimated_cost_usd,
                            cost_source: "estimate".into(),
                        });
                    }
                    let body: Value = response.json().await?;
                    let (response_cost, cost_source) = response_cost(provider, &body, input_chars);
                    if let Some(text) = body
                        .pointer("/choices/0/message/content")
                        .and_then(Value::as_str)
                    {
                        return Ok(CompletionResult {
                            text: text.to_string(),
                            estimated_cost_usd: response_cost,
                            cost_source,
                        });
                    }
                    if let Some(text) = body.get("output_text").and_then(Value::as_str) {
                        return Ok(CompletionResult {
                            text: text.to_string(),
                            estimated_cost_usd: response_cost,
                            cost_source,
                        });
                    }
                    bail!("provider response did not contain choices[0].message.content")
                }
                Ok(response) => {
                    let status = response.status();
                    last_error = format!("HTTP {status}");
                    if !is_retryable(status) || attempt + 1 == attempts {
                        bail!("provider request failed: {last_error}");
                    }
                }
                Err(error) => {
                    last_error = error.to_string();
                    if attempt + 1 == attempts {
                        bail!("provider request failed: {last_error}");
                    }
                }
            }
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
        bail!("provider request failed: {last_error}")
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_anthropic(
        &self,
        provider: &ProviderConfig,
        key: &Option<String>,
        messages: &[Message],
        images: &[ImageInput],
        input_chars: usize,
        estimated_cost_usd: f64,
        attempts: usize,
    ) -> Result<CompletionResult> {
        let system = messages
            .iter()
            .find(|message| message.role == "system")
            .map(|message| message.content.clone());
        let user_messages = messages
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| anthropic_message(message, images))
            .collect::<Vec<_>>();
        let mut payload = json!({
            "model": provider.model,
            "max_tokens": provider.max_tokens,
            "messages": user_messages,
            "temperature": provider.temperature,
            "stream": provider.streaming,
        });
        if let Some(system) = system {
            payload["system"] = Value::String(system);
        }
        let mut last_error = String::from("Anthropic Messages request failed");
        for attempt in 0..attempts {
            let mut request = self
                .client
                .post(anthropic_endpoint(&provider.base_url))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&payload);
            if let Some(key) = key {
                request = request.header("x-api-key", key);
            }
            match request
                .timeout(Duration::from_secs(provider.timeout_secs))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    if provider.streaming {
                        return Ok(CompletionResult {
                            text: streamed_anthropic_text(response).await?,
                            estimated_cost_usd,
                            cost_source: "estimate".into(),
                        });
                    }
                    let body: Value = response.json().await?;
                    let (response_cost, cost_source) = response_cost(provider, &body, input_chars);
                    if let Some(text) = body.pointer("/content/0/text").and_then(Value::as_str) {
                        return Ok(CompletionResult {
                            text: text.to_string(),
                            estimated_cost_usd: response_cost,
                            cost_source,
                        });
                    }
                    bail!("Anthropic response did not contain content[0].text")
                }
                Ok(response) => {
                    let status = response.status();
                    last_error = format!("HTTP {status}");
                    if !is_retryable(status) || attempt + 1 == attempts {
                        bail!("Anthropic request failed: {last_error}");
                    }
                }
                Err(error) => {
                    last_error = crate::observability::redact(&error.to_string());
                    if attempt + 1 == attempts {
                        bail!("Anthropic request failed: {last_error}");
                    }
                }
            }
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
        bail!("Anthropic request failed: {last_error}")
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_gemini(
        &self,
        provider: &ProviderConfig,
        key: &Option<String>,
        messages: &[Message],
        images: &[ImageInput],
        input_chars: usize,
        estimated_cost_usd: f64,
        attempts: usize,
    ) -> Result<CompletionResult> {
        let mut contents = Vec::new();
        let mut system_instruction = None;
        for (index, message) in messages.iter().enumerate() {
            if message.role == "system" {
                system_instruction = Some(json!({"parts": [{"text": message.content}]}));
                continue;
            }
            let role = if message.role == "assistant" {
                "model"
            } else {
                "user"
            };
            let mut parts = vec![json!({"text": message.content})];
            if index + 1 == messages.len() && message.role == "user" {
                parts.extend(images.iter().map(|image| {
                    json!({
                        "inlineData": {
                            "mimeType": image.mime_type,
                            "data": image.data_base64,
                        }
                    })
                }));
            }
            contents.push(json!({"role": role, "parts": parts}));
        }
        let mut payload = json!({
            "contents": contents,
            "generationConfig": {"temperature": provider.temperature, "maxOutputTokens": provider.max_tokens}
        });
        if let Some(system_instruction) = system_instruction {
            payload["systemInstruction"] = system_instruction;
        }
        let mut last_error = String::from("Gemini Generate Content request failed");
        for attempt in 0..attempts {
            let mut request = self
                .client
                .post(gemini_endpoint(
                    &provider.base_url,
                    &provider.model,
                    provider.streaming,
                ))
                .header("content-type", "application/json")
                .json(&payload);
            if let Some(key) = key {
                request = request.header("x-goog-api-key", key);
            }
            match request
                .timeout(Duration::from_secs(provider.timeout_secs))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    if provider.streaming {
                        return Ok(CompletionResult {
                            text: streamed_gemini_text(response).await?,
                            estimated_cost_usd,
                            cost_source: "estimate".into(),
                        });
                    }
                    let body: Value = response.json().await?;
                    let (response_cost, cost_source) = response_cost(provider, &body, input_chars);
                    if let Some(text) = body
                        .pointer("/candidates/0/content/parts/0/text")
                        .and_then(Value::as_str)
                    {
                        return Ok(CompletionResult {
                            text: text.to_string(),
                            estimated_cost_usd: response_cost,
                            cost_source,
                        });
                    }
                    bail!("Gemini response did not contain candidates[0].content.parts[0].text")
                }
                Ok(response) => {
                    let status = response.status();
                    last_error = format!("HTTP {status}");
                    if !is_retryable(status) || attempt + 1 == attempts {
                        bail!("Gemini request failed: {last_error}");
                    }
                }
                Err(error) => {
                    last_error = crate::observability::redact(&error.to_string());
                    if attempt + 1 == attempts {
                        bail!("Gemini request failed: {last_error}");
                    }
                }
            }
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
        bail!("Gemini request failed: {last_error}")
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_ollama(
        &self,
        provider: &ProviderConfig,
        key: &Option<String>,
        messages: &[Message],
        images: &[ImageInput],
        input_chars: usize,
        estimated_cost_usd: f64,
        attempts: usize,
    ) -> Result<CompletionResult> {
        let mut ollama_messages = messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.content}))
            .collect::<Vec<_>>();
        if !images.is_empty()
            && let Some(last) = ollama_messages
                .iter_mut()
                .rev()
                .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        {
            last["images"] = json!(
                images
                    .iter()
                    .map(|image| image.data_base64.clone())
                    .collect::<Vec<_>>()
            );
        }
        let payload = json!({
            "model": provider.model,
            "messages": ollama_messages,
            "stream": provider.streaming,
            "options": {"num_predict": provider.max_tokens, "temperature": provider.temperature},
        });
        let mut last_error = String::from("Ollama /api/chat request failed");
        for attempt in 0..attempts {
            let mut request = self
                .client
                .post(ollama_chat_endpoint(&provider.base_url))
                .header("content-type", "application/json")
                .json(&payload);
            if let Some(key) = key {
                request = request.bearer_auth(key);
            }
            match request
                .timeout(Duration::from_secs(provider.timeout_secs))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    if provider.streaming {
                        return Ok(CompletionResult {
                            text: streamed_ollama_text(response).await?,
                            estimated_cost_usd,
                            cost_source: "estimate".into(),
                        });
                    }
                    let body: Value = response.json().await?;
                    let (response_cost, cost_source) = response_cost(provider, &body, input_chars);
                    if let Some(text) = body.pointer("/message/content").and_then(Value::as_str) {
                        return Ok(CompletionResult {
                            text: text.to_string(),
                            estimated_cost_usd: response_cost,
                            cost_source,
                        });
                    }
                    bail!("Ollama response did not contain message.content")
                }
                Ok(response) => {
                    let status = response.status();
                    last_error = format!("HTTP {status}");
                    if !is_retryable(status) || attempt + 1 == attempts {
                        bail!("Ollama request failed: {last_error}");
                    }
                }
                Err(error) => {
                    last_error = crate::observability::redact(&error.to_string());
                    if attempt + 1 == attempts {
                        bail!("Ollama request failed: {last_error}");
                    }
                }
            }
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
        bail!("Ollama request failed: {last_error}")
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_responses(
        &self,
        provider: &ProviderConfig,
        key: &Option<String>,
        messages: &[Message],
        images: &[ImageInput],
        input_chars: usize,
        estimated_cost_usd: f64,
        attempts: usize,
    ) -> Result<CompletionResult> {
        let instructions = messages
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let input = messages
            .iter()
            .filter(|message| message.role != "system")
            .enumerate()
            .map(|(index, message)| responses_message(message, images, index + 1 == messages.len()))
            .collect::<Vec<_>>();
        let mut payload = json!({
            "model": provider.model,
            "input": input,
            "max_output_tokens": provider.max_tokens,
            "temperature": provider.temperature,
            "stream": provider.streaming,
            "store": false,
        });
        if !instructions.is_empty() {
            payload["instructions"] = Value::String(instructions);
        }
        let mut last_error = String::from("OpenAI Responses request failed");
        for attempt in 0..attempts {
            let mut request = self
                .client
                .post(responses_endpoint(&provider.base_url))
                .header("content-type", "application/json")
                .json(&payload);
            if let Some(key) = key {
                request = request.bearer_auth(key);
            }
            match request
                .timeout(Duration::from_secs(provider.timeout_secs))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    if provider.streaming {
                        return Ok(CompletionResult {
                            text: streamed_responses_text(response).await?,
                            estimated_cost_usd,
                            cost_source: "estimate".into(),
                        });
                    }
                    let body: Value = response.json().await?;
                    let (response_cost, cost_source) = response_cost(provider, &body, input_chars);
                    if let Some(text) = responses_text(&body) {
                        return Ok(CompletionResult {
                            text,
                            estimated_cost_usd: response_cost,
                            cost_source,
                        });
                    }
                    bail!("OpenAI Responses response did not contain output text")
                }
                Ok(response) => {
                    let status = response.status();
                    last_error = format!("HTTP {status}");
                    if !is_retryable(status) || attempt + 1 == attempts {
                        bail!("OpenAI Responses request failed: {last_error}");
                    }
                }
                Err(error) => {
                    last_error = crate::observability::redact(&error.to_string());
                    if attempt + 1 == attempts {
                        bail!("OpenAI Responses request failed: {last_error}");
                    }
                }
            }
            sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
        bail!("OpenAI Responses request failed: {last_error}")
    }

    async fn circuit_allows(&self, alias: &str) -> bool {
        let mut circuits = self.circuits.lock().await;
        let Some(state) = circuits.get_mut(alias) else {
            return true;
        };
        if let Some(until) = state.open_until {
            if until > Instant::now() {
                return false;
            }
            state.failures = 0;
            state.open_until = None;
        }
        true
    }

    async fn record_failure(&self, alias: &str, provider: &ProviderConfig) {
        let mut circuits = self.circuits.lock().await;
        let state = circuits.entry(alias.to_string()).or_insert(CircuitState {
            failures: 0,
            open_until: None,
        });
        state.failures = state.failures.saturating_add(1);
        if state.failures >= provider.circuit_breaker_threshold.max(1) {
            state.open_until = Some(
                Instant::now() + Duration::from_secs(provider.circuit_breaker_cooldown_secs.max(1)),
            );
        }
    }

    async fn record_success(&self, alias: &str) {
        self.circuits.lock().await.remove(alias);
    }

    async fn record_spend(&self, alias: &str, amount: f64) {
        if amount > 0.0 {
            let mut spent = self.spent_usd.lock().await;
            *spent.entry(alias.to_string()).or_default() += amount;
        }
    }
    pub async fn test(&self, alias: &str) -> Result<String> {
        let health = self.health(alias).await?;
        Ok(format!("{alias}: ok ({} ms)", health.latency_ms))
    }
    pub async fn health(&self, alias: &str) -> Result<ProviderHealth> {
        let p = find_provider(&self.config, alias)?;
        if p.base_url.is_empty() {
            bail!("provider requires base_url");
        }
        let started = Instant::now();
        let response = self
            .authorized_request(p, &models_endpoint(p))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(ProviderHealth {
                alias: alias.to_string(),
                protocol: p.protocol.clone(),
                ok: true,
                latency_ms: started.elapsed().as_millis(),
                status: Some(response.status().as_u16()),
            })
        } else {
            bail!("{alias}: HTTP {}", response.status())
        }
    }
    pub async fn models(&self, alias: &str) -> Result<Vec<String>> {
        let p = find_provider(&self.config, alias)?;
        let body: Value = self
            .authorized_request(p, &models_endpoint(p))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if p.protocol == "ollama" {
            return Ok(body
                .get("models")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| {
                            v.get("name")
                                .or_else(|| v.get("model"))
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default());
        }
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }
    fn authorized_request(&self, provider: &ProviderConfig, url: &str) -> reqwest::RequestBuilder {
        let builder = self.client.get(url);
        if provider.api_key_env.is_empty() {
            return builder;
        }
        match std::env::var(&provider.api_key_env) {
            Ok(key) if provider.protocol == "anthropic_messages" => {
                builder.header("x-api-key", key)
            }
            Ok(key) if provider.protocol == "gemini" => builder.header("x-goog-api-key", key),
            Ok(key) if provider.kind.eq_ignore_ascii_case("cerebras") => builder
                .bearer_auth(key)
                .header("X-Cerebras-3rd-Party-Integration", "sapiens-agent"),
            Ok(key) => builder.bearer_auth(key),
            Err(_) => builder,
        }
    }
}

fn is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn streamed_text(mut response: reqwest::Response) -> Result<String> {
    let mut buffer = String::new();
    let mut answer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(std::str::from_utf8(&chunk).context("provider SSE was not UTF-8")?);
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].trim_end_matches('\r').to_string();
            buffer.drain(..=index);
            if consume_sse_line(&line, &mut answer)? {
                return Ok(answer);
            }
        }
    }
    if !buffer.is_empty() {
        consume_sse_line(buffer.trim_end_matches('\r'), &mut answer)?;
    }
    if answer.is_empty() {
        bail!("provider SSE response did not contain text deltas");
    }
    Ok(answer)
}

fn consume_sse_line(line: &str, answer: &mut String) -> Result<bool> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(true);
    }
    if data.is_empty() {
        return Ok(false);
    }
    let body: Value = serde_json::from_str(data).context("invalid provider SSE data")?;
    if let Some(text) = body
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
    {
        answer.push_str(text);
    } else if let Some(text) = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
    {
        answer.push_str(text);
    } else if let Some(text) = body.get("output_text").and_then(Value::as_str) {
        answer.push_str(text);
    }
    Ok(false)
}

fn chat_completion_messages(messages: &[Message], images: &[ImageInput]) -> Vec<Value> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if !images.is_empty() && index + 1 == messages.len() && message.role == "user" {
                let mut parts = vec![json!({"type": "text", "text": message.content})];
                parts.extend(images.iter().map(|image| {
                    json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", image.mime_type, image.data_base64),
                            "detail": "auto"
                        }
                    })
                }));
                json!({"role": message.role, "content": parts})
            } else {
                json!({"role": message.role, "content": message.content})
            }
        })
        .collect()
}

fn anthropic_message(message: &Message, images: &[ImageInput]) -> Value {
    if message.role == "user" && !images.is_empty() {
        let mut content = vec![json!({"type": "text", "text": message.content})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image.mime_type,
                    "data": image.data_base64
                }
            })
        }));
        json!({"role": message.role, "content": content})
    } else {
        json!({"role": message.role, "content": message.content})
    }
}

fn responses_message(message: &Message, images: &[ImageInput], is_last: bool) -> Value {
    if message.role == "user" && is_last && !images.is_empty() {
        let mut content = vec![json!({"type": "input_text", "text": message.content})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "input_image",
                "image_url": format!("data:{};base64,{}", image.mime_type, image.data_base64),
                "detail": "auto"
            })
        }));
        json!({"role": "user", "content": content})
    } else {
        json!({
            "role": if message.role == "assistant" { "assistant" } else { "user" },
            "content": message.content,
        })
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or_default() as u32;
        let third = chunk.get(2).copied().unwrap_or_default() as u32;
        let value = (first << 16) | (second << 8) | third;
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

async fn streamed_responses_text(mut response: reqwest::Response) -> Result<String> {
    let mut buffer = String::new();
    let mut answer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(std::str::from_utf8(&chunk).context("Responses SSE was not UTF-8")?);
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].trim_end_matches('\r').to_string();
            buffer.drain(..=index);
            consume_responses_sse_line(&line, &mut answer)?;
        }
    }
    if !buffer.trim().is_empty() {
        consume_responses_sse_line(buffer.trim_end_matches('\r'), &mut answer)?;
    }
    if answer.is_empty() {
        bail!("OpenAI Responses SSE did not contain output text deltas");
    }
    Ok(answer)
}

fn consume_responses_sse_line(line: &str, answer: &mut String) -> Result<()> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(());
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let body: Value = serde_json::from_str(data).context("invalid Responses SSE data")?;
    if body.get("type").and_then(Value::as_str) == Some("response.output_text.delta")
        && let Some(delta) = body.get("delta").and_then(Value::as_str)
    {
        answer.push_str(delta);
    }
    Ok(())
}

fn endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn responses_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/responses") {
        base.to_string()
    } else {
        format!("{base}/responses")
    }
}

fn responses_text(body: &Value) -> Option<String> {
    if let Some(text) = body.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let mut text = String::new();
    for item in body.get("output")?.as_array()? {
        for content in item.get("content")?.as_array()? {
            if content.get("type").and_then(Value::as_str) == Some("output_text")
                && let Some(value) = content.get("text").and_then(Value::as_str)
            {
                text.push_str(value);
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

fn ollama_chat_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/api/chat") {
        base.to_string()
    } else if base.ends_with("/api") {
        format!("{base}/chat")
    } else {
        format!("{base}/api/chat")
    }
}

fn models_endpoint(provider: &ProviderConfig) -> String {
    let base = provider.base_url.trim_end_matches('/');
    if provider.protocol != "ollama" {
        return format!("{base}/models");
    }
    if base.ends_with("/api/tags") {
        base.to_string()
    } else if base.ends_with("/api") {
        format!("{base}/tags")
    } else {
        format!("{base}/api/tags")
    }
}

fn anthropic_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/messages") {
        base.to_string()
    } else {
        format!("{base}/messages")
    }
}

fn gemini_endpoint(base_url: &str, model: &str, streaming: bool) -> String {
    let base = base_url.trim_end_matches('/');
    let action = if streaming {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    if base.ends_with("generateContent") || base.ends_with("streamGenerateContent") {
        base.to_string()
    } else if base.contains("/models/") {
        format!("{base}:{action}")
    } else {
        format!("{base}/models/{model}:{action}")
    }
}

async fn streamed_anthropic_text(mut response: reqwest::Response) -> Result<String> {
    let mut buffer = String::new();
    let mut answer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(std::str::from_utf8(&chunk).context("Anthropic SSE was not UTF-8")?);
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].trim_end_matches('\r').to_string();
            buffer.drain(..=index);
            consume_anthropic_sse_line(&line, &mut answer)?;
        }
    }
    if !buffer.trim().is_empty() {
        consume_anthropic_sse_line(buffer.trim_end_matches('\r'), &mut answer)?;
    }
    if answer.is_empty() {
        bail!("Anthropic SSE response did not contain text deltas");
    }
    Ok(answer)
}

async fn streamed_gemini_text(mut response: reqwest::Response) -> Result<String> {
    let mut buffer = String::new();
    let mut answer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(std::str::from_utf8(&chunk).context("Gemini SSE was not UTF-8")?);
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].trim_end_matches('\r').to_string();
            buffer.drain(..=index);
            consume_gemini_sse_line(&line, &mut answer)?;
        }
    }
    if !buffer.trim().is_empty() {
        consume_gemini_sse_line(buffer.trim_end_matches('\r'), &mut answer)?;
    }
    if answer.is_empty() {
        bail!("Gemini SSE response did not contain text deltas");
    }
    Ok(answer)
}

async fn streamed_ollama_text(mut response: reqwest::Response) -> Result<String> {
    let mut buffer = String::new();
    let mut answer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(std::str::from_utf8(&chunk).context("Ollama stream was not UTF-8")?);
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].trim();
            if !line.is_empty() {
                consume_ollama_line(line, &mut answer)?;
            }
            buffer.drain(..=index);
        }
    }
    if !buffer.trim().is_empty() {
        consume_ollama_line(buffer.trim(), &mut answer)?;
    }
    if answer.is_empty() {
        bail!("Ollama stream response did not contain message.content");
    }
    Ok(answer)
}

fn consume_ollama_line(line: &str, answer: &mut String) -> Result<()> {
    let body: Value = serde_json::from_str(line).context("invalid Ollama NDJSON data")?;
    if let Some(text) = body.pointer("/message/content").and_then(Value::as_str) {
        answer.push_str(text);
    }
    Ok(())
}

fn consume_gemini_sse_line(line: &str, answer: &mut String) -> Result<()> {
    let data = line
        .strip_prefix("data:")
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .unwrap_or_else(|| line.trim());
    if data.is_empty() {
        return Ok(());
    }
    let body: Value = serde_json::from_str(data).context("invalid Gemini SSE data")?;
    if let Some(text) = body
        .pointer("/candidates/0/content/parts/0/text")
        .and_then(Value::as_str)
    {
        answer.push_str(text);
    }
    Ok(())
}

fn consume_anthropic_sse_line(line: &str, answer: &mut String) -> Result<()> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(());
    };
    let data = data.trim();
    if data.is_empty() {
        return Ok(());
    }
    let body: Value = serde_json::from_str(data).context("invalid Anthropic SSE data")?;
    if body.get("type").and_then(Value::as_str) == Some("content_block_delta")
        && let Some(text) = body.pointer("/delta/text").and_then(Value::as_str)
    {
        answer.push_str(text);
    }
    Ok(())
}

fn estimate_cost(provider: &ProviderConfig, input_chars: usize) -> f64 {
    let input_tokens = input_chars.saturating_add(3) / 4;
    let input_cost = input_tokens as f64 / 1000.0 * provider.input_cost_per_1k_tokens;
    let output_cost = provider.max_tokens as f64 / 1000.0 * provider.output_cost_per_1k_tokens;
    input_cost + output_cost
}

fn response_cost(provider: &ProviderConfig, body: &Value, input_chars: usize) -> (f64, String) {
    let usage = body
        .get("usage")
        .or_else(|| body.get("usageMetadata"))
        .and_then(Value::as_object);
    let input_tokens = usage
        .and_then(|usage| {
            usage
                .get("prompt_tokens")
                .or_else(|| usage.get("input_tokens"))
                .or_else(|| usage.get("promptTokenCount"))
                .or_else(|| usage.get("prompt_eval_count"))
        })
        .and_then(Value::as_u64)
        .or_else(|| body.get("prompt_eval_count").and_then(Value::as_u64));
    let output_tokens = usage
        .and_then(|usage| {
            usage
                .get("completion_tokens")
                .or_else(|| usage.get("output_tokens"))
                .or_else(|| usage.get("candidatesTokenCount"))
                .or_else(|| usage.get("eval_count"))
        })
        .and_then(Value::as_u64)
        .or_else(|| body.get("eval_count").and_then(Value::as_u64));
    match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => (
            input as f64 / 1000.0 * provider.input_cost_per_1k_tokens
                + output as f64 / 1000.0 * provider.output_cost_per_1k_tokens,
            "provider_usage".into(),
        ),
        _ => (estimate_cost(provider, input_chars), "estimate".into()),
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn catalog_covers_remote_and_local_provider_families() {
        assert!(catalog().iter().any(|item| item.id == "openai"));
        assert!(catalog().iter().any(|item| item.id == "ollama"));
        assert!(catalog().iter().any(|item| item.id == "cerebras"));
        assert!(catalog().iter().any(|item| item.id == "custom"));
    }

    #[test]
    fn cerebras_has_safe_openai_compatible_defaults() {
        assert_eq!(
            provider_defaults("Cerebras"),
            Some(("https://api.cerebras.ai/v1", "gpt-oss-120b"))
        );
        let spec = find_spec("cerebras").expect("Cerebras catalog entry");
        assert_eq!(spec.protocols, "chat_completions");
        assert_eq!(spec.credential_hint, "CEREBRAS_API_KEY");
    }

    #[test]
    fn endpoint_accepts_base_or_full_chat_path() {
        assert_eq!(
            endpoint("https://example.test/v1"),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            endpoint("https://example.test/v1/chat/completions"),
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn multimodal_payloads_follow_native_provider_shapes() {
        let image = ImageInput {
            mime_type: "image/png".into(),
            data_base64: "AQID".into(),
        };
        let messages = vec![Message {
            role: "user".into(),
            content: "descreva a imagem".into(),
        }];
        let openai = chat_completion_messages(&messages, std::slice::from_ref(&image));
        assert_eq!(openai[0]["content"][1]["type"], "image_url");
        assert_eq!(
            openai[0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AQID"
        );
        let anthropic = anthropic_message(&messages[0], std::slice::from_ref(&image));
        assert_eq!(anthropic["content"][1]["type"], "image");
        assert_eq!(anthropic["content"][1]["source"]["media_type"], "image/png");
        let responses = responses_message(&messages[0], &[image], true);
        assert_eq!(responses["content"][1]["type"], "input_image");
        assert_eq!(responses["content"][1]["detail"], "auto");
    }

    #[test]
    fn image_file_input_is_bounded_and_encoded_without_logging_path() {
        let path =
            std::env::temp_dir().join(format!("sapiens-provider-image-{}.png", std::process::id()));
        std::fs::write(&path, [0_u8, 1, 2, 3]).expect("image");
        let image = image_input_from_path(&path).expect("image input");
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.data_base64, "AAECAw==");
        let _ = std::fs::remove_file(&path);

        let bad_path =
            std::env::temp_dir().join(format!("sapiens-provider-image-{}.txt", std::process::id()));
        std::fs::write(&bad_path, [0_u8]).expect("bad image");
        assert!(image_input_from_path(&bad_path).is_err());
        let _ = std::fs::remove_file(bad_path);
        assert!(
            validate_images(&[ImageInput {
                mime_type: "image/png".into(),
                data_base64: "not-base64".into(),
            }])
            .is_err()
        );
    }

    #[test]
    fn multimodal_chat_completion_sends_a_real_image_request() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let received = Arc::new(StdMutex::new(String::new()));
            let received_server = Arc::clone(&received);
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = vec![0_u8; 32_768];
                let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                *received_server.lock().expect("received lock") =
                    String::from_utf8_lossy(&buffer[..count]).to_string();
                let body = r#"{"choices":[{"message":{"content":"imagem recebida"}}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("local-image".into());
            config.providers.push(ProviderConfig {
                alias: "local-image".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                api_key_env: String::new(),
                ..Default::default()
            });
            let answer = ProviderRegistry::new(config)
                .chat_with_context_and_images(
                    None,
                    "test",
                    "descreva",
                    None,
                    &[ImageInput {
                        mime_type: "image/png".into(),
                        data_base64: "AQID".into(),
                    }],
                )
                .await
                .expect("multimodal answer");
            server.await.expect("server");
            assert_eq!(answer, "imagem recebida");
            let request = received.lock().expect("received lock").clone();
            assert!(request.contains("data:image/png;base64,AQID"));
        });
    }

    #[test]
    fn anthropic_messages_adapter_separates_system_context_and_usage_cost() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = vec![0_u8; 16_384];
                let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with("POST /v1/messages HTTP/1.1"));
                assert!(request.to_ascii_lowercase().contains("anthropic-version: 2023-06-01"));
                assert!(request.contains("\"system\":\"context\""));
                assert!(request.contains("\"role\":\"user\""));
                let body = r#"{"content":[{"type":"text","text":"anthropic answer"}],"usage":{"input_tokens":10,"output_tokens":5}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("claude".into());
            config.providers.push(ProviderConfig {
                alias: "claude".into(),
                protocol: "anthropic_messages".into(),
                base_url: format!("http://{address}/v1"),
                model: "claude-test".into(),
                api_key_env: String::new(),
                input_cost_per_1k_tokens: 2.0,
                output_cost_per_1k_tokens: 4.0,
                ..Default::default()
            });
            let outcome = ProviderRegistry::new(config)
                .chat_detailed_with_context(None, "test", "hello", Some("context"))
                .await
                .expect("Anthropic response");
            assert_eq!(outcome.answer, "anthropic answer");
            assert_eq!(outcome.cost_source, "provider_usage");
            assert!((outcome.estimated_cost_usd - 0.04).abs() < f64::EPSILON);
            server.await.expect("server");
        });
    }

    #[test]
    fn gemini_generate_content_adapter_maps_system_and_usage() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = vec![0_u8; 16_384];
                let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with("POST /v1beta/models/gemini-test:generateContent HTTP/1.1"));
                assert!(request.to_ascii_lowercase().contains("x-goog-api-key"));
                assert!(request.contains("\"systemInstruction\""));
                let body = r#"{"candidates":[{"content":{"parts":[{"text":"gemini answer"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":4}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("gemini-local".into());
            config.providers.push(ProviderConfig {
                alias: "gemini-local".into(),
                protocol: "gemini".into(),
                base_url: format!("http://{address}/v1beta"),
                model: "gemini-test".into(),
                api_key_env: "SAPIENS_GEMINI_TEST_KEY".into(),
                input_cost_per_1k_tokens: 1.0,
                output_cost_per_1k_tokens: 2.0,
                ..Default::default()
            });
            unsafe { std::env::set_var("SAPIENS_GEMINI_TEST_KEY", "test-key") };
            let outcome = ProviderRegistry::new(config)
                .chat_detailed_with_context(None, "test", "hello", Some("context"))
                .await
                .expect("Gemini response");
            unsafe { std::env::remove_var("SAPIENS_GEMINI_TEST_KEY") };
            assert_eq!(outcome.answer, "gemini answer");
            assert_eq!(outcome.cost_source, "provider_usage");
            assert!((outcome.estimated_cost_usd - 0.016).abs() < f64::EPSILON);
            server.await.expect("server");
        });
    }

    #[test]
    fn ollama_chat_adapter_maps_messages_and_usage() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = vec![0_u8; 16_384];
                let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with("POST /api/chat HTTP/1.1"));
                assert!(request.contains("\"model\":\"qwen-test\""));
                assert!(request.contains("\"stream\":false"));
                assert!(request.contains("\"num_predict\":2048"));
                let body = r#"{"message":{"role":"assistant","content":"ollama answer"},"done":true,"prompt_eval_count":7,"eval_count":3}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("ollama-local".into());
            config.providers.push(ProviderConfig {
                alias: "ollama-local".into(),
                protocol: "ollama".into(),
                base_url: format!("http://{address}"),
                model: "qwen-test".into(),
                api_key_env: String::new(),
                input_cost_per_1k_tokens: 1.0,
                output_cost_per_1k_tokens: 2.0,
                ..Default::default()
            });
            let outcome = ProviderRegistry::new(config)
                .chat_detailed_with_context(None, "test", "hello", Some("context"))
                .await
                .expect("Ollama response");
            assert_eq!(outcome.answer, "ollama answer");
            assert_eq!(outcome.cost_source, "provider_usage");
            assert!((outcome.estimated_cost_usd - 0.013).abs() < f64::EPSILON);
            server.await.expect("server");
        });
    }

    #[test]
    fn parses_ollama_ndjson_stream() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut request = vec![0_u8; 16_384];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut request)
                    .await
                    .expect("read");
                let body = concat!(
                    "{\"message\":{\"role\":\"assistant\",\"content\":\"ol\"},\"done\":false}\n",
                    "{\"message\":{\"role\":\"assistant\",\"content\":\"lama\"},\"done\":true}\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("ollama-stream".into());
            config.providers.push(ProviderConfig {
                alias: "ollama-stream".into(),
                protocol: "ollama".into(),
                base_url: format!("http://{address}"),
                model: "qwen-test".into(),
                api_key_env: String::new(),
                streaming: true,
                ..Default::default()
            });
            let answer = ProviderRegistry::new(config)
                .chat(None, "test", "hello")
                .await
                .expect("Ollama stream");
            assert_eq!(answer, "ollama");
            server.await.expect("server");
        });
    }

    #[test]
    fn openai_responses_adapter_maps_input_output_and_usage() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = vec![0_u8; 16_384];
                let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with("POST /v1/responses HTTP/1.1"));
                assert!(request.contains("\"instructions\":\"context\""));
                assert!(request.contains("\"store\":false"));
                assert!(request.contains("\"max_output_tokens\":2048"));
                let body = r#"{"object":"response","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"responses answer"}]}],"usage":{"input_tokens":9,"output_tokens":4}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("responses-local".into());
            config.providers.push(ProviderConfig {
                alias: "responses-local".into(),
                protocol: "responses".into(),
                base_url: format!("http://{address}/v1"),
                model: "gpt-test".into(),
                api_key_env: String::new(),
                input_cost_per_1k_tokens: 1.0,
                output_cost_per_1k_tokens: 2.0,
                ..Default::default()
            });
            let outcome = ProviderRegistry::new(config)
                .chat_detailed_with_context(None, "test", "hello", Some("context"))
                .await
                .expect("Responses response");
            assert_eq!(outcome.answer, "responses answer");
            assert_eq!(outcome.cost_source, "provider_usage");
            assert!((outcome.estimated_cost_usd - 0.017).abs() < f64::EPSILON);
            server.await.expect("server");
        });
    }

    #[test]
    fn openai_responses_adapter_accumulates_sse_deltas() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut request = vec![0_u8; 16_384];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut request)
                    .await
                    .expect("read");
                let body = concat!(
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"resp\"}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"onse\"}\n\n",
                    "data: [DONE]\n\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("responses-stream".into());
            config.providers.push(ProviderConfig {
                alias: "responses-stream".into(),
                protocol: "responses".into(),
                base_url: format!("http://{address}/v1"),
                model: "gpt-test".into(),
                api_key_env: String::new(),
                streaming: true,
                ..Default::default()
            });
            let answer = ProviderRegistry::new(config)
                .chat(None, "test", "hello")
                .await
                .expect("Responses stream");
            assert_eq!(answer, "response");
            server.await.expect("server");
        });
    }

    #[test]
    fn rejects_input_over_provider_context_limit_before_network() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(async {
            let mut config = AppConfig::default();
            config.active_provider = Some("limited".into());
            config.providers.push(ProviderConfig {
                alias: "limited".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "simulated".into(),
                api_key_env: String::new(),
                max_input_chars: 3,
                ..Default::default()
            });
            ProviderRegistry::new(config)
                .chat(None, "test", "long")
                .await
        });
        assert!(result.unwrap_err().to_string().contains("max_input_chars"));
    }

    #[test]
    fn enforces_provider_budget_before_network() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(async {
            let mut config = AppConfig::default();
            config.active_provider = Some("budgeted".into());
            config.providers.push(ProviderConfig {
                alias: "budgeted".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "simulated".into(),
                api_key_env: String::new(),
                budget_usd: 0.001,
                output_cost_per_1k_tokens: 1.0,
                max_tokens: 10,
                ..Default::default()
            });
            ProviderRegistry::new(config)
                .chat(None, "test", "hello")
                .await
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds provider budget")
        );
    }

    #[test]
    fn opens_circuit_after_configured_consecutive_failure() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let dead = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("dead listener");
            let address = dead.local_addr().expect("dead address");
            drop(dead);
            let mut config = AppConfig::default();
            config.active_provider = Some("unstable".into());
            config.providers.push(ProviderConfig {
                alias: "unstable".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                api_key_env: String::new(),
                retries: 0,
                circuit_breaker_threshold: 1,
                circuit_breaker_cooldown_secs: 60,
                ..Default::default()
            });
            let registry = ProviderRegistry::new(config);
            let _ = registry.chat(None, "test", "first").await;
            let second = registry.chat(None, "test", "second").await;
            assert!(
                second
                    .unwrap_err()
                    .to_string()
                    .contains("circuit breaker aberto")
            );
        });
    }

    #[test]
    fn rejects_missing_credential_before_network_access() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(async {
            let mut config = AppConfig::default();
            config.active_provider = Some("secured".into());
            config.providers.push(ProviderConfig {
                alias: "secured".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "simulated".into(),
                api_key_env: "SAPIENS_TEST_MISSING_CREDENTIAL".into(),
                ..Default::default()
            });
            ProviderRegistry::new(config)
                .chat(None, "test", "hello")
                .await
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing credential environment variable")
        );
    }

    #[test]
    fn circuit_breaker_recovers_after_cooldown() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                for attempt in 0..2 {
                    let (mut stream, _) = listener.accept().await.expect("request");
                    let mut buffer = [0_u8; 4096];
                    tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                        .await
                        .expect("read");
                    let (status, body) = if attempt == 0 {
                        ("503 Service Unavailable", "temporary")
                    } else {
                        (
                            "200 OK",
                            r#"{"choices":[{"message":{"content":"recovered"}}]}"#,
                        )
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                        .await
                        .expect("write");
                }
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("recovering".into());
            config.providers.push(ProviderConfig {
                alias: "recovering".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                api_key_env: String::new(),
                retries: 0,
                circuit_breaker_threshold: 1,
                circuit_breaker_cooldown_secs: 1,
                ..Default::default()
            });
            let registry = ProviderRegistry::new(config);
            assert!(registry.chat(None, "test", "first").await.is_err());
            tokio::time::sleep(Duration::from_millis(1_100)).await;
            assert_eq!(registry.chat(None, "test", "second").await?, "recovered");
            server.await.expect("server");
            Ok::<(), anyhow::Error>(())
        })
        .expect("circuit recovery");
    }

    #[test]
    fn estimates_cost_from_input_and_output_limits() {
        let provider = ProviderConfig {
            input_cost_per_1k_tokens: 2.0,
            output_cost_per_1k_tokens: 4.0,
            max_tokens: 500,
            ..Default::default()
        };
        let cost = estimate_cost(&provider, 400);
        assert!((cost - 2.2).abs() < f64::EPSILON);
    }

    #[test]
    fn retries_transient_http_failure_then_returns_the_successful_answer() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let requests = Arc::new(AtomicUsize::new(0));
            let requests_for_server = Arc::clone(&requests);
            let server = tokio::spawn(async move {
                for attempt in 0..2 {
                    let (mut stream, _) = listener.accept().await.expect("request");
                    let mut buffer = [0_u8; 4096];
                    tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                        .await
                        .expect("read");
                    requests_for_server.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = if attempt == 0 {
                        ("503 Service Unavailable", "busy")
                    } else {
                        (
                            "200 OK",
                            r#"{"choices":[{"message":{"content":"ok after retry"}}]}"#,
                        )
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                        .await
                        .expect("write");
                }
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("local".into());
            config.providers.push(ProviderConfig {
                alias: "local".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                api_key_env: String::new(),
                retries: 1,
                ..Default::default()
            });
            let answer = ProviderRegistry::new(config)
                .chat(None, "test", "hello")
                .await
                .expect("retry answer");
            server.await.expect("server");
            assert_eq!(answer, "ok after retry");
            assert_eq!(requests.load(Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn fallback_uses_the_next_alias_after_a_provider_failure() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let primary_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("primary listener");
            let primary_address = primary_listener.local_addr().expect("primary address");
            let primary_server = tokio::spawn(async move {
                let (mut stream, _) = primary_listener.accept().await.expect("primary request");
                let mut buffer = [0_u8; 4096];
                tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("primary read");
                let body = "primary unavailable";
                let response = format!(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("primary write");
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = [0_u8; 4096];
                tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let body = r#"{"choices":[{"message":{"content":"fallback answer"}}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("primary".into());
            config.fallback = vec!["backup".into()];
            config.providers.push(ProviderConfig {
                alias: "primary".into(),
                base_url: format!("http://{primary_address}/v1"),
                model: "primary".into(),
                api_key_env: String::new(),
                retries: 0,
                timeout_secs: 1,
                ..Default::default()
            });
            config.providers.push(ProviderConfig {
                alias: "backup".into(),
                base_url: format!("http://{address}/v1"),
                model: "backup".into(),
                api_key_env: String::new(),
                retries: 0,
                ..Default::default()
            });
            let outcome = ProviderRegistry::new(config)
                .chat_detailed(None, "test", "hello")
                .await
                .expect("fallback answer");
            primary_server.await.expect("primary server");
            server.await.expect("server");
            assert_eq!(outcome.provider, "backup");
            assert!(!outcome.attempts.is_empty());
            assert_eq!(outcome.answer, "fallback answer");
        });
    }

    #[test]
    fn streaming_sse_deltas_are_joined_into_one_answer() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = [0_u8; 4096];
                tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let body = concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"stream\"}}]}\n\n",
                    "data: [DONE]\n\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("stream".into());
            config.providers.push(ProviderConfig {
                alias: "stream".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                streaming: true,
                api_key_env: String::new(),
                ..Default::default()
            });
            let answer = ProviderRegistry::new(config)
                .chat(None, "test", "hello")
                .await
                .expect("streaming answer");
            server.await.expect("server");
            assert_eq!(answer, "hello stream");
        });
    }

    #[test]
    fn sends_identity_context_as_a_system_message() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = [0_u8; 8192];
                let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.contains("system"));
                assert!(request.contains("Sapiens identity"));
                let body = r#"{"choices":[{"message":{"content":"context ok"}}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("context".into());
            config.providers.push(ProviderConfig {
                alias: "context".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                api_key_env: String::new(),
                ..Default::default()
            });
            let answer = ProviderRegistry::new(config)
                .chat_with_context(None, "test", "hello", Some("Sapiens identity"))
                .await
                .expect("context answer");
            server.await.expect("server");
            assert_eq!(answer, "context ok");
        });
    }

    #[test]
    fn prefers_provider_reported_usage_for_cost_accounting() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut buffer = [0_u8; 4096];
                tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                    .await
                    .expect("read");
                let body = r#"{"choices":[{"message":{"content":"priced"}}],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                    .await
                    .expect("write");
            });
            let mut config = AppConfig::default();
            config.active_provider = Some("priced".into());
            config.providers.push(ProviderConfig {
                alias: "priced".into(),
                base_url: format!("http://{address}/v1"),
                model: "simulated".into(),
                api_key_env: String::new(),
                input_cost_per_1k_tokens: 2.0,
                output_cost_per_1k_tokens: 4.0,
                max_tokens: 500,
                ..Default::default()
            });
            let outcome = ProviderRegistry::new(config)
                .chat_detailed(None, "test", "hello")
                .await
                .expect("usage answer");
            server.await.expect("server");
            assert_eq!(outcome.answer, "priced");
            assert_eq!(outcome.cost_source, "provider_usage");
            assert!((outcome.estimated_cost_usd - 0.1).abs() < f64::EPSILON);
        });
    }
}
