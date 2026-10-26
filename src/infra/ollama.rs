use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

use crate::config::OllamaConfig;

// Local models can take minutes to chew through a long prompt.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct OllamaClient {
    config: OllamaConfig,
    client: reqwest::blocking::Client,
    /// Print each prompt and raw reply to stderr (EPISODE_MATCHER_OLLAMA_DEBUG).
    debug: bool,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatMessage,
    /// Prompt tokens Ollama actually evaluated.
    prompt_eval_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: String,
}

impl OllamaClient {
    pub fn new(config: OllamaConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        let debug = std::env::var_os("EPISODE_MATCHER_OLLAMA_DEBUG").is_some_and(|v| !v.is_empty());
        Ok(Self {
            config,
            client,
            debug,
        })
    }

    /// Sends a chat request constrained to `schema` and returns the raw JSON text of the reply.
    pub fn chat_json(&self, system: &str, user: &str, schema: serde_json::Value) -> Result<String> {
        if self.debug {
            eprintln!("--- Ollama system prompt ---\n{system}\n--- Ollama user prompt ---\n{user}");
        }

        let body = serde_json::json!({
            "model": self.config.model,
            "stream": false,
            "format": schema,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "options": {
                "temperature": 0,
                // Ollama's default context is small and silently truncates long prompts.
                "num_ctx": self.config.num_ctx,
            },
        });

        let response = self
            .client
            .post(format!("{}/api/chat", self.config.url))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .context("Failed to reach Ollama")?;

        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            bail!("Ollama request failed: HTTP {status}: {text}");
        }

        let chat: ChatResponse =
            serde_json::from_str(&text).context("Unexpected response from Ollama")?;

        if prompt_filled_context(chat.prompt_eval_count, self.config.num_ctx) {
            eprintln!(
                "Warning: the prompt filled Ollama's whole {}-token context window and was probably \
                 truncated. Raise EPISODE_MATCHER_OLLAMA_NUM_CTX or use --hint to offer fewer episodes.",
                self.config.num_ctx
            );
        }

        if self.debug {
            eprintln!("--- Ollama reply ---\n{}\n---", chat.message.content);
        }

        Ok(chat.message.content)
    }
}

/// Ollama silently drops the start of a prompt that doesn't fit, so a prompt that used up the
/// whole window was almost certainly cut off.
fn prompt_filled_context(prompt_eval_count: Option<u64>, num_ctx: u32) -> bool {
    prompt_eval_count.is_some_and(|used| used >= u64::from(num_ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves one canned response and returns (request line, request body) it received.
    fn serve_once(
        response_body: &'static str,
        status: &'static str,
    ) -> (String, std::thread::JoinHandle<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let (head_end, content_length) = loop {
                let n = stream.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..pos]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map(|v| v.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    break (pos + 4, len);
                }
            };
            while raw.len() < head_end + content_length {
                let n = stream.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
            }
            let request_line = String::from_utf8_lossy(&raw)
                .lines()
                .next()
                .unwrap()
                .to_string();
            let body = String::from_utf8_lossy(&raw[head_end..]).to_string();

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            (request_line, body)
        });
        (url, handle)
    }

    fn client(url: String) -> OllamaClient {
        OllamaClient::new(OllamaConfig {
            url,
            model: "test-model".to_string(),
            num_ctx: 8192,
        })
        .unwrap()
    }

    #[test]
    fn test_chat_json_request_and_response() {
        let (url, server) = serve_once(
            r#"{"message":{"role":"assistant","content":"{\"season\":1}"}}"#,
            "200 OK",
        );

        let schema = serde_json::json!({"type": "object"});
        let content = client(url).chat_json("sys", "usr", schema).unwrap();
        assert_eq!(content, r#"{"season":1}"#);

        let (request_line, body) = server.join().unwrap();
        assert_eq!(request_line, "POST /api/chat HTTP/1.1");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], false);
        assert_eq!(body["format"]["type"], "object");
        assert_eq!(body["options"]["num_ctx"], 8192);
        assert_eq!(body["options"]["temperature"], 0);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "usr");
    }

    #[test]
    fn test_prompt_filled_context() {
        assert!(!prompt_filled_context(None, 8192));
        assert!(!prompt_filled_context(Some(5000), 8192));
        assert!(prompt_filled_context(Some(8192), 8192));
        assert!(prompt_filled_context(Some(9000), 8192));
    }

    #[test]
    fn test_chat_json_http_error() {
        let (url, server) = serve_once(r#"{"error":"model not found"}"#, "404 Not Found");
        let err = client(url)
            .chat_json("sys", "usr", serde_json::json!({}))
            .unwrap_err();
        server.join().unwrap();
        assert!(err.to_string().contains("404"), "{err}");
        assert!(err.to_string().contains("model not found"), "{err}");
    }
}
