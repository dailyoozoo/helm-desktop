use std::path::Path;

fn read_json_object_from_stdin() -> Result<serde_json::Value, String> {
    use std::io::Read;

    let mut input = Vec::new();
    let mut depth = 0_i32;
    let mut started = false;
    let mut in_string = false;
    let mut escaped = false;
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        let mut byte = [0_u8; 1];
        if reader.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            break;
        }
        input.push(byte[0]);
        if in_string {
            if escaped {
                escaped = false;
            } else if byte[0] == b'\\' {
                escaped = true;
            } else if byte[0] == b'"' {
                in_string = false;
            }
        } else if byte[0] == b'"' {
            in_string = true;
        } else if matches!(byte[0], b'{' | b'[') {
            depth += 1;
            started = true;
        } else if matches!(byte[0], b'}' | b']') {
            depth -= 1;
            if started && depth == 0 {
                break;
            }
        }
    }
    serde_json::from_slice(&input).map_err(|e| format!("invalid hook JSON: {e}"))
}

/// Native hook entrypoint used on Windows where PowerShell keeps a piped stdin
/// reader alive until EOF. Claude sends one JSON object without closing stdin.
pub fn run_native_runtime_hook(state_path: &Path) -> i32 {
    let output = || -> Result<serde_json::Value, String> {
        let payload = read_json_object_from_stdin()?;
        let endpoint = std::env::var("HELM_PERMISSION_ENDPOINT")
            .map_err(|_| "permission endpoint missing".to_string())?;
        let token = std::env::var("HELM_PERMISSION_TOKEN")
            .map_err(|_| "permission token missing".to_string())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime.block_on(runtime_hook_response(
            state_path,
            payload,
            &endpoint,
            &token,
            &std::env::var("HELM_HISTORY_SESSION_ID").unwrap_or_default(),
            &std::env::var("HELM_TURN_ID").unwrap_or_default(),
            &std::env::var("HELM_SESSION_CWD").unwrap_or_default(),
        ))
    }();
    let response = output.unwrap_or_else(|_| {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "helm-runtime-bridge-failed",
            }
        })
    });
    println!("{}", response);
    0
}

async fn runtime_hook_response(
    state_path: &Path,
    payload: serde_json::Value,
    endpoint: &str,
    token: &str,
    history_session_id: &str,
    turn_id: &str,
    cwd: &str,
) -> Result<serde_json::Value, String> {
    let tool_id = payload
        .get("tool_use_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let state = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let (decision, reason) = if let Some(value) = state
        .get("decisions")
        .and_then(|values| values.get(tool_id))
        .and_then(serde_json::Value::as_str)
    {
        (value.to_string(), "helm-user-decision".to_string())
    } else {
        let body = serde_json::json!({
            "historySessionId": history_session_id,
            "turnId": turn_id,
            "toolCallId": tool_id,
            "principal": "main-agent",
            "toolName": payload.get("tool_name").and_then(serde_json::Value::as_str).unwrap_or_default(),
            "input": payload.get("tool_input").cloned().unwrap_or(serde_json::Value::Null),
            "cwd": cwd,
        });
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| e.to_string())?
            .post(format!("{endpoint}/v1/decide"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())?;
        let decision = match response.get("effect").and_then(serde_json::Value::as_str) {
            Some("allow") => "allow",
            Some("ask") => "defer",
            _ => "deny",
        };
        let reason = response
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("helm-runtime-bridge")
            .to_string();
        (decision.to_string(), reason)
    };
    Ok(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    }))
}

pub fn write_runtime_hook_script(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(WINDOWS_RUNTIME_HOOK.as_bytes());
        std::fs::write(path, bytes).map_err(|e| format!("写入 Claude Runtime Hook 失败：{e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::write(path, PYTHON_RUNTIME_HOOK)
            .map_err(|e| format!("写入 Claude Runtime Hook 失败：{e}"))
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_RUNTIME_HOOK: &str = r#"
param([Parameter(Mandatory=$true)][string]$StatePath)
$raw = [Console]::In.ReadToEnd()
$decision = "deny"
$reason = "helm-runtime-bridge-failed"
try {
  $payload = $raw | ConvertFrom-Json
  $toolName = [string]$payload.tool_name
  $toolId = [string]$payload.tool_use_id
  $state = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
  if ($state.decisions -and $state.decisions.PSObject.Properties[$toolId]) {
    $decision = [string]$state.decisions.PSObject.Properties[$toolId].Value
    $reason = "helm-user-decision"
  } else {
    $body = @{
      historySessionId = [string]$env:HELM_HISTORY_SESSION_ID
      turnId = [string]$env:HELM_TURN_ID
      toolCallId = $toolId
      principal = "main-agent"
      toolName = $toolName
      input = $payload.tool_input
      cwd = [string]$env:HELM_SESSION_CWD
    } | ConvertTo-Json -Depth 30 -Compress
    $headers = @{ Authorization = "Bearer $env:HELM_PERMISSION_TOKEN" }
    $result = Invoke-RestMethod -Method Post -Uri "$env:HELM_PERMISSION_ENDPOINT/v1/decide" -Headers $headers -ContentType "application/json" -Body $body -TimeoutSec 5
    if ([string]$result.effect -eq "allow") { $decision = "allow" }
    elseif ([string]$result.effect -eq "ask") { $decision = "defer" }
    else { $decision = "deny" }
    $reason = [string]$result.reason
  }
} catch {
  $decision = "deny"
}
@{ hookSpecificOutput = @{ hookEventName = "PreToolUse"; permissionDecision = $decision; permissionDecisionReason = $reason } } | ConvertTo-Json -Depth 10 -Compress
exit 0
"#;

#[cfg(not(target_os = "windows"))]
const PYTHON_RUNTIME_HOOK: &str = r#"#!/usr/bin/env python3
import json, os, sys, urllib.request
state_path = sys.argv[1]
decision, reason = "deny", "helm-runtime-bridge-failed"
def read_json_object():
    chars, depth, started, in_string, escaped = [], 0, False, False, False
    while True:
        char = sys.stdin.read(1)
        if not char:
            break
        chars.append(char)
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char in "{[":
            depth, started = depth + 1, True
        elif char in "}]":
            depth -= 1
            if started and depth == 0:
                break
    return "".join(chars)
try:
    payload = json.loads(read_json_object())
    tool_id = str(payload.get("tool_use_id") or "")
    with open(state_path, "r", encoding="utf-8") as stream:
        state = json.load(stream)
    if tool_id in (state.get("decisions") or {}):
        decision, reason = str(state["decisions"][tool_id]), "helm-user-decision"
    else:
        body = json.dumps({
            "historySessionId": os.environ.get("HELM_HISTORY_SESSION_ID", ""),
            "turnId": os.environ.get("HELM_TURN_ID", ""),
            "toolCallId": tool_id,
            "principal": "main-agent",
            "toolName": str(payload.get("tool_name") or ""),
            "input": payload.get("tool_input"),
            "cwd": os.environ.get("HELM_SESSION_CWD", ""),
        }).encode()
        request = urllib.request.Request(
            os.environ["HELM_PERMISSION_ENDPOINT"] + "/v1/decide", body,
            {"Authorization": "Bearer " + os.environ["HELM_PERMISSION_TOKEN"], "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=5) as response:
            result = json.load(response)
        decision = {"allow":"allow", "ask":"defer"}.get(result.get("effect"), "deny")
        reason = str(result.get("reason") or "helm-runtime-bridge")
except Exception:
    pass
print(json.dumps({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":decision,"permissionDecisionReason":reason}}, separators=(",",":")))
"#;

#[cfg(test)]
mod tests {
    use super::runtime_hook_response;
    use crate::permission_service::{PermissionService, PermissionSessionContext};
    use crate::sessions::SessionHistoryStore;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_runtime_hook_allows_safe_read_and_defers_commands_without_a_version_gate() {
        let root = std::env::temp_dir().join(format!("helm-runtime-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let state_path = root.join("state.json");
        std::fs::write(&state_path, r#"{"decisions":{}}"#).unwrap();

        let store = SessionHistoryStore::new(root.join("db.sqlite"));
        let service = PermissionService::start(store).await.unwrap();
        let registration = service
            .register(PermissionSessionContext {
                engine: "claude-code".to_string(),
                history_session_id: "history-1".to_string(),
                turn_id: "turn-1".to_string(),
                cwd: "D:/repo".to_string(),
                permission_profile: "standard".to_string(),
            })
            .await;
        let read = runtime_hook_response(
            &state_path,
            serde_json::json!({
                "tool_name":"Read",
                "tool_use_id":"read-1",
                "tool_input":{"file_path":"README.md"}
            }),
            &registration.endpoint,
            &registration.token,
            "history-1",
            "turn-1",
            "D:/repo",
        )
        .await
        .unwrap();
        assert_eq!(read["hookSpecificOutput"]["permissionDecision"], "allow");

        let command = runtime_hook_response(
            &state_path,
            serde_json::json!({
                "tool_name":"Bash",
                "tool_use_id":"bash-1",
                "tool_input":{"command":"cargo test"}
            }),
            &registration.endpoint,
            &registration.token,
            "history-1",
            "turn-1",
            "D:/repo",
        )
        .await
        .unwrap();
        assert_eq!(command["hookSpecificOutput"]["permissionDecision"], "defer");

        service.shutdown().await;
        let failed = runtime_hook_response(
            &state_path,
            serde_json::json!({
                "tool_name":"Write",
                "tool_use_id":"write-1",
                "tool_input":{"file_path":"out.txt","content":"x"}
            }),
            &registration.endpoint,
            &registration.token,
            "history-1",
            "turn-1",
            "D:/repo",
        )
        .await;
        assert!(failed.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "uses the installed real Claude CLI with a loopback fake Anthropic Provider"]
    async fn real_claude_binary_defers_bash_without_a_version_allowlist() -> Result<(), String> {
        let bin = std::env::var("HELM_REAL_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string());
        let root = std::env::temp_dir().join(format!(
            "helm-real-claude-runtime-contract-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let state_path = root.join("approval-state.json");
        std::fs::write(&state_path, r#"{"decisions":{}}"#).map_err(|error| error.to_string())?;
        let helm_executable = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .and_then(std::path::Path::parent)
            .map(|path| path.join("helm.exe"))
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                "build target/debug/helm.exe before running the real probe".to_string()
            })?;
        let settings_path = root.join("claude-settings.json");
        let settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "\"{}\" --helm-runtime-hook \"{}\"",
                            helm_executable.display(),
                            state_path.display()
                        ),
                        "timeout": 10
                    }]
                }]
            }
        });
        std::fs::write(
            &settings_path,
            serde_json::to_vec(&settings).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;

        let cwd = root
            .canonicalize()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .to_string();
        let service =
            PermissionService::start(SessionHistoryStore::new(root.join("db.sqlite"))).await?;
        let registration = service
            .register(PermissionSessionContext {
                engine: "claude-code".to_string(),
                history_session_id: "real-runtime-probe".to_string(),
                turn_id: "turn-1".to_string(),
                cwd: cwd.clone(),
                permission_profile: "standard".to_string(),
            })
            .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| error.to_string())?;
        let provider_url = format!(
            "http://{}",
            listener.local_addr().map_err(|error| error.to_string())?
        );
        let provider_task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            for _ in 0..4 {
                let accepted =
                    tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept())
                        .await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                let mut request = Vec::new();
                let mut header_end = None;
                let mut content_length = 0_usize;
                loop {
                    let mut chunk = [0_u8; 4096];
                    let count = stream
                        .read(&mut chunk)
                        .await
                        .map_err(|error| error.to_string())?;
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    if header_end.is_none() {
                        header_end = request
                            .windows(4)
                            .position(|part| part == b"\r\n\r\n")
                            .map(|at| at + 4);
                        if let Some(end) = header_end {
                            let headers = String::from_utf8_lossy(&request[..end]);
                            content_length = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                        }
                    }
                    if header_end.is_some_and(|end| request.len() >= end + content_length) {
                        break;
                    }
                }
                let first_line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let (content_type, body) = if first_line.contains("/v1/messages/count_tokens") {
                    ("application/json", "{\"input_tokens\":1}".to_string())
                } else if first_line.contains("/v1/messages") {
                    (
                        "text/event-stream",
                        concat!(
                            "event: message_start\n",
                            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_helm_probe\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-helm-probe\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                            "event: content_block_start\n",
                            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_helm_probe\",\"name\":\"Bash\",\"input\":{}}}\n\n",
                            "event: content_block_delta\n",
                            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
                            "event: content_block_stop\n",
                            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                            "event: message_delta\n",
                            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
                            "event: message_stop\n",
                            "data: {\"type\":\"message_stop\"}\n\n"
                        )
                        .to_string(),
                    )
                } else {
                    ("application/json", "{\"error\":\"not_found\"}".to_string())
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok::<(), String>(())
        });
        let mut command = crate::adapter::build_command(&bin);
        command
            .arg("-p")
            .arg("Use Bash exactly once to run `pwd`. Do not answer without using Bash.")
            .args([
                "--output-format",
                "stream-json",
                "--verbose",
                "--no-session-persistence",
                "--permission-mode",
                "manual",
                "--setting-sources",
                "",
                "--no-chrome",
                "--disable-slash-commands",
                "--system-prompt",
                "You are running a permission bridge contract test. Follow the user instruction exactly.",
                "--settings",
            ])
            .arg(&settings_path)
            .args(["--model", "claude-helm-probe", "--tools", "Bash"])
            .current_dir(&root)
            .env("HELM_PERMISSION_ENDPOINT", &registration.endpoint)
            .env("HELM_PERMISSION_TOKEN", &registration.token)
            .env("HELM_HISTORY_SESSION_ID", "real-runtime-probe")
            .env("HELM_TURN_ID", "turn-1")
            .env("HELM_SESSION_CWD", &cwd)
            .env("ANTHROPIC_BASE_URL", &provider_url)
            .env("ANTHROPIC_API_KEY", "helm-loopback-probe-key")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "real Claude runtime probe stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "real Claude runtime probe stderr unavailable".to_string())?;
        let stderr_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            bytes
        });
        let observed = tokio::time::timeout(std::time::Duration::from_secs(150), async {
            use tokio::io::{AsyncBufReadExt as _, BufReader};
            let mut lines = BufReader::new(stdout).lines();
            let mut stream = String::new();
            while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
                stream.push_str(&line);
                stream.push('\n');
                if stream.contains("engine capability manifest is not verified") {
                    return Err(
                        "real Claude runtime probe hit the removed version gate".to_string()
                    );
                }
                if crate::claude_capabilities::defer_contract_probe_succeeded(&stream, "pwd") {
                    return Ok(());
                }
            }
            Err("real Claude runtime probe exited before deferring Bash".to_string())
        })
        .await;
        let reaped = crate::adapter::terminate_child_bounded(
            &mut child,
            pid,
            std::time::Duration::from_secs(2),
        )
        .await;
        stderr_task.abort();
        let _ = stderr_task.await;
        provider_task.abort();
        let _ = provider_task.await;
        service.shutdown().await;
        let result = match observed {
            Ok(result) if reaped => result,
            Ok(Ok(())) => Err("real Claude runtime probe process was not reaped".to_string()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("real Claude runtime probe timed out before defer".to_string()),
        };
        let _ = std::fs::remove_dir_all(root);
        result
    }
}
