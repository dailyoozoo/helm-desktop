//! Tauri 命令 + 会话存储。
//!
//! 前端 `src/engine/transport.ts` 通过 `invoke` 调这些命令；归一化事件经
//! `app.emit("agent-event", ...)` 推回前端（见 adapter.rs）。
//! 注意：JS 侧用 camelCase 参数（`handleId`），Tauri 会自动映射到 Rust 的 snake_case（`handle_id`）。

use crate::adapter::{
    agent_environment_from_settings, approval_policy_from_settings, codex_sandbox_from_settings,
    start_claude, start_claude_with_resume, start_codex, AgentSession, ApprovalDecision, TurnMode,
};
use crate::protocol::EngineId;
use crate::providers::{
    read_engine_config_file as read_engine_config_file_from_disk, sync_provider_models,
    test_engine_connection, test_provider_connection,
    write_engine_config_file as write_engine_config_file_to_disk, AppConfig, BindingConfig,
    ConnectionResult, EngineConfig, EngineConfigFile, KeyringSecretStore, ModelConfig,
    ProviderConfig, ProviderStore, ProviderTest, TestOutcome,
};
use crate::sessions::{NewSessionRecord, SessionDetail, SessionHistoryStore, SessionSummary};
use crate::settings::load_app_settings_from_store;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// 进程会话表：handleId → 会话句柄。
/// handleId 是 Helm 内部句柄，区别于 claude 自己的 sessionId（后者来自 session_started 事件）。
#[derive(Default)]
pub struct SessionStore {
    sessions: Mutex<HashMap<String, AgentSession>>,
    history_session_ids: Mutex<HashMap<String, String>>,
    counter: AtomicU64,
}

fn ensure_budget_allows_turn(budget: &crate::sessions::Budget) -> Result<(), String> {
    if budget.stop_at_100
        && budget.monthly_limit > 0.0
        && budget.current_month_cost >= budget.monthly_limit
    {
        return Err(format!(
            "已超出本月预算（${:.2} / ${:.2}），无法发起新任务。请前往「用量与成本」调整预算。",
            budget.current_month_cost, budget.monthly_limit
        ));
    }
    Ok(())
}

impl SessionStore {
    fn next_handle(&self) -> String {
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let now_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("s-{now_nanos}-{counter}")
    }

    fn bind_history_session(
        &self,
        handle_id: &str,
        history_session_id: &str,
    ) -> Result<(), String> {
        self.history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?
            .insert(handle_id.to_string(), history_session_id.to_string());
        Ok(())
    }

    fn history_session_id_for_handle(&self, handle_id: &str) -> Result<String, String> {
        Ok(self
            .history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?
            .get(handle_id)
            .cloned()
            .unwrap_or_else(|| handle_id.to_string()))
    }
}

/// 创建会话运行时：返回内部句柄 id；真实 `claude` 进程在 send / approve 时启动。
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    engine: String,
    model: String,
    cwd: String,
) -> Result<String, String> {
    // 预算护栏：检查是否超预算
    let budget = history_store.get_budget()?;
    ensure_budget_allows_turn(&budget)?;

    let config = config_store.load()?;
    let app_settings = load_app_settings_from_store(&history_store)?;
    let mut approval_policy = approval_policy_from_settings(&app_settings);
    // 播种跨会话持久化的「始终允许」清单（P2-4）
    approval_policy.always_allow_tools = history_store.get_always_allow_tools()?;
    let codex_sandbox = codex_sandbox_from_settings(&app_settings).to_string();
    sync_history_model_prices(&history_store, &config);
    let engine = if engine.is_empty() {
        config.default_engine.clone()
    } else {
        engine
    };
    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == engine)
        .cloned()
        .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
    let model = if model.is_empty() {
        binding.primary_model.clone()
    } else {
        model
    };
    // 用量归属（P3-6）：记录本会话实际使用的服务商
    let provider_id = binding.provider_id.clone();
    let launch_binding = BindingConfig {
        primary_model: model.clone(),
        ..binding
    };
    let mut env = config_store.launch_env(&launch_binding)?;
    env.extend(agent_environment_from_settings(&app_settings));
    let bin = config
        .engine_bin(&engine)
        .filter(|bin| !bin.is_empty())
        .unwrap_or(if engine == "codex" { "codex" } else { "claude" })
        .to_string();
    let handle = store.next_handle();
    let session = match engine.as_str() {
        "claude-code" => start_claude(
            app,
            handle.clone(),
            bin,
            model.clone(),
            cwd.clone(),
            env,
            approval_policy,
        )?,
        "codex" => start_codex(
            app,
            handle.clone(),
            bin,
            model.clone(),
            cwd.clone(),
            env,
            codex_sandbox,
            vec![],
            None,
        )?,
        _ => return Err(format!("暂不支持的引擎：{engine}")),
    };
    store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .insert(handle.clone(), session);
    store.bind_history_session(&handle, &handle)?;
    history_store.create_session(NewSessionRecord {
        id: handle.clone(),
        engine: engine_id_from_str(&engine)?,
        model,
        cwd,
        created_at: unix_timestamp_seconds()?,
    })?;
    history_store.set_session_provider(&handle, &provider_id)?;
    Ok(handle)
}

#[tauri::command]
pub fn list_sessions(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Vec<SessionSummary>, String> {
    history_store.list_sessions()
}

#[tauri::command]
pub fn get_active_session(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Option<SessionDetail>, String> {
    history_store.active_session()
}

#[tauri::command]
pub fn get_session_history(
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
) -> Result<SessionDetail, String> {
    history_store.get_session(&session_id)
}

#[tauri::command]
pub async fn resume_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
) -> Result<String, String> {
    let detail = history_store.get_session(&session_id)?;
    let config = config_store.load()?;
    let app_settings = load_app_settings_from_store(&history_store)?;
    let mut approval_policy = approval_policy_from_settings(&app_settings);
    // 播种跨会话持久化的「始终允许」清单（P2-4）
    approval_policy.always_allow_tools = history_store.get_always_allow_tools()?;
    let codex_sandbox = codex_sandbox_from_settings(&app_settings).to_string();
    sync_history_model_prices(&history_store, &config);
    let engine = engine_id_to_string(detail.summary.engine);
    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == engine)
        .cloned()
        .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
    // 用量归属（P3-6）：恢复会话按当前生效绑定归属（实际计费方）
    let provider_id = binding.provider_id.clone();
    let launch_binding = BindingConfig {
        primary_model: detail.summary.model.clone(),
        ..binding
    };
    let mut env = config_store.launch_env(&launch_binding)?;
    env.extend(agent_environment_from_settings(&app_settings));
    let bin = config
        .engine_bin(&engine)
        .filter(|bin| !bin.is_empty())
        .unwrap_or(if engine == "codex" { "codex" } else { "claude" })
        .to_string();
    // 回溯过的会话：已回滚消息不进入重建上下文（P2-5）
    let context_messages: Vec<crate::sessions::SessionMessage> = detail
        .messages
        .iter()
        .filter(|message| !message.reverted)
        .cloned()
        .collect();
    let session = match detail.summary.engine {
        EngineId::ClaudeCode => start_claude_with_resume(
            app,
            detail.summary.id.clone(),
            bin,
            detail.summary.model.clone(),
            detail.summary.cwd.clone(),
            env,
            approval_policy,
            detail.summary.cli_session_id.clone(),
            context_messages,
        )?,
        EngineId::Codex => start_codex(
            app,
            detail.summary.id.clone(),
            bin,
            detail.summary.model.clone(),
            detail.summary.cwd.clone(),
            env,
            codex_sandbox,
            context_messages,
            detail.summary.cli_session_id.clone(),
        )?,
    };
    let handle = store.next_handle();
    store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .insert(handle.clone(), session);
    store.bind_history_session(&handle, &detail.summary.id)?;
    history_store.set_active_session(&detail.summary.id)?;
    history_store.set_session_provider(&detail.summary.id, &provider_id)?;
    Ok(handle)
}

#[tauri::command]
pub fn get_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
) -> Result<AppConfig, String> {
    store.load()
}

#[tauri::command]
pub fn reveal_provider_secret(
    app: AppHandle,
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<String, String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    let provider_name = store
        .load()?
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| provider.name.clone())
        .unwrap_or_else(|| provider_id.clone());
    // 明文密钥出钥匙串前必须经过系统级确认（可靠性检查 P2-7）：
    // 门槛放在后端，前端调用无法绕过。
    let confirmed = app
        .dialog()
        .message(format!(
            "即将在界面上显示服务商「{provider_name}」的明文密钥。\n\n请确认当前没有投屏、录屏或旁观。"
        ))
        .title("显示明文密钥")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "显示密钥".to_string(),
            "取消".to_string(),
        ))
        .blocking_show();
    if !confirmed {
        return Err("已取消显示密钥".to_string());
    }
    store.provider_secret(&provider_id)
}

#[tauri::command]
pub fn save_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider: ProviderConfig,
    api_key: Option<String>,
) -> Result<AppConfig, String> {
    store.save_provider(provider, api_key.as_deref())
}

#[tauri::command]
pub fn delete_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<AppConfig, String> {
    store.delete_provider(&provider_id)
}

#[tauri::command]
pub fn save_engine_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    engine: EngineConfig,
) -> Result<AppConfig, String> {
    store.save_engine(engine)
}

#[tauri::command]
pub fn save_model_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    model: ModelConfig,
) -> Result<AppConfig, String> {
    store.save_model(model)
}

#[tauri::command]
pub fn save_binding_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    binding: BindingConfig,
) -> Result<AppConfig, String> {
    store.save_binding(binding)
}

#[tauri::command]
pub fn get_equivalent_env(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    binding: BindingConfig,
) -> Result<Vec<(String, String)>, String> {
    store.equivalent_env(&binding)
}

#[tauri::command]
pub fn read_engine_config_file(engine_id: String) -> Result<EngineConfigFile, String> {
    read_engine_config_file_from_disk(&engine_id)
}

#[tauri::command]
pub fn write_engine_config_file(
    engine_id: String,
    content: String,
) -> Result<EngineConfigFile, String> {
    write_engine_config_file_to_disk(&engine_id, &content)
}

#[tauri::command]
pub fn set_provider_defaults(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    engine_id: String,
    model_id: String,
) -> Result<AppConfig, String> {
    store.set_defaults(&engine_id, &model_id)
}

#[tauri::command]
pub async fn test_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<ConnectionResult, String> {
    let result = test_provider_connection(&store, &provider_id).await?;
    let test = ProviderTest {
        result: if !result.verified {
            TestOutcome::Unverified
        } else if result.ok {
            TestOutcome::Ok
        } else {
            TestOutcome::Fail
        },
        latency_ms: Some(result.latency_ms),
        at: unix_timestamp_seconds()?,
    };
    store.record_test_result(&provider_id, test)?;
    Ok(result)
}

fn unix_timestamp_seconds() -> Result<i64, String> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("系统时间异常：{e}"))?;
    i64::try_from(duration.as_secs()).map_err(|_| "系统时间超出范围".to_string())
}

/// message.ts 用毫秒（变更-07：与 checkpoint.ts 同单位，回溯截断依赖比较）
fn unix_timestamp_millis() -> Result<i64, String> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("系统时间异常：{e}"))?;
    i64::try_from(duration.as_millis()).map_err(|_| "系统时间超出范围".to_string())
}

#[tauri::command]
pub async fn sync_provider_models_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<AppConfig, String> {
    sync_provider_models(&store, &provider_id).await
}

#[tauri::command]
pub async fn test_engine_config(bin: String) -> Result<ConnectionResult, String> {
    Ok(test_engine_connection(&bin).await)
}

/// 发送一条用户消息（拉起真实 claude 轮次并流式回传 stdout）。
#[tauri::command]
pub fn send_message(
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
    text: String,
    display_text: Option<String>,
    attachments: Option<Vec<String>>,
    mode: Option<String>,
) -> Result<(), String> {
    let attachments = attachments.unwrap_or_default();
    ensure_budget_allows_turn(&history_store.get_budget()?)?;
    // 会话模式（变更-04）：轮次级属性，未知/缺省一律回落构建（兼容旧前端调用）
    let mode = TurnMode::parse(mode.as_deref());
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    // 历史存用户原文（变更-08）：斜杠命令展开结果只进 CLI，气泡与历史显示 /cmd args 原文
    let record_text = display_text.unwrap_or_else(|| text.clone());
    let prepared = history_store.prepare_user_turn(
        &history_session_id,
        &record_text,
        unix_timestamp_millis()?,
    )?;
    let send_result = (|| {
        let sessions = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?;
        sessions
            .get(&handle_id)
            .ok_or_else(|| format!("找不到会话：{handle_id}"))?
            .send(text, attachments, mode)
    })();
    if let Err(send_error) = send_result {
        return match history_store.rollback_prepared_user_turn(prepared) {
            Ok(()) => Err(send_error),
            Err(rollback_error) => Err(format!(
                "{send_error}；同时回滚未启动轮次失败：{rollback_error}"
            )),
        };
    }
    Ok(())
}

/// 中断当前轮次（杀掉对应进程树，并合成 turn_complete{interrupted}）。
#[tauri::command]
pub fn interrupt(store: State<'_, SessionStore>, handle_id: String) -> Result<(), String> {
    let sessions = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?;
    sessions
        .get(&handle_id)
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?
        .interrupt()
}

/// 关闭并回收一个会话句柄：终止残留进程、从 SessionStore 移除，防止 runtime 泄漏。
/// 幂等：句柄不存在时静默成功（前端可能重复调用）。
#[tauri::command]
pub fn close_session(store: State<'_, SessionStore>, handle_id: String) -> Result<(), String> {
    let removed = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .remove(&handle_id);
    if let Some(session) = removed {
        let _ = session.interrupt();
    }
    store
        .history_session_ids
        .lock()
        .map_err(|_| "会话历史映射锁中毒".to_string())?
        .remove(&handle_id);
    Ok(())
}

/// 回应一个审批请求，并用 Claude Code 的 `--resume` 继续被 defer 的工具调用。
#[tauri::command]
pub fn approval_response(
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
    approval_id: String,
    decision: String,
) -> Result<(), String> {
    let decision = match decision.as_str() {
        "allow" => ApprovalDecision::Allow,
        "deny" => ApprovalDecision::Deny,
        "always" => ApprovalDecision::Always,
        other => return Err(format!("未知审批决定：{other}")),
    };
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    // 审计记录必须先于真实恢复执行：防止工具已经运行、数据库仍显示 pending。
    history_store.resolve_approval(&history_session_id, &approval_id)?;
    let approve_result = (|| {
        let sessions = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?;
        sessions
            .get(&handle_id)
            .ok_or_else(|| format!("找不到会话：{handle_id}"))?
            .approve(approval_id.clone(), decision)
    })();
    if let Err(approve_error) = approve_result {
        return match history_store.reopen_approval(&history_session_id, &approval_id) {
            Ok(()) => Err(approve_error),
            Err(rollback_error) => Err(format!(
                "{approve_error}；同时恢复审批待处理状态失败：{rollback_error}"
            )),
        };
    }
    Ok(())
}

/// 删除会话（变更-12）：终止其存活运行时 → 级联删库 → 清理检查点快照文件。
#[tauri::command]
pub fn delete_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
) -> Result<(), String> {
    // 先回收该会话的所有存活句柄（后台运行中的轮次一并终止——删除是显式破坏性操作）
    let handles: Vec<String> = store
        .history_session_ids
        .lock()
        .map_err(|_| "会话历史映射锁中毒".to_string())?
        .iter()
        .filter(|(_, history_id)| history_id.as_str() == session_id)
        .map(|(handle, _)| handle.clone())
        .collect();
    for handle in handles {
        let removed = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?
            .remove(&handle);
        if let Some(session) = removed {
            let _ = session.interrupt();
        }
        store
            .history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?
            .remove(&handle);
    }
    let snapshot_refs = history_store.delete_session(&session_id)?;
    // 快照文件清理：失败不阻断删除（孤儿文件不影响正确性）
    if let Ok(app_data_dir) = app.path().app_data_dir() {
        let snapshot_store = crate::snapshots::SnapshotStore::new(app_data_dir.join("snapshots"));
        for reference in snapshot_refs {
            let _ = snapshot_store.delete(&reference);
        }
    }
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// 重命名会话（变更-12）
#[tauri::command]
pub fn rename_session(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    history_store.rename_session(&session_id, &title)?;
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// 置顶/取消置顶（变更-12）
#[tauri::command]
pub fn set_session_pinned(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    pinned: bool,
) -> Result<(), String> {
    history_store.set_session_pinned(&session_id, pinned)?;
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// @文件引用（变更-12）：在工作目录下按名称片段搜索文件，供输入框 @ 菜单联想。
/// 深度/数量双限制 + 跳过依赖与版本库目录，防止大仓库遍历卡顿。
#[tauri::command]
pub fn search_workspace_files(cwd: String, query: String) -> Result<Vec<String>, String> {
    const SKIP_DIRS: [&str; 8] = [
        "node_modules",
        ".git",
        "target",
        "dist",
        "build",
        ".venv",
        "__pycache__",
        ".next",
    ];
    const MAX_DEPTH: usize = 5;
    const MAX_RESULTS: usize = 30;
    const MAX_SCANNED: usize = 20_000;

    let root = std::path::PathBuf::from(&cwd);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let needle = query.trim().to_lowercase();
    let mut results: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root.clone(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if results.len() >= MAX_RESULTS || scanned >= MAX_SCANNED {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            scanned += 1;
            if results.len() >= MAX_RESULTS || scanned >= MAX_SCANNED {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if depth + 1 <= MAX_DEPTH
                    && !name.starts_with('.')
                    && !SKIP_DIRS.contains(&name.as_str())
                {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if needle.is_empty() || name.to_lowercase().contains(&needle) {
                if let Ok(relative) = path.strip_prefix(&root) {
                    results.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    results.sort_by_key(|item| item.len());
    Ok(results)
}

/// 粘贴图片附件（变更-12）：剪贴板图片落成临时文件，路径进 attachments。
#[tauri::command]
pub fn save_pasted_image(
    app: AppHandle,
    bytes: Vec<u8>,
    extension: String,
) -> Result<String, String> {
    let ext = match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => extension.as_str(),
        _ => "png",
    };
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败：{e}"))?
        .join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建附件目录失败：{e}"))?;
    let path = dir.join(format!("paste-{}.{ext}", unix_timestamp_millis()?));
    std::fs::write(&path, bytes).map_err(|e| format!("写入图片失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮启动 CLI 时真实生效。
#[tauri::command]
pub fn set_session_mcp_disabled(
    store: State<'_, SessionStore>,
    handle_id: String,
    disabled: Vec<String>,
) -> Result<(), String> {
    let sessions = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?;
    sessions
        .get(&handle_id)
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?
        .set_disabled_mcp(disabled)
}

/// 跨会话「始终允许」清单（P2-4）：设置页展示用
#[tauri::command]
pub fn get_always_allow_tools(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Vec<String>, String> {
    history_store.get_always_allow_tools()
}

/// 撤销某个工具的「始终允许」；只影响之后启动的会话，运行中的会话沿用其 hook state
#[tauri::command]
pub fn remove_always_allow_tool(
    history_store: State<'_, SessionHistoryStore>,
    tool: String,
) -> Result<Vec<String>, String> {
    history_store.remove_always_allow_tool(&tool)
}

/// 回溯到某个检查点：还原文件 + 标记回滚 + 重建 Agent 上下文（P2-5 方案 A）
#[tauri::command]
pub async fn restore_checkpoint(
    app: AppHandle,
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    checkpoint_id: String,
) -> Result<(), String> {
    use crate::snapshots::SnapshotStore;
    use std::path::PathBuf;

    let checkpoint = history_store
        .get_checkpoint(&checkpoint_id)?
        .ok_or_else(|| format!("找不到检查点：{checkpoint_id}"))?;

    let snapshots_dir: PathBuf = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败：{e}"))?
        .join("snapshots");

    let snapshot_store = SnapshotStore::new(snapshots_dir);
    let snapshot = snapshot_store.load(&checkpoint.snapshot_ref)?;
    snapshot_store.restore_files(&snapshot)?;

    history_store.revert_messages_after(&checkpoint.session_id, checkpoint.ts)?;
    // 回溯语义对齐（P2-5）：检查点之后的消息不再进入 Agent 上下文，
    // 并作废旧 CLI 会话 id——之后不 `--resume`，改用截断历史重新开场。
    history_store.clear_cli_session(&checkpoint.session_id)?;
    reset_live_session_context(&store, &history_store, &checkpoint.session_id)?;

    Ok(())
}

/// 撤销回溯：恢复标记；Agent 上下文继续用重建模式（旧 CLI 会话已作废，全量历史重新开场）。
/// 按内部句柄定位会话——回溯已把 cli_session_id 置空，不能再用它解析。
#[tauri::command]
pub async fn undo_revert(
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
) -> Result<(), String> {
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    history_store.unrevert_messages(&history_session_id)?;
    reset_live_session_context(&store, &history_store, &history_session_id)
}

/// 把某个历史会话对应的所有运行中句柄的上下文重置为「未回滚消息」的截断历史
fn reset_live_session_context(
    store: &SessionStore,
    history_store: &SessionHistoryStore,
    history_session_id: &str,
) -> Result<(), String> {
    let detail = history_store.get_session(history_session_id)?;
    let truncated: Vec<crate::sessions::SessionMessage> = detail
        .messages
        .into_iter()
        .filter(|message| !message.reverted)
        .collect();
    let handles: Vec<String> = store
        .history_session_ids
        .lock()
        .map_err(|_| "会话历史映射锁中毒".to_string())?
        .iter()
        .filter(|(_, history_id)| {
            history_id.as_str() == history_session_id
                || history_id.as_str() == detail.summary.id.as_str()
        })
        .map(|(handle, _)| handle.clone())
        .collect();
    let sessions = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?;
    for handle in handles {
        if let Some(session) = sessions.get(&handle) {
            // 会话可能刚结束，重置失败不阻断回溯本身
            let _ = session.reset_context(truncated.clone());
        }
    }
    Ok(())
}

fn engine_id_from_str(engine: &str) -> Result<EngineId, String> {
    match engine {
        "claude-code" => Ok(EngineId::ClaudeCode),
        "codex" => Ok(EngineId::Codex),
        other => Err(format!("暂不支持的引擎：{other}")),
    }
}

fn engine_id_to_string(engine: EngineId) -> String {
    match engine {
        EngineId::ClaudeCode => "claude-code".to_string(),
        EngineId::Codex => "codex".to_string(),
    }
}

fn sync_history_model_prices(history_store: &SessionHistoryStore, config: &AppConfig) {
    for model in &config.models {
        history_store.set_model_price(
            &model.id,
            model.input_price_per_mtok,
            model.output_price_per_mtok,
        );
    }
}

// 用量统计命令
#[tauri::command]
pub async fn get_usage_stats(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
) -> Result<crate::sessions::UsageStats, String> {
    history_store.get_usage_stats(days)
}

#[tauri::command]
pub async fn get_usage_by_model(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
) -> Result<Vec<crate::sessions::ModelUsage>, String> {
    history_store.get_usage_by_model(days)
}

/// 按服务商聚合用量（P3-6）：真实 provider_id 归属，不再按模型名推断
#[tauri::command]
pub async fn get_usage_by_provider(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
) -> Result<Vec<crate::sessions::ProviderUsage>, String> {
    history_store.get_usage_by_provider(days)
}

#[tauri::command]
pub async fn get_daily_usage(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
) -> Result<Vec<crate::sessions::DailyUsage>, String> {
    history_store.get_daily_usage(days)
}

#[tauri::command]
pub async fn get_top_sessions(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
    limit: usize,
) -> Result<Vec<crate::sessions::TopSession>, String> {
    history_store.get_top_sessions(days, limit)
}

#[tauri::command]
pub async fn get_budget(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<crate::sessions::Budget, String> {
    history_store.get_budget()
}

#[tauri::command]
pub async fn set_budget(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    monthly_limit: f64,
    alert_at_80: bool,
    stop_at_100: bool,
) -> Result<(), String> {
    history_store.set_budget(monthly_limit, alert_at_80, stop_at_100)?;
    // 预算变化立即反映到托盘（P3-2）
    crate::tray::refresh_usage(&app);
    Ok(())
}

#[tauri::command]
pub async fn check_budget(history_store: State<'_, SessionHistoryStore>) -> Result<bool, String> {
    let budget = history_store.get_budget()?;
    Ok(ensure_budget_allows_turn(&budget).is_ok())
}

#[cfg(test)]
mod tests {
    use super::ensure_budget_allows_turn;
    use crate::sessions::Budget;

    #[test]
    fn existing_session_cannot_start_turn_after_hard_budget_limit() {
        let budget = Budget {
            monthly_limit: 10.0,
            alert_at_80: true,
            stop_at_100: true,
            current_month_cost: 10.0,
            percentage: 100.0,
        };

        let error = ensure_budget_allows_turn(&budget).expect_err("达到硬上限时必须拒绝新轮次");

        assert!(error.contains("$10.00 / $10.00"));
        assert!(error.contains("用量与成本"));
    }

    #[test]
    fn zero_monthly_limit_does_not_block_turns() {
        let budget = Budget {
            monthly_limit: 0.0,
            alert_at_80: true,
            stop_at_100: true,
            current_month_cost: 42.0,
            percentage: 0.0,
        };

        ensure_budget_allows_turn(&budget).expect("0 表示未设置预算上限");
    }
}

// ============================================================
// 扩展管理命令
// ============================================================

#[tauri::command]
pub async fn list_skills(
    engine: Option<String>,
    project_dir: Option<String>,
) -> Result<Vec<crate::extensions::Skill>, String> {
    crate::extensions::list_skills(engine, project_dir)
}

#[tauri::command]
pub async fn toggle_skill(
    skill_id: String,
    enabled: bool,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::toggle_skill(&skill_id, enabled, project_dir)
}

#[tauri::command]
pub async fn list_mcp_servers() -> Result<Vec<crate::extensions::McpServer>, String> {
    crate::extensions::list_mcp_servers()
}

#[tauri::command]
pub async fn test_mcp_connection(
    server: crate::extensions::McpServer,
) -> Result<Vec<crate::extensions::McpTool>, String> {
    let result = crate::extensions::test_mcp_connection(&server).await;
    // 最近一次连接状态持久化（变更-05）：成功失败都记录，跨重启可见
    crate::extensions::record_mcp_status(&server.name, &result);
    result
}

#[tauri::command]
pub async fn save_mcp_server(server: crate::extensions::McpServer) -> Result<(), String> {
    crate::extensions::save_mcp_server(server)
}

#[tauri::command]
pub async fn delete_mcp_server(name: String) -> Result<(), String> {
    crate::extensions::delete_mcp_server(&name)?;
    crate::extensions::forget_mcp_status(&name);
    Ok(())
}

#[tauri::command]
pub async fn list_subagents(
    project_dir: Option<String>,
) -> Result<Vec<crate::extensions::Subagent>, String> {
    crate::extensions::list_subagents(project_dir)
}

#[tauri::command]
pub async fn save_subagent(
    subagent: crate::extensions::Subagent,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::save_subagent(subagent, project_dir)
}

#[tauri::command]
pub async fn delete_subagent(id: String, project_dir: Option<String>) -> Result<(), String> {
    crate::extensions::delete_subagent(&id, project_dir)
}

#[tauri::command]
pub async fn list_slash_commands(
    engine: Option<String>,
    cwd: Option<String>,
) -> Result<Vec<crate::extensions::SlashCommand>, String> {
    crate::extensions::list_slash_commands(engine, cwd)
}

#[tauri::command]
pub async fn save_slash_command(
    command: crate::extensions::SlashCommand,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::save_slash_command(command, project_dir)
}

#[tauri::command]
pub async fn delete_slash_command(id: String, project_dir: Option<String>) -> Result<(), String> {
    crate::extensions::delete_slash_command(&id, project_dir)
}

#[tauri::command]
pub async fn list_hooks(
    project_dir: Option<String>,
) -> Result<Vec<crate::extensions::Hook>, String> {
    crate::extensions::list_hooks(project_dir)
}

#[tauri::command]
pub async fn save_hook(
    hook: crate::extensions::Hook,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::save_hook(hook, project_dir)
}

#[tauri::command]
pub async fn delete_hook(id: String, project_dir: Option<String>) -> Result<(), String> {
    crate::extensions::delete_hook(&id, project_dir)
}

#[tauri::command]
pub async fn market_search_skills(
    query: String,
) -> Result<Vec<crate::extensions::MarketSkill>, String> {
    crate::extensions::market_search_skills(&query).await
}

#[tauri::command]
pub async fn market_install_skill(
    source: String,
    skill_id: String,
    scope: crate::extensions::SkillScope,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::market_install_skill(&source, &skill_id, scope, project_dir).await
}
