use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

use crate::config::JevConfig;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// TypeSafe's Jev classifier, which picks an episode from a fixed list of options.
#[derive(Debug, Clone)]
pub struct JevClient {
    config: JevConfig,
    client: reqwest::blocking::Client,
    /// Print each request and raw reply to stderr (EPISODE_MATCHER_JEV_DEBUG).
    debug: bool,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UsageOnly {
    model: Option<String>,
    usage: Option<Usage>,
}

impl JevClient {
    pub fn new(config: JevConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        let debug = std::env::var_os("EPISODE_MATCHER_JEV_DEBUG").is_some_and(|v| !v.is_empty());
        Ok(Self {
            config,
            client,
            debug,
        })
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Sends a request built by `matchers::jev::build_request` and returns the raw JSON reply.
    pub fn ask(&self, request: &serde_json::Value) -> Result<String> {
        if self.debug {
            eprintln!("--- Jev request ---\n{request:#}");
        }

        let response = self
            .client
            .post(format!("{}/v1/systemone", self.config.url))
            .bearer_auth(&self.config.api_key)
            .header("Content-Type", "application/json")
            .body(request.to_string())
            .send()
            .context("Failed to reach Jev")?;

        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            let hint = match status.as_u16() {
                401 => " (check JEV_API_KEY)",
                // Jev limits how long a request can be
                422 => " (if the request is too long, use --season to offer fewer episodes)",
                429 | 529 => " (Jev is busy; try again shortly)",
                _ => "",
            };
            bail!("Jev request failed: HTTP {status}{hint}: {text}");
        }

        if self.debug {
            eprintln!("--- Jev reply ---\n{text}\n---");
            if let Ok(UsageOnly { model, usage }) = serde_json::from_str(&text) {
                eprintln!(
                    "--- Jev: model {}, {} input tokens ---",
                    model.unwrap_or_default(),
                    usage.and_then(|u| u.input_tokens).unwrap_or(0)
                );
            }
        }

        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves one canned response and returns (request head, request body) it received.
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
            let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
            let body = String::from_utf8_lossy(&raw[head_end..]).to_string();

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            (head, body)
        });
        (url, handle)
    }

    fn client(url: String) -> JevClient {
        JevClient::new(JevConfig {
            url,
            api_key: "secret".to_string(),
            model: "jev-latest".to_string(),
        })
        .unwrap()
    }

    #[test]
    fn test_ask_request_and_response() {
        let (url, server) = serve_once(r#"{"answers":{}}"#, "200 OK");

        let request = serde_json::json!({"model": "jev-latest", "state": "x", "questions": {}});
        let reply = client(url).ask(&request).unwrap();
        assert_eq!(reply, r#"{"answers":{}}"#);

        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /v1/systemone HTTP/1.1"), "{head}");
        assert!(head.to_lowercase().contains("authorization: bearer secret"));
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body, request);
    }

    #[test]
    fn test_ask_http_error() {
        let (url, server) = serve_once(
            r#"{"error":"request too long"}"#,
            "422 Unprocessable Entity",
        );
        let err = client(url).ask(&serde_json::json!({})).unwrap_err();
        server.join().unwrap();
        let err = err.to_string();
        assert!(err.contains("422"), "{err}");
        assert!(err.contains("request too long"), "{err}");
        assert!(err.contains("--season"), "{err}");
    }
}
