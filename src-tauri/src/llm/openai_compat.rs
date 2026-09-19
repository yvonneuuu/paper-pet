//! OpenAI 兼容 provider（04 §2）。
//!
//! 兼容 OpenAI 以及绝大多数国产 provider——它们都提供 `/chat/completions`
//! 这一套请求/响应形状。换 provider 只需要改环境变量或 config.json，
//! **不用动这个文件**；只有当 provider 形状不同（不是 OpenAI 兼容）时才改这里。

use super::{LlmConfig, LlmError};

/// 一次 chat completion。
pub fn chat_completion(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, LlmError> {
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));

    let body = serde_json::json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        // 引用提取要的是稳定、可解析的输出，不要发挥
        "temperature": 0.0,
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(cfg.timeout)
        .build()
        .map_err(|e| LlmError::Failed(format!("创建 HTTP 客户端失败: {e}")))?;

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::Failed("请求超时，请检查网络".to_string())
            } else {
                LlmError::Failed(format!("网络请求失败: {e}"))
            }
        })?;

    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| LlmError::Failed(format!("读取响应失败: {e}")))?;

    if !status.is_success() {
        // 正文里通常有 provider 的错误说明，截断后带出去，方便现场排查
        let brief: String = text.chars().take(200).collect();
        return Err(LlmError::Failed(format!("HTTP {status}: {brief}")));
    }

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| LlmError::Failed(format!("响应不是合法 JSON: {e}")))?;

    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| LlmError::Failed("响应里没有 choices[0].message.content".to_string()))
}
