use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use platform::milancode_config_home;
use serde::Deserialize;
use serde::Serialize;

use crate::error::ApiError;
use crate::sse::SseParser;
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, MessageRequest, MessageResponse, ModelsResponse,
    ProviderSelectionResponse, StreamEvent,
};

const DEFAULT_BASE_URL: &str = "https://nano-gpt.com/api";
const REQUEST_ID_HEADER: &str = "request-id";
const ALT_REQUEST_ID_HEADER: &str = "x-request-id";
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(200);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(2);
const DEFAULT_MAX_RETRIES: u32 = 2;

fn nanogpt_client_debug_enabled() -> bool {
    std::env::var("NANOGPT_CLIENT_DEBUG")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "debug"
            )
        })
}

#[derive(Debug, Clone)]
pub struct NanoGptClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    service: ApiService,
    provider: Option<String>,
    force_paygo: bool,
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl NanoGptClient {
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            service: ApiService::NanoGpt,
            provider: None,
            force_paygo: false,
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }

    pub fn from_env() -> Result<Self, ApiError> {
        Self::from_service_env(ApiService::NanoGpt)
    }

    pub fn from_service_env(service: ApiService) -> Result<Self, ApiError> {
        Ok(Self::new(resolve_api_key_for(service)?)
            .with_service(service)
            .with_base_url(resolve_base_url_for(service)))
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn with_service(mut self, service: ApiService) -> Self {
        self.service = service;
        self
    }

    #[must_use]
    pub fn with_provider(mut self, provider: Option<String>) -> Self {
        self.provider = provider.filter(|value| !value.is_empty());
        self.force_paygo = self.provider.is_some();
        self
    }

    #[must_use]
    pub fn with_retry_policy(
        mut self,
        max_retries: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        self.max_retries = max_retries;
        self.initial_backoff = initial_backoff;
        self.max_backoff = max_backoff;
        self
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let request = MessageRequest {
            stream: false,
            ..self.normalize_message_request(request)
        };
        let response = self.send_with_retry(&request).await?;
        let request_id = request_id_from_headers(response.headers());
        let mut response = response
            .json::<MessageResponse>()
            .await
            .map_err(ApiError::from)?;
        if response.request_id.is_none() {
            response.request_id = request_id;
        }
        Ok(response)
    }

    pub async fn send_chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ApiError> {
        let response = self.send_chat_completion_raw(request).await?;
        let response = expect_success(response).await?;
        response
            .json::<ChatCompletionResponse>()
            .await
            .map_err(ApiError::from)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        let response = self
            .send_with_retry(&self.normalize_message_request(&request.clone().with_streaming()))
            .await?;
        Ok(MessageStream::from_http_response(response))
    }

    pub async fn fetch_models(&self, detailed: bool) -> Result<ModelsResponse, ApiError> {
        let response = self
            .send_get_request(
                "/v1/models",
                &[("detailed", if detailed { "true" } else { "false" })],
            )
            .await?;
        response
            .json::<ModelsResponse>()
            .await
            .map_err(ApiError::from)
    }

    pub async fn fetch_providers(
        &self,
        canonical_id: &str,
    ) -> Result<ProviderSelectionResponse, ApiError> {
        let request_url = providers_url(&self.base_url, canonical_id)?;
        if nanogpt_client_debug_enabled() {
            let resolved_base_url = self.base_url.trim_end_matches('/');
            eprintln!("[nanogpt-client] resolved_base_url={resolved_base_url}");
            eprintln!("[nanogpt-client] request_url={request_url}");
        }

        let request_builder = self.http.get(request_url);
        let request_builder = self.apply_auth_headers(request_builder, false);

        let response = request_builder.send().await.map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        response
            .json::<ProviderSelectionResponse>()
            .await
            .map_err(ApiError::from)
    }

    async fn send_with_retry(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        for attempts in 1..=self.max_retries + 1 {
            let error = match self.send_raw_request(request).await {
                Ok(response) => match expect_success(response).await {
                    Ok(response) => return Ok(response),
                    Err(error) if error.is_retryable() => error,
                    Err(error) => return Err(error),
                },
                Err(error) if error.is_retryable() => error,
                Err(error) => return Err(error),
            };

            if attempts > self.max_retries {
                return Err(ApiError::RetriesExhausted {
                    attempts,
                    last_error: Box::new(error),
                });
            }

            tokio::time::sleep(self.backoff_for_attempt(attempts)?).await;
        }

        Err(ApiError::RetriesExhausted {
            attempts: self.max_retries + 1,
            last_error: Box::new(ApiError::BackoffOverflow {
                attempt: self.max_retries + 1,
                base_delay: self.initial_backoff,
            }),
        })
    }

    async fn send_raw_request(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        let request_url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.messages_path()
        );
        if nanogpt_client_debug_enabled() {
            let resolved_base_url = self.base_url.trim_end_matches('/');
            eprintln!("[nanogpt-client] resolved_base_url={resolved_base_url}");
            eprintln!("[nanogpt-client] request_url={request_url}");
        }
        let request_builder = self
            .http
            .post(&request_url)
            .header("content-type", "application/json");
        let request_builder = self.apply_auth_headers(request_builder, true);

        request_builder
            .json(request)
            .send()
            .await
            .map_err(ApiError::from)
    }

    async fn send_chat_completion_raw(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<reqwest::Response, ApiError> {
        let request_url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.chat_completions_path()
        );
        if nanogpt_client_debug_enabled() {
            let resolved_base_url = self.base_url.trim_end_matches('/');
            eprintln!("[nanogpt-client] resolved_base_url={resolved_base_url}");
            eprintln!("[nanogpt-client] request_url={request_url}");
        }
        let request_builder = self
            .http
            .post(&request_url)
            .header("content-type", "application/json");
        let request_builder = self.apply_auth_headers(request_builder, true);

        request_builder
            .json(request)
            .send()
            .await
            .map_err(ApiError::from)
    }

    async fn send_get_request(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<reqwest::Response, ApiError> {
        let request_url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        if nanogpt_client_debug_enabled() {
            let resolved_base_url = self.base_url.trim_end_matches('/');
            eprintln!("[nanogpt-client] resolved_base_url={resolved_base_url}");
            eprintln!("[nanogpt-client] request_url={request_url}");
        }

        let request_builder = self.http.get(&request_url).query(query);
        let request_builder = self.apply_auth_headers(request_builder, false);

        let response = request_builder.send().await.map_err(ApiError::from)?;
        expect_success(response).await
    }

    fn apply_auth_headers(
        &self,
        request_builder: reqwest::RequestBuilder,
        include_provider: bool,
    ) -> reqwest::RequestBuilder {
        let debug = nanogpt_client_debug_enabled();
        let request_builder = if self.api_key.is_empty() {
            if debug {
                eprintln!("[nanogpt-client] headers authorization=<absent> x-api-key=<absent>");
            }
            request_builder
        } else {
            if debug {
                eprintln!(
                    "[nanogpt-client] headers x-api-key=[REDACTED] authorization=Bearer [REDACTED]"
                );
            }
            request_builder
                .bearer_auth(&self.api_key)
                .header("x-api-key", &self.api_key)
        };

        if include_provider {
            if let Some(provider) = &self.provider {
                if debug {
                    eprintln!("[nanogpt-client] headers provider={provider} mode=paygo");
                }
                request_builder
                    .header("provider", provider)
                    .header("billing-mode", "paygo")
            } else if self.force_paygo {
                if debug {
                    eprintln!("[nanogpt-client] headers billing-mode=paygo");
                }
                request_builder.header("billing-mode", "paygo")
            } else {
                request_builder
            }
        } else {
            request_builder
        }
    }

    fn messages_path(&self) -> &'static str {
        "/v1/messages"
    }

    fn chat_completions_path(&self) -> &'static str {
        "/v1/chat/completions"
    }

    fn normalize_message_request(&self, request: &MessageRequest) -> MessageRequest {
        request.clone()
    }

    fn backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        if attempt == 0 {
            return Ok(self.initial_backoff.min(self.max_backoff));
        }

        let multiplier = 2_u32
            .checked_pow(attempt.saturating_sub(1))
            .ok_or(ApiError::BackoffOverflow {
                attempt,
                base_delay: self.initial_backoff,
            })?;
        Ok(self
            .initial_backoff
            .checked_mul(multiplier)
            .map_or(self.max_backoff, |delay| delay.min(self.max_backoff)))
    }
}

fn read_api_key() -> Result<String, ApiError> {
    resolve_api_key_for(ApiService::NanoGpt)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiService {
    NanoGpt,
}

impl ApiService {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        "nanogpt"
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        "NanoGPT"
    }
}

pub fn resolve_api_key_for(service: ApiService) -> Result<String, ApiError> {
    let _ = service;
    match std::env::var(service_api_key_env(ApiService::NanoGpt)) {
        Ok(api_key) if !api_key.is_empty() => Ok(api_key),
        Ok(_) => Err(ApiError::MissingApiKey),
        Err(std::env::VarError::NotPresent) => {
            read_api_key_from_credentials_file(ApiService::NanoGpt).ok_or(ApiError::MissingApiKey)
        }
        Err(error) => Err(ApiError::from(error)),
    }
}

pub fn resolve_api_key() -> Result<String, ApiError> {
    read_api_key()
}

#[must_use]
pub fn resolve_base_url_for(service: ApiService) -> String {
    let _ = service;
    std::env::var("NANOGPT_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

#[must_use]
pub fn resolve_root_url_for(service: ApiService) -> String {
    let _ = service;
    let base = resolve_base_url_for(ApiService::NanoGpt);
    let trimmed = base.trim_end_matches('/');
    trimmed.strip_suffix("/api").unwrap_or(trimmed).to_string()
}

fn read_api_key_from_credentials_file(_service: ApiService) -> Option<String> {
    let path = credentials_path()?;
    let contents = fs::read_to_string(path).ok()?;
    let parsed = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    parsed
        .get("nanogpt_api_key")
        .and_then(serde_json::Value::as_str)
        .or_else(|| parsed.get("apiKey").and_then(serde_json::Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn service_api_key_env(_service: ApiService) -> &'static str {
    "NANOGPT_API_KEY"
}

fn credentials_path() -> Option<PathBuf> {
    Some(milancode_config_home()?.join("credentials.json"))
}

fn request_id_from_headers(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .or_else(|| headers.get(ALT_REQUEST_ID_HEADER))
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn providers_url(base_url: &str, canonical_id: &str) -> Result<reqwest::Url, ApiError> {
    let mut url =
        reqwest::Url::parse(&format!("{}/", base_url.trim_end_matches('/'))).map_err(|error| {
            ApiError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?;
    let mut segments = url.path_segments_mut().map_err(|()| {
        ApiError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid base url",
        ))
    })?;
    segments.pop_if_empty();
    segments.push("models");
    segments.push(canonical_id);
    segments.push("providers");
    drop(segments);
    Ok(url)
}

#[derive(Debug)]
pub struct MessageStream {
    request_id: Option<String>,
    state: MessageStreamState,
    pending: VecDeque<StreamEvent>,
}

#[derive(Debug)]
enum MessageStreamState {
    Http {
        response: reqwest::Response,
        parser: SseParser,
        done: bool,
    },
}

impl MessageStream {
    fn from_http_response(response: reqwest::Response) -> Self {
        Self {
            request_id: request_id_from_headers(response.headers()),
            state: MessageStreamState::Http {
                response,
                parser: SseParser::new(),
                done: false,
            },
            pending: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            match &mut self.state {
                MessageStreamState::Http {
                    response,
                    parser,
                    done,
                } => {
                    if *done {
                        let remaining = parser.finish()?;
                        self.pending.extend(remaining);
                        if let Some(event) = self.pending.pop_front() {
                            return Ok(Some(event));
                        }
                        return Ok(None);
                    }

                    match response.chunk().await? {
                        Some(chunk) => {
                            self.pending.extend(parser.push(&chunk)?);
                        }
                        None => {
                            *done = true;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct NanoGptErrorEnvelope {
    error: NanoGptErrorBody,
}

#[derive(Debug, Deserialize)]
struct NanoGptErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

async fn expect_success(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.map_err(ApiError::from)?;
    if let Ok(parsed) = serde_json::from_str::<NanoGptErrorEnvelope>(&body) {
        return Err(ApiError::Api {
            status,
            error_type: Some(parsed.error.error_type),
            message: Some(parsed.error.message),
            body,
            retryable: is_retryable_status(status),
        });
    }

    Err(ApiError::Api {
        status,
        error_type: None,
        message: None,
        body,
        retryable: is_retryable_status(status),
    })
}

#[cfg(test)]
mod tests {
    use super::{ALT_REQUEST_ID_HEADER, REQUEST_ID_HEADER};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use crate::types::{ContentBlockDelta, MessageRequest};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env test lock should not be poisoned")
    }

    fn temp_config_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "milancode-api-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should be after epoch")
                .as_nanos()
        ))
    }

    #[test]
    fn read_api_key_requires_presence() {
        let _guard = env_lock();
        let root = temp_config_home();
        std::fs::create_dir_all(&root).expect("config dir should exist");
        std::env::remove_var("NANOGPT_API_KEY");
        std::env::set_var("MILANCODE_CONFIG_HOME", &root);
        let error = super::read_api_key().expect_err("missing key should error");
        assert!(matches!(error, crate::error::ApiError::MissingApiKey));
        std::env::remove_var("MILANCODE_CONFIG_HOME");
        std::fs::remove_dir_all(root).expect("temp config dir should be removed");
    }

    #[test]
    fn read_api_key_requires_non_empty_value() {
        let _guard = env_lock();
        let root = temp_config_home();
        std::fs::create_dir_all(&root).expect("config dir should exist");
        std::env::set_var("NANOGPT_API_KEY", "");
        std::env::set_var("MILANCODE_CONFIG_HOME", &root);
        let error = super::read_api_key().expect_err("empty key should error");
        assert!(matches!(error, crate::error::ApiError::MissingApiKey));
        std::env::remove_var("NANOGPT_API_KEY");
        std::env::remove_var("MILANCODE_CONFIG_HOME");
        std::fs::remove_dir_all(root).expect("temp config dir should be removed");
    }

    #[test]
    fn read_api_key_uses_nanogpt_env() {
        let _guard = env_lock();
        let root = temp_config_home();
        std::fs::create_dir_all(&root).expect("config dir should exist");
        std::env::set_var("NANOGPT_API_KEY", "nano-key");
        std::env::set_var("MILANCODE_CONFIG_HOME", &root);
        assert_eq!(
            super::read_api_key().expect("api key should load"),
            "nano-key"
        );
        std::env::remove_var("NANOGPT_API_KEY");
        std::env::remove_var("MILANCODE_CONFIG_HOME");
        std::fs::remove_dir_all(root).expect("temp config dir should be removed");
    }

    #[test]
    fn read_base_url_defaults_to_nanogpt_messages_api_root() {
        let _guard = env_lock();
        std::env::remove_var("NANOGPT_BASE_URL");
        assert_eq!(
            super::resolve_base_url_for(super::ApiService::NanoGpt),
            "https://nano-gpt.com/api"
        );
    }

    #[test]
    fn read_api_key_uses_milancode_credentials_file() {
        let _guard = env_lock();
        let root = temp_config_home();
        std::fs::create_dir_all(&root).expect("config dir should exist");
        std::fs::write(
            root.join("credentials.json"),
            r#"{"nanogpt_api_key":"from-credentials"}"#,
        )
        .expect("credentials should write");

        std::env::remove_var("NANOGPT_API_KEY");
        std::env::set_var("MILANCODE_CONFIG_HOME", &root);
        assert_eq!(
            super::read_api_key().expect("api key should load"),
            "from-credentials"
        );

        std::env::remove_var("MILANCODE_CONFIG_HOME");
        std::fs::remove_dir_all(root).expect("temp config dir should be removed");
    }

    #[test]
    fn message_request_stream_helper_sets_stream_true() {
        let request = MessageRequest {
            model: "openai/gpt-5.2".to_string(),
            max_tokens: 64,
            messages: vec![],
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            reasoning_effort: None,
            fast_mode: false,
            stream: false,
        };

        assert!(request.with_streaming().stream);
    }

    #[test]
    fn backoff_doubles_until_maximum() {
        let client = super::NanoGptClient::new("test-key").with_retry_policy(
            3,
            Duration::from_millis(10),
            Duration::from_millis(25),
        );
        assert_eq!(
            client.backoff_for_attempt(1).expect("attempt 1"),
            Duration::from_millis(10)
        );
        assert_eq!(
            client.backoff_for_attempt(2).expect("attempt 2"),
            Duration::from_millis(20)
        );
        assert_eq!(
            client.backoff_for_attempt(3).expect("attempt 3"),
            Duration::from_millis(25)
        );
    }

    #[test]
    fn retryable_statuses_are_detected() {
        assert!(super::is_retryable_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(super::is_retryable_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!super::is_retryable_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
    }

    #[test]
    fn tool_delta_variant_round_trips() {
        let delta = ContentBlockDelta::InputJsonDelta {
            partial_json: "{\"city\":\"Paris\"}".to_string(),
        };
        let encoded = serde_json::to_string(&delta).expect("delta should serialize");
        let decoded: ContentBlockDelta =
            serde_json::from_str(&encoded).expect("delta should deserialize");
        assert_eq!(decoded, delta);
    }

    #[test]
    fn request_id_uses_primary_or_fallback_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(REQUEST_ID_HEADER, "req_primary".parse().expect("header"));
        assert_eq!(
            super::request_id_from_headers(&headers).as_deref(),
            Some("req_primary")
        );

        headers.clear();
        headers.insert(
            ALT_REQUEST_ID_HEADER,
            "req_fallback".parse().expect("header"),
        );
        assert_eq!(
            super::request_id_from_headers(&headers).as_deref(),
            Some("req_fallback")
        );
    }
}
