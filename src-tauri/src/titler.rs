//! fast model 自动起标题 + 会话摘要（P3-5）。
//!
//! 首轮 TurnComplete 后，用会话绑定的服务商与 fast model 真实调用一次补全接口，
//! 生成 ≤16 字标题与一句话摘要，写入 `session.title` / `session.summary`。
//!
//! 披露与开关（隐私要求：默认外发必须显式披露 + 可关）：
//! - 设置 `general.autoTitleSessions` 可关闭（默认开）；
//! - 内容只发给用户自己绑定、已在计费的服务商，不发第三方；
//! - 订阅登录（无 Key）服务商 Helm 不持有凭证，自动跳过，标题保持首条消息截断。

use crate::providers::{
    provider_completion_endpoint, AuthMethod, KeyringSecretStore, Protocol, ProviderStore,
};
use crate::sessions::SessionHistoryStore;
use crate::settings::load_app_settings_from_store;
use tauri::{AppHandle, Emitter, Manager};

const MAX_SNIPPET_CHARS: usize = 600;
const MAX_TITLE_CHARS: usize = 24;

/// TurnComplete 后调用：条件满足则后台生成标题（不阻塞事件流）
pub fn maybe_generate_title(app: &AppHandle, history_session_id: &str) {
    let app = app.clone();
    let history_session_id = history_session_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = generate_title(&app, &history_session_id).await {
            // 起标题是锦上添花：失败只留诊断，不打扰用户；摘要仍为空，下一轮会再试
            eprintln!("[titler] 会话 {history_session_id} 自动起标题跳过/失败：{err}");
        }
    });
}

async fn generate_title(app: &AppHandle, history_session_id: &str) -> Result<(), String> {
    let history_store = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let settings = load_app_settings_from_store(&history_store)?;
    if !settings.general.auto_title_sessions {
        return Ok(());
    }
    if !history_store.session_needs_auto_title(history_session_id)? {
        return Ok(());
    }

    let detail = history_store.get_session(history_session_id)?;
    let user_text = detail
        .messages
        .iter()
        .find(|message| matches!(message.role, crate::protocol::Role::User))
        .map(|message| message.text.clone())
        .ok_or("没有用户消息")?;
    let assistant_text = detail
        .messages
        .iter()
        .find(|message| matches!(message.role, crate::protocol::Role::Assistant))
        .map(|message| message.text.clone())
        .ok_or("没有助手回复")?;

    let provider_store = app
        .try_state::<ProviderStore<KeyringSecretStore>>()
        .ok_or("服务商存储未初始化")?;
    let config = provider_store.load()?;
    let provider_id = history_store.session_provider_id(history_session_id)?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("会话服务商不存在：{provider_id}"))?;
    // 订阅登录（无 Key）：Helm 不持有凭证，无法代表用户调 API——跳过而不是硬错
    let has_key = provider
        .key_ref
        .as_deref()
        .is_some_and(|key_ref| !key_ref.trim().is_empty());
    if !has_key {
        return Err("服务商未存密钥（订阅登录模式），跳过自动起标题".to_string());
    }
    if !matches!(provider.auth_method, AuthMethod::ApiKey | AuthMethod::OAuth) {
        return Err("云凭证/本地服务商暂不支持自动起标题".to_string());
    }
    let api_key = provider_store.provider_secret(&provider.id)?;

    // fast model 优先，没配就用主模型
    let model = config
        .bindings
        .iter()
        .find(|binding| binding.provider_id == provider.id)
        .and_then(|binding| binding.fast_model.clone().filter(|model| !model.is_empty()))
        .unwrap_or_else(|| detail.summary.model.clone());

    let prompt = title_prompt(&user_text, &assistant_text);
    let raw = request_completion(
        &provider.protocol,
        &provider.base_url,
        &api_key,
        &model,
        &prompt,
    )
    .await?;
    let (title, summary) = parse_title_summary(&raw, &user_text);

    history_store.set_session_title_and_summary(history_session_id, &title, &summary)?;
    // 通知前端刷新会话列表（侧栏标题即时更新）
    let _ = app.emit("helm-sessions-changed", history_session_id);
    Ok(())
}

/// 构造起标题的 prompt（截断首轮内容，避免长对话浪费 token）
pub fn title_prompt(user_text: &str, assistant_text: &str) -> String {
    format!(
        "根据下面这轮对话，输出两行中文：\n第一行：不超过 16 个字的会话标题（不要引号、句号）。\n第二行：一句话摘要（不超过 40 字）。\n只输出这两行，不要任何其他内容。\n\n用户：{}\n\n助手：{}",
        truncate_chars(user_text, MAX_SNIPPET_CHARS),
        truncate_chars(assistant_text, MAX_SNIPPET_CHARS),
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// 解析模型输出：第一行标题、第二行摘要；输出不合规时回退到首条消息截断
pub fn parse_title_summary(raw: &str, fallback_user_text: &str) -> (String, String) {
    let mut lines = raw
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '-', '*', ' '])
                .trim_matches(['"', '“', '”', '「', '」'])
                .trim()
        })
        .filter(|line| !line.is_empty());
    let title_line = lines.next().unwrap_or("");
    let summary_line = lines.next().unwrap_or("");

    let title = if title_line.is_empty() {
        truncate_chars(fallback_user_text.trim(), 20)
    } else {
        truncate_chars(title_line, MAX_TITLE_CHARS)
    };
    let summary = if summary_line.is_empty() {
        title.clone()
    } else {
        truncate_chars(summary_line, 80)
    };
    (title, summary)
}

async fn request_completion(
    protocol: &Protocol,
    base_url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let endpoint = provider_completion_endpoint(protocol, base_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;

    match protocol {
        Protocol::Anthropic => {
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 120,
                "messages": [{ "role": "user", "content": prompt }],
            });
            let response = client
                .post(endpoint)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("请求失败：{e}"))?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!("HTTP {}", status.as_u16()));
            }
            let payload: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("解析响应失败：{e}"))?;
            payload["content"][0]["text"]
                .as_str()
                .map(|text| text.to_string())
                .ok_or_else(|| "响应缺少 content[0].text".to_string())
        }
        Protocol::OpenAiResponses | Protocol::OpenAiChat => {
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 120,
                "messages": [{ "role": "user", "content": prompt }],
            });
            let response = client
                .post(endpoint)
                .bearer_auth(api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("请求失败：{e}"))?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!("HTTP {}", status.as_u16()));
            }
            let payload: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("解析响应失败：{e}"))?;
            payload["choices"][0]["message"]["content"]
                .as_str()
                .map(|text| text.to_string())
                .ok_or_else(|| "响应缺少 choices[0].message.content".to_string())
        }
        Protocol::Bedrock | Protocol::Vertex => Err("该协议暂不支持自动起标题".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_title_summary, title_prompt};

    #[test]
    fn title_prompt_truncates_long_first_turn() {
        let long_user = "长".repeat(2000);
        let prompt = title_prompt(&long_user, "回复");
        assert!(prompt.chars().count() < 800, "prompt 必须截断长对话");
        assert!(prompt.contains('…'));
        assert!(prompt.contains("16 个字"));
    }

    #[test]
    fn parse_title_summary_takes_two_lines_and_strips_quotes() {
        let (title, summary) = parse_title_summary(
            "「修复登录超时」\n排查并修复了登录接口 30s 超时的问题。\n多余的第三行",
            "fallback",
        );
        assert_eq!(title, "修复登录超时");
        assert_eq!(summary, "排查并修复了登录接口 30s 超时的问题。");
    }

    #[test]
    fn parse_title_summary_falls_back_to_user_text_when_output_is_garbage() {
        let (title, summary) = parse_title_summary("   \n\n", "帮我看看这个报错是怎么回事");
        assert_eq!(title, "帮我看看这个报错是怎么回事");
        assert_eq!(summary, title);
    }

    #[test]
    fn parse_title_summary_caps_overlong_title() {
        let raw = format!("{}\n摘要", "标".repeat(60));
        let (title, _) = parse_title_summary(&raw, "fallback");
        assert!(title.chars().count() <= 25, "超长标题必须截断：{title}");
    }
}
