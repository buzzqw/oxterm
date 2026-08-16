#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::logging::LOGGER;

pub const PROVIDERS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "openai",
        "OpenAI",
        "https://api.openai.com/v1/chat/completions",
        "gpt-4o",
        "openai",
    ),
    (
        "claude",
        "Claude (Anthropic)",
        "https://api.anthropic.com/v1/messages",
        "claude-sonnet-4-6",
        "claude",
    ),
    (
        "gemini",
        "Google Gemini",
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent",
        "gemini-2.5-flash",
        "gemini",
    ),
    (
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.com/chat/completions",
        "deepseek-chat",
        "openai",
    ),
    (
        "ollama",
        "Ollama (Local)",
        "http://localhost:11434/v1/chat/completions",
        "llama3",
        "ollama",
    ),
    (
        "custom",
        "Custom OpenAI API",
        "http://localhost:8080/v1/chat/completions",
        "llama3",
        "openai",
    ),
];

pub fn provider_info(provider: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    PROVIDERS
        .iter()
        .find(|p| p.0 == provider)
        .map(|(_, name, url, model, proto)| (*name, *url, *model, *proto))
}

pub fn provider_keys() -> Vec<&'static str> {
    PROVIDERS.iter().map(|p| p.0).collect()
}

#[derive(Debug)]
pub enum AiError {
    Cancelled,
    Http(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Cancelled => write!(f, "AI request cancelled"),
            AiError::Http(msg) => write!(f, "{}", msg),
        }
    }
}

const MAX_MESSAGE_PAIRS: usize = 20;

static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn agent() -> &'static ureq::Agent {
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(120))
            .build()
    })
}

pub struct AIClient {
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    protocol: String,
    messages: Mutex<Vec<Value>>,
    system_prompt: String,
}

impl AIClient {
    pub fn new(
        provider: &str,
        api_key: &str,
        model: Option<&str>,
        base_url: &str,
    ) -> Result<AIClient, String> {
        let info = provider_info(provider).ok_or_else(|| format!("Unknown provider: {}", provider))?;
        let model = model
            .map(|m| m.to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| info.2.to_string());
        let base_url = if base_url.is_empty() {
            info.1.to_string()
        } else {
            base_url.to_string()
        };
        Ok(AIClient {
            provider: provider.to_string(),
            api_key: api_key.to_string(),
            model,
            base_url,
            protocol: info.3.to_string(),
            messages: Mutex::new(Vec::new()),
            system_prompt: String::new(),
        })
    }

    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = prompt.to_string();
    }

    pub fn reset(&self) {
        *self.messages.lock().unwrap() = Vec::new();
    }

    fn truncate_messages(&self) {
        let mut msgs = self.messages.lock().unwrap();
        let system: Vec<Value> = msgs
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
            .cloned()
            .collect();
        let non_system: Vec<Value> = msgs
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
            .cloned()
            .collect();
        if non_system.len() > MAX_MESSAGE_PAIRS * 2 {
            let keep = non_system[non_system.len() - MAX_MESSAGE_PAIRS * 2..].to_vec();
            *msgs = system;
            msgs.extend(keep);
        }
    }

    fn has_system_message(&self) -> bool {
        self.messages
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
    }

    fn add_message(&self, role: &str, content: &str) {
        self.messages
            .lock()
            .unwrap()
            .push(json!({"role": role, "content": content}));
    }

    fn remove_last_user(&self) {
        let mut msgs = self.messages.lock().unwrap();
        if let Some(pos) = msgs.iter().rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("user")) {
            msgs.remove(pos);
        }
    }

    fn remove_last_system(&self) {
        let mut msgs = self.messages.lock().unwrap();
        if let Some(pos) = msgs.iter().position(|m| m.get("role").and_then(|r| r.as_str()) == Some("system")) {
            msgs.remove(pos);
        }
    }

    pub fn cancel(&self) {
        // Cancellation is signalled via the shared AtomicBool passed to
        // chat_stream; the worker unwinds on the next read iteration.
    }

    pub fn chat(&self, message: &str) -> Result<String, String> {
        self.truncate_messages();
        let mut system_message = None;
        if !self.system_prompt.is_empty() && !self.has_system_message() {
            self.add_message("system", &self.system_prompt);
            system_message = Some(());
        }
        self.add_message("user", message);
        let result = match self.protocol.as_str() {
            "claude" => self.call_claude(false),
            "gemini" => self.call_gemini(false),
            _ => self.call_openai(false),
        };
        if result.is_err() {
            self.remove_last_user();
            if system_message.is_some() {
                self.remove_last_system();
            }
        }
        result
    }

    /// Stream a chat response. `on_chunk` is invoked for each text delta and
    /// should return `true` to keep going (the terminal stops when cancelled).
    pub fn chat_stream<F>(
        &self,
        message: &str,
        cancel: &AtomicBool,
        mut on_chunk: F,
    ) -> Result<(), AiError>
    where
        F: FnMut(&str),
    {
        self.truncate_messages();
        let mut system_message = None;
        if !self.system_prompt.is_empty() && !self.has_system_message() {
            self.add_message("system", &self.system_prompt);
            system_message = Some(());
        }
        self.add_message("user", message);
        let result = match self.protocol.as_str() {
            "claude" => self.call_claude_stream(cancel, &mut on_chunk),
            "gemini" => self.call_gemini_stream(cancel, &mut on_chunk),
            _ => self.call_openai_stream(cancel, &mut on_chunk),
        };
        if result.is_err() {
            self.remove_last_user();
            if system_message.is_some() {
                self.remove_last_system();
            }
        }
        result
    }

    fn open_stream(&self, url: &str, headers: &BTreeMap<String, String>, body: Value) -> Result<ureq::Response, AiError> {
        let mut req = agent().post(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        let body_str = body.to_string();
        req.send_bytes(body_str.as_bytes())
            .map_err(|e| AiError::Http(e.to_string()))
    }

    fn auth_headers(&self) -> BTreeMap<String, String> {
        let mut h = BTreeMap::new();
        h.insert("Content-Type".to_string(), "application/json".to_string());
        if !self.api_key.is_empty() && self.protocol != "ollama" {
            h.insert(
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key),
            );
        }
        h
    }

    fn claude_headers(&self) -> BTreeMap<String, String> {
        let mut h = BTreeMap::new();
        h.insert("x-api-key".to_string(), self.api_key.clone());
        h.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        h.insert("Content-Type".to_string(), "application/json".to_string());
        h
    }

    fn gemini_headers(&self) -> BTreeMap<String, String> {
        let mut h = BTreeMap::new();
        h.insert("Content-Type".to_string(), "application/json".to_string());
        h.insert("x-goog-api-key".to_string(), self.api_key.clone());
        h
    }

    fn build_messages_payload(&self, streaming: bool) -> Value {
        let msgs = self.messages.lock().unwrap();
        let mut system = String::new();
        let mut rest = Vec::new();
        for m in msgs.iter() {
            if m.get("role").and_then(|r| r.as_str()) == Some("system") {
                system = m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
            } else {
                rest.push(m.clone());
            }
        }
        let mut payload = json!({"model": self.model, "max_tokens": 8192, "messages": rest});
        if !streaming {
            // no change
        }
        if !system.is_empty() {
            payload["system"] = Value::String(system);
        }
        payload
    }

    fn build_gemini_payload(&self) -> (Value, Option<Value>) {
        let msgs = self.messages.lock().unwrap();
        let mut contents = Vec::new();
        let mut system_instruction = None;
        for m in msgs.iter() {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if role == "system" {
                system_instruction = Some(json!({"parts": [{"text": content}]}));
                continue;
            }
            let mapped = if role == "user" { "user" } else { "model" };
            contents.push(json!({"role": mapped, "parts": [{"text": content}]}));
        }
        let mut payload = json!({
            "contents": contents,
            "generationConfig": {"temperature": 0.7, "maxOutputTokens": 8192}
        });
        if let Some(si) = &system_instruction {
            payload["systemInstruction"] = si.clone();
        }
        (payload, system_instruction)
    }

    fn call_openai(&self, streaming: bool) -> Result<String, String> {
        let headers = self.auth_headers();
        let mut payload = json!({"model": self.model, "messages": self.messages.lock().unwrap().clone()});
        if streaming {
            payload["stream"] = Value::Bool(true);
        }
        let mut req = agent().post(&self.base_url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let resp = req
            .send_bytes(payload.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("HTTP {}: {}", status, truncate(&text, 300)));
        }
        let data: Value = resp.into_json().map_err(|e| e.to_string())?;
        let content = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        self.add_message("assistant", &content);
        Ok(content)
    }

    fn call_openai_stream<F: FnMut(&str)>(&self, cancel: &AtomicBool, on_chunk: &mut F) -> Result<(), AiError> {
        let headers = self.auth_headers();
        let mut payload = json!({"model": self.model, "messages": self.messages.lock().unwrap().clone()});
        payload["stream"] = Value::Bool(true);
        let resp = self.open_stream(&self.base_url, &headers, payload)?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            return Err(AiError::Http(format!("HTTP {}: {}", status, truncate(&text, 300))));
        }
        let mut full = String::new();
        let reader = std::io::BufReader::new(resp.into_reader());
        for line in reader.lines() {
            if cancel.load(Ordering::SeqCst) {
                return Err(AiError::Cancelled);
            }
            let line = line.map_err(|e| AiError::Http(e.to_string()))?;
            let line = line.trim();
            if line.is_empty() || line.starts_with(':') || !line.starts_with("data: ") {
                continue;
            }
            let data_str = &line[6..];
            if data_str == "[DONE]" {
                break;
            }
            if let Ok(chunk) = serde_json::from_str::<Value>(data_str) {
                if let Some(content) = chunk
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if !content.is_empty() {
                        full.push_str(content);
                        on_chunk(content);
                    }
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(AiError::Cancelled);
        }
        self.add_message("assistant", &full);
        Ok(())
    }

    fn call_claude(&self, streaming: bool) -> Result<String, String> {
        let headers = self.claude_headers();
        let mut payload = self.build_messages_payload(streaming);
        if streaming {
            payload["stream"] = Value::Bool(true);
        }
        let mut req = agent().post(&self.base_url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let resp = req
            .send_bytes(payload.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("HTTP {}: {}", status, truncate(&text, 300)));
        }
        let data: Value = resp.into_json().map_err(|e| e.to_string())?;
        let content = data
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        self.add_message("assistant", &content);
        Ok(content)
    }

    fn call_claude_stream<F: FnMut(&str)>(&self, cancel: &AtomicBool, on_chunk: &mut F) -> Result<(), AiError> {
        let headers = self.claude_headers();
        let mut payload = self.build_messages_payload(true);
        payload["stream"] = Value::Bool(true);
        let resp = self.open_stream(&self.base_url, &headers, payload)?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            return Err(AiError::Http(format!("HTTP {}: {}", status, truncate(&text, 300))));
        }
        let mut full = String::new();
        let mut event_type = String::new();
        let reader = std::io::BufReader::new(resp.into_reader());
        for line in reader.lines() {
            if cancel.load(Ordering::SeqCst) {
                return Err(AiError::Cancelled);
            }
            let line = line.map_err(|e| AiError::Http(e.to_string()))?;
            if line.is_empty() {
                event_type.clear();
                continue;
            }
            if let Some(ev) = line.strip_prefix("event: ") {
                event_type = ev.trim().to_string();
                continue;
            }
            if !line.starts_with("data: ") {
                continue;
            }
            let data_str = &line[6..];
            if event_type == "error" {
                return Err(AiError::Http(truncate(data_str, 300)));
            }
            if let Ok(event) = serde_json::from_str::<Value>(data_str) {
                if event.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
                    if let Some(text) = event
                        .get("delta")
                        .and_then(|d| d.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        full.push_str(text);
                        on_chunk(text);
                    }
                } else if event.get("type").and_then(|t| t.as_str()) == Some("message_stop") {
                    break;
                } else if event.get("type").and_then(|t| t.as_str()) == Some("error") {
                    let msg = event
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("claude error");
                    return Err(AiError::Http(msg.to_string()));
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(AiError::Cancelled);
        }
        self.add_message("assistant", &full);
        Ok(())
    }

    fn gemini_url(&self, streaming: bool) -> String {
        let mut url = self.base_url.replace("{model}", &self.model);
        if streaming {
            if !url.contains(":streamGenerateContent") {
                url = url.replace(":generateContent", ":streamGenerateContent");
            }
        } else {
            url = url.replace(":streamGenerateContent", ":generateContent");
        }
        url
    }

    fn call_gemini(&self, _streaming: bool) -> Result<String, String> {
        let url = self.gemini_url(false);
        let headers = self.gemini_headers();
        let (payload, _) = self.build_gemini_payload();
        let mut req = agent().post(&url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let resp = req
            .send_bytes(payload.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("HTTP {}: {}", status, truncate(&text, 300)));
        }
        let data: Value = resp.into_json().map_err(|e| e.to_string())?;
        let content = data
            .get("candidates")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        self.add_message("assistant", &content);
        Ok(content)
    }

    fn call_gemini_stream<F: FnMut(&str)>(&self, cancel: &AtomicBool, on_chunk: &mut F) -> Result<(), AiError> {
        let url = self.gemini_url(true);
        let headers = self.gemini_headers();
        let (payload, _) = self.build_gemini_payload();
        let resp = self.open_stream(&url, &headers, payload)?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            return Err(AiError::Http(format!("HTTP {}: {}", status, truncate(&text, 300))));
        }
        let mut full = String::new();
        let reader = std::io::BufReader::new(resp.into_reader());
        for line in reader.lines() {
            if cancel.load(Ordering::SeqCst) {
                return Err(AiError::Cancelled);
            }
            let line = line.map_err(|e| AiError::Http(e.to_string()))?;
            let line = line.trim();
            if line.is_empty() || !line.starts_with("data: ") {
                continue;
            }
            let data_str = &line[6..];
            if let Ok(event) = serde_json::from_str::<Value>(data_str) {
                if let Some(text) = event
                    .get("candidates")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("content"))
                    .and_then(|c| c.get("parts"))
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("text"))
                    .and_then(|c| c.as_str())
                {
                    full.push_str(text);
                    on_chunk(text);
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(AiError::Cancelled);
        }
        self.add_message("assistant", &full);
        Ok(())
    }
}

pub fn fetch_models(provider: &str, api_key: &str, base_url: &str) -> Vec<String> {
    let resp = if provider == "ollama" {
        let mut url = if base_url.is_empty() {
            "http://localhost:11434".to_string()
        } else {
            base_url.replace("/v1/chat/completions", "").trim_end_matches('/').to_string()
        };
        if url.is_empty() {
            url = "http://localhost:11434".to_string();
        }
        agent()
            .get(&format!("{}/api/tags", url.trim_end_matches('/')))
            .timeout(Duration::from_secs(5))
            .call()
    } else if provider == "custom" && !base_url.is_empty() {
        let base = base_url
            .rsplit_once("/chat/completions")
            .map(|(b, _)| b.trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| base_url.trim_end_matches('/').to_string());
        let mut req = agent().get(&format!("{}/models", base)).timeout(Duration::from_secs(5));
        if !api_key.is_empty() {
            req = req.set("Authorization", &format!("Bearer {}", api_key));
        }
        req.call()
    } else if (provider == "openai" || provider == "deepseek") && !api_key.is_empty() {
        let Some((_, _, url, _)) = provider_info(provider) else {
            return Vec::new();
        };
        let base = url
            .rsplit_once("/chat/completions")
            .map(|(b, _)| b.trim_end_matches('/').to_string())
            .unwrap_or_default();
        let mut req = agent().get(&format!("{}/models", base)).timeout(Duration::from_secs(5));
        req = req.set("Authorization", &format!("Bearer {}", api_key));
        req.call()
    } else {
        return Vec::new();
    };
    let resp = match resp {
        Ok(r) if r.status() == 200 => r,
        _ => return Vec::new(),
    };
    let data: Value = match resp.into_json() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if provider == "ollama" {
        if let Some(models) = data.get("models").and_then(|m| m.as_array()) {
            for m in models {
                let name = m
                    .get("name")
                    .and_then(|n| n.as_str())
                    .or_else(|| m.get("model").and_then(|n| n.as_str()))
                    .unwrap_or(&m.to_string())
                    .to_string();
                out.push(name);
            }
        }
    } else if let Some(items) = data.get("data").and_then(|d| d.as_array()) {
        for m in items {
            let name = m
                .get("id")
                .and_then(|n| n.as_str())
                .or_else(|| m.get("name").and_then(|n| n.as_str()))
                .unwrap_or(&m.to_string())
                .to_string();
            out.push(name);
        }
    }
    if provider == "openai" || provider == "deepseek" {
        out.sort();
    }
    out
}

pub fn ping_provider(provider: &str, url: &str) -> bool {
    let result = if provider == "ollama" {
        let u = if url.is_empty() {
            "http://localhost:11434".to_string()
        } else {
            url.replace("/v1/chat/completions", "").trim_end_matches('/').to_string()
        };
        agent()
            .get(&format!("{}/api/tags", u.trim_end_matches('/')))
            .timeout(Duration::from_secs(3))
            .call()
    } else if provider == "custom" && !url.is_empty() {
        let base = url
            .rsplit_once("/chat/completions")
            .map(|(b, _)| b.trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| url.trim_end_matches('/').to_string());
        let mut ok = false;
        for test_url in [url.to_string(), format!("{}/models", base), base.clone()] {
            if let Ok(r) = agent().get(&test_url).timeout(Duration::from_secs(3)).call() {
                if r.status() == 200 {
                    ok = true;
                    break;
                }
            }
        }
        return ok;
    } else {
        return false;
    };
    match result {
        Ok(r) => r.status() == 200,
        Err(_) => false,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max).collect::<String>() + "..."
    } else {
        s.to_string()
    }
}

pub fn log_provider_error(provider: &str, msg: &str) {
    LOGGER.error(&format!("ai_models_fetch_failed provider={} error={}", provider, msg));
}
