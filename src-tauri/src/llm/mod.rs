//! LLM 统一接口（04 §2）。
//!
//! 对外只暴露 `complete()`。换 provider 只改 `openai_compat.rs`，
//! 捕获/引用/存储的逻辑一行都不用动。

pub mod openai_compat;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// 调用失败的原因。区分「没配置」和「调通但出错」，
/// 因为前者要走离线兜底（05 §三），后者要报给用户。
#[derive(Debug)]
pub enum LlmError {
    /// 没配 API key / base_url——直接走离线兜底，不算错误
    NotConfigured(String),
    /// 网络、超时、HTTP 报错、返回体解析失败
    Failed(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::NotConfigured(s) => write!(f, "LLM 未配置: {s}"),
            LlmError::Failed(s) => write!(f, "LLM 调用失败: {s}"),
        }
    }
}

/// LLM 配置（04 §2：API key 由用户自有，放配置文件 / 环境变量，不入 git）。
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
}

impl LlmConfig {
    /// 只取环境变量（测试与无配置文件时的路径）。
    fn from_env_only() -> Option<Self> {
        let api_key = non_empty(std::env::var("PAPER_PET_API_KEY").ok())?;
        Some(Self {
            base_url: non_empty(std::env::var("PAPER_PET_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            api_key,
            model: non_empty(std::env::var("PAPER_PET_MODEL").ok())
                .unwrap_or_else(|| "gpt-4o-mini".to_string()),
            timeout: Duration::from_secs(20),
        })
    }
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 配置查找顺序（06 §4 D / 22 项）：
/// 1. 环境变量 `PAPER_PET_API_KEY` / `PAPER_PET_BASE_URL` / `PAPER_PET_MODEL`
/// 2. 数据目录下的 `config.json`：`{"base_url": "...", "api_key": "...", "model": "..."}`
///
/// 两者都没有 → 返回 None，调用方走离线兜底（05 §三）。
fn load_config() -> Option<LlmConfig> {
    if let Some(cfg) = LlmConfig::from_env_only() {
        return Some(cfg);
    }

    let path = config_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;

    let api_key = non_empty(json.get("api_key").and_then(|v| v.as_str()).map(String::from))?;
    Some(LlmConfig {
        base_url: non_empty(json.get("base_url").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        api_key,
        model: non_empty(json.get("model").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_else(|| "gpt-4o-mini".to_string()),
        timeout: Duration::from_secs(20),
    })
}

/// 配置文件所在路径，由启动时注册（见 `set_config_path`）。
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 启动时调用一次，告诉本模块数据目录在哪。
pub fn set_config_path(dir: PathBuf) {
    let _ = CONFIG_PATH.set(dir.join("config.json"));
}

fn config_path() -> Option<PathBuf> {
    CONFIG_PATH.get().cloned()
}

/// 配置是否可用（前端/命令用它决定要不要提示用户去配 key）。
pub fn is_configured() -> bool {
    load_config().is_some()
}

/// 统一的补全接口。所有需要 LLM 的地方都走这里。
pub fn complete(system: &str, user: &str) -> Result<String, LlmError> {
    let cfg = load_config().ok_or_else(|| {
        LlmError::NotConfigured(
            "未找到 API key。请设置环境变量 PAPER_PET_API_KEY，或在数据目录的 config.json 里配置"
                .to_string(),
        )
    })?;

    openai_compat::chat_completion(&cfg, system, user)
}

/// 从模型回复里抽出 JSON 对象。
///
/// 模型经常把 JSON 包在 ```json 围栏里，或者前后带一句解释，这里做容错剥离。
pub fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();

    // 1) 直接就是 JSON
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }

    // 2) ```json ... ``` 围栏
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(after[..end].trim()) {
                return Some(v);
            }
        }
    }

    // 3) 最外层花括号
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end > start {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) {
            return Some(v);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_json() {
        let v = extract_json(r#"{"a": 1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn extracts_fenced_json() {
        let v = extract_json("好的，结果如下：\n```json\n{\"a\": 2}\n```\n希望有帮助").unwrap();
        assert_eq!(v["a"], 2);
    }

    #[test]
    fn extracts_embedded_json() {
        let v = extract_json(r#"结果是 {"a": 3} 这样"#).unwrap();
        assert_eq!(v["a"], 3);
    }

    #[test]
    fn returns_none_on_garbage() {
        assert!(extract_json("模型今天不想干活").is_none());
    }
}
