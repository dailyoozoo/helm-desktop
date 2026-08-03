use helm_lib::extensions::{
    list_plugin_skills_from_dir,
    delete_hook_from_settings_path, delete_mcp_server_from_codex_config_path,
    delete_mcp_server_from_settings_path, delete_slash_command_from_dir, delete_subagent_from_dir,
    list_hooks_from_settings_path, list_mcp_servers_from_codex_config_path,
    list_mcp_servers_from_settings_path, list_skills_from_dir, list_slash_commands_from_dir,
    list_slash_commands_from_sources, list_subagents_from_dir, save_hook_to_settings_path,
    save_mcp_server_to_codex_config_path, save_mcp_server_to_settings_path,
    save_slash_command_to_dir, save_subagent_to_dir, toggle_skill_in_dir, CommandSource, Hook,
    HookEvent, McpServer, McpStatus, McpTransport, SkillScope, SlashCommand, Subagent,
};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

fn temp_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("helm-extensions-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn temp_settings_path(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    dir.join("settings.json")
}

#[test]
fn mcp_server_save_and_delete_round_trips_claude_settings_json() {
    let settings_path = temp_settings_path("mcp");
    fs::write(
        &settings_path,
        r#"{"permissions":{"allow":["Bash(npm test)"]}}"#,
    )
    .unwrap();
    let server = McpServer {
        name: "filesystem".to_string(),
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-filesystem".to_string(),
            "D:\\work".to_string(),
        ],
        env: HashMap::from([("SAFE_ENV".to_string(), "1".to_string())]),
        transport: McpTransport::Stdio,
        enabled: true,
        status: McpStatus::Disconnected,
        last_tested_at: None,
        tool_count: None,
        last_error: None,
    };

    save_mcp_server_to_settings_path(&settings_path, server).unwrap();

    let servers = list_mcp_servers_from_settings_path(&settings_path).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "filesystem");
    assert_eq!(servers[0].command, "npx");
    assert_eq!(servers[0].args[2], "D:\\work");
    assert_eq!(servers[0].env.get("SAFE_ENV").unwrap(), "1");
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        raw["permissions"]["allow"][0],
        serde_json::Value::String("Bash(npm test)".to_string())
    );
    assert_eq!(raw["mcpServers"]["filesystem"]["command"], "npx");

    delete_mcp_server_from_settings_path(&settings_path, "filesystem").unwrap();

    assert!(list_mcp_servers_from_settings_path(&settings_path)
        .unwrap()
        .is_empty());
}

#[test]
fn mcp_server_save_and_delete_round_trips_codex_config_toml() {
    let config_path = temp_dir("codex-mcp").join("config.toml");
    fs::write(
        &config_path,
        r#"
model = "gpt-5"

[mcp_servers.existing]
command = "node"
args = ["server.js"]
"#,
    )
    .unwrap();
    let server = McpServer {
        name: "filesystem".to_string(),
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-filesystem".to_string(),
            "D:\\work".to_string(),
        ],
        env: HashMap::from([("SAFE_ENV".to_string(), "1".to_string())]),
        transport: McpTransport::Stdio,
        enabled: true,
        status: McpStatus::Disconnected,
        last_tested_at: None,
        tool_count: None,
        last_error: None,
    };

    save_mcp_server_to_codex_config_path(&config_path, server).unwrap();

    let servers = list_mcp_servers_from_codex_config_path(&config_path).unwrap();
    let filesystem = servers
        .iter()
        .find(|server| server.name == "filesystem")
        .expect("filesystem server should be listed");
    assert_eq!(filesystem.command, "npx");
    assert_eq!(filesystem.args[2], "D:\\work");
    assert_eq!(filesystem.env.get("SAFE_ENV").unwrap(), "1");
    let raw: toml::Value = fs::read_to_string(&config_path).unwrap().parse().unwrap();
    assert_eq!(raw["model"].as_str(), Some("gpt-5"));
    assert_eq!(
        raw["mcp_servers"]["filesystem"]["command"].as_str(),
        Some("npx")
    );
    assert_eq!(
        raw["mcp_servers"]["filesystem"]["env"]["SAFE_ENV"].as_str(),
        Some("1")
    );

    delete_mcp_server_from_codex_config_path(&config_path, "filesystem").unwrap();

    let servers = list_mcp_servers_from_codex_config_path(&config_path).unwrap();
    assert!(servers.iter().all(|server| server.name != "filesystem"));
    assert!(servers.iter().any(|server| server.name == "existing"));
}

#[test]
fn mcp_sse_server_round_trips_claude_and_codex_configs() {
    let claude_settings_path = temp_settings_path("mcp-sse-claude");
    let codex_config_path = temp_dir("mcp-sse-codex").join("config.toml");
    let server = McpServer {
        name: "remote".to_string(),
        command: "http://127.0.0.1:3000/sse".to_string(),
        args: vec![],
        env: HashMap::new(),
        transport: McpTransport::Sse,
        enabled: true,
        status: McpStatus::Disconnected,
        last_tested_at: None,
        tool_count: None,
        last_error: None,
    };

    save_mcp_server_to_settings_path(&claude_settings_path, server.clone()).unwrap();
    save_mcp_server_to_codex_config_path(&codex_config_path, server).unwrap();

    let claude_servers = list_mcp_servers_from_settings_path(&claude_settings_path).unwrap();
    assert_eq!(claude_servers[0].name, "remote");
    assert!(matches!(claude_servers[0].transport, McpTransport::Sse));
    assert_eq!(claude_servers[0].command, "http://127.0.0.1:3000/sse");

    let codex_servers = list_mcp_servers_from_codex_config_path(&codex_config_path).unwrap();
    assert_eq!(codex_servers[0].name, "remote");
    assert!(matches!(codex_servers[0].transport, McpTransport::Sse));
    assert_eq!(codex_servers[0].command, "http://127.0.0.1:3000/sse");

    let claude_raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&claude_settings_path).unwrap()).unwrap();
    assert_eq!(claude_raw["mcpServers"]["remote"]["type"], "sse");
    assert_eq!(
        claude_raw["mcpServers"]["remote"]["url"],
        "http://127.0.0.1:3000/sse"
    );
    let codex_raw: toml::Value = fs::read_to_string(&codex_config_path)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        codex_raw["mcp_servers"]["remote"]["url"].as_str(),
        Some("http://127.0.0.1:3000/sse")
    );
}

#[test]
fn subagent_save_and_delete_round_trips_claude_agent_markdown() {
    let agents_dir = temp_dir("agents");
    let subagent = Subagent {
        id: "security-reviewer".to_string(),
        name: "安全审查".to_string(),
        model: "claude-opus-4.7".to_string(),
        role: "审查注入、鉴权与密钥泄露风险。".to_string(),
        tools: "Read,Grep,Glob".to_string(),
        auto: true,
        prompt: "你是安全审查代理，按严重程度输出风险。".to_string(),
        scope: SkillScope::Global,
    };

    save_subagent_to_dir(&agents_dir, subagent).unwrap();

    let file = agents_dir.join("security-reviewer.md");
    let raw = fs::read_to_string(&file).unwrap();
    assert!(raw.contains("name: security-reviewer"));
    assert!(raw.contains("description: 审查注入、鉴权与密钥泄露风险。"));
    assert!(raw.contains("model: claude-opus-4.7"));
    assert!(raw.contains("tools: Read,Grep,Glob"));
    assert!(raw.contains("你是安全审查代理"));
    let subagents = list_subagents_from_dir(&agents_dir).unwrap();
    assert_eq!(subagents.len(), 1);
    assert_eq!(subagents[0].id, "security-reviewer");
    assert_eq!(subagents[0].name, "安全审查");
    assert!(subagents[0].auto);

    delete_subagent_from_dir(&agents_dir, "security-reviewer").unwrap();

    assert!(list_subagents_from_dir(&agents_dir).unwrap().is_empty());
}

#[test]
fn slash_command_enabled_state_maps_to_cli_visible_markdown_file() {
    let commands_dir = temp_dir("commands");
    let mut command = SlashCommand {
        id: "review".to_string(),
        trigger: "/review".to_string(),
        description: "审查当前改动并按严重程度给出修复建议".to_string(),
        scope: SkillScope::Global,
        enabled: true,
        body: "审查 git diff 中的全部改动。".to_string(),
        engine: "all".to_string(),
        source: CommandSource::Extension,
        argument_hint: Some("[文件或目录]".to_string()),
    };

    save_slash_command_to_dir(&commands_dir, command.clone()).unwrap();

    assert!(commands_dir.join("review.md").exists());
    let raw = fs::read_to_string(commands_dir.join("review.md")).unwrap();
    assert!(raw.contains("argument-hint: [文件或目录]"));
    let commands = list_slash_commands_from_dir(&commands_dir, None).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].trigger, "/review");
    assert_eq!(commands[0].argument_hint.as_deref(), Some("[文件或目录]"));
    assert!(commands[0].enabled);

    command.enabled = false;
    save_slash_command_to_dir(&commands_dir, command).unwrap();

    assert!(!commands_dir.join("review.md").exists());
    assert!(commands_dir
        .join(".helm-disabled")
        .join("review.md")
        .exists());
    let commands = list_slash_commands_from_dir(&commands_dir, None).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].trigger, "/review");
    assert!(!commands[0].enabled);

    delete_slash_command_from_dir(&commands_dir, "review").unwrap();

    assert!(list_slash_commands_from_dir(&commands_dir, None)
        .unwrap()
        .is_empty());
}

#[test]
fn slash_commands_merge_codex_prompts_with_extension_priority() {
    let extension_dir = temp_dir("merge-ext");
    let prompts_dir = temp_dir("merge-codex-prompts");
    // 扩展中心：engine=codex 的 /deploy
    save_slash_command_to_dir(
        &extension_dir,
        SlashCommand {
            id: "deploy".to_string(),
            trigger: "/deploy".to_string(),
            description: "扩展中心版本".to_string(),
            scope: SkillScope::Global,
            enabled: true,
            body: "扩展中心 deploy 模板".to_string(),
            engine: "codex".to_string(),
            source: CommandSource::Extension,
            argument_hint: None,
        },
    )
    .unwrap();
    // Codex 原生 prompts：同名 /deploy（应被扩展中心遮蔽）+ 独有 /native
    fs::write(prompts_dir.join("deploy.md"), "原生 deploy 模板").unwrap();
    fs::write(
        prompts_dir.join("native.md"),
        "---\ndescription: 原生命令\nargument-hint: [目标]\n---\n原生 native 模板",
    )
    .unwrap();
    fs::write(prompts_dir.join("notes.txt"), "非 md 忽略").unwrap();

    let commands =
        list_slash_commands_from_sources(&extension_dir, Some(&prompts_dir), None, Some("codex"))
            .unwrap();

    let deploy: Vec<_> = commands
        .iter()
        .filter(|command| command.trigger == "/deploy")
        .collect();
    assert_eq!(deploy.len(), 1, "同名命令应只保留扩展中心一条");
    assert_eq!(deploy[0].source, CommandSource::Extension);
    assert_eq!(deploy[0].body, "扩展中心 deploy 模板");

    let native = commands
        .iter()
        .find(|command| command.trigger == "/native")
        .expect("Codex 原生 prompt 应出现在列表");
    assert_eq!(native.source, CommandSource::EngineUser);
    assert_eq!(native.id, "__codex_native");
    assert_eq!(native.description, "原生命令");
    assert_eq!(native.argument_hint.as_deref(), Some("[目标]"));
    assert_eq!(native.engine, "codex");

    // 内置命令仍在，且来源为 builtin
    assert!(commands
        .iter()
        .any(|command| command.id == "__proto_review" && command.source == CommandSource::Builtin));
}

#[test]
fn slash_commands_merge_project_claude_commands() {
    let extension_dir = temp_dir("proj-ext");
    let project_root = temp_dir("proj-root");
    let project_commands = project_root.join(".claude").join("commands");
    fs::create_dir_all(&project_commands).unwrap();
    fs::write(
        project_commands.join("bar.md"),
        "---\ndescription: 项目级命令\n---\n项目级 bar 模板",
    )
    .unwrap();

    let commands = list_slash_commands_from_sources(
        &extension_dir,
        None,
        Some(&project_root),
        Some("claude-code"),
    )
    .unwrap();

    let bar = commands
        .iter()
        .find(|command| command.trigger == "/bar")
        .expect("项目级命令应出现在列表");
    assert_eq!(bar.source, CommandSource::EngineProject);
    assert_eq!(bar.id, "__proj_bar");
    assert_eq!(bar.engine, "claude-code");
    matches!(bar.scope, SkillScope::Project);
}

#[test]
fn disabled_extension_command_does_not_mask_engine_native_command() {
    let extension_dir = temp_dir("mask-ext");
    let prompts_dir = temp_dir("mask-codex-prompts");
    save_slash_command_to_dir(
        &extension_dir,
        SlashCommand {
            id: "native".to_string(),
            trigger: "/native".to_string(),
            description: "已停用的扩展中心版本".to_string(),
            scope: SkillScope::Global,
            enabled: false,
            body: "扩展中心 native 模板".to_string(),
            engine: "codex".to_string(),
            source: CommandSource::Extension,
            argument_hint: None,
        },
    )
    .unwrap();
    fs::write(prompts_dir.join("native.md"), "原生 native 模板").unwrap();

    let commands =
        list_slash_commands_from_sources(&extension_dir, Some(&prompts_dir), None, Some("codex"))
            .unwrap();

    let enabled_native: Vec<_> = commands
        .iter()
        .filter(|command| command.trigger == "/native" && command.enabled)
        .collect();
    assert_eq!(enabled_native.len(), 1, "停用的扩展命令不应遮蔽原生命令");
    assert_eq!(enabled_native[0].source, CommandSource::EngineUser);
}

#[test]
fn skill_toggle_moves_directory_between_enabled_and_disabled() {
    let skills_dir = temp_dir("skills-toggle");
    let skill = skills_dir.join("demo-skill");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("SKILL.md"), "# 演示技能\n\n用于测试。\n").unwrap();

    let skills = list_skills_from_dir(&skills_dir).unwrap();
    assert_eq!(skills.len(), 1);
    assert!(skills[0].enabled);

    toggle_skill_in_dir(&skills_dir, "demo-skill", false).unwrap();
    assert!(!skills_dir.join("demo-skill").exists());
    assert!(skills_dir
        .join(".helm-disabled")
        .join("demo-skill")
        .is_dir());
    let skills = list_skills_from_dir(&skills_dir).unwrap();
    assert_eq!(skills.len(), 1);
    assert!(!skills[0].enabled);

    toggle_skill_in_dir(&skills_dir, "demo-skill", true).unwrap();
    assert!(skills_dir.join("demo-skill").is_dir());
    let skills = list_skills_from_dir(&skills_dir).unwrap();
    assert!(skills[0].enabled);
}

#[test]
fn engine_native_and_builtin_commands_are_read_only() {
    let commands_dir = temp_dir("readonly");
    assert!(delete_slash_command_from_dir(&commands_dir, "__codex_demo").is_err());
    assert!(delete_slash_command_from_dir(&commands_dir, "__proto_review").is_err());
    assert!(delete_slash_command_from_dir(&commands_dir, "__proj_bar").is_err());
    let readonly = SlashCommand {
        id: "__codex_demo".to_string(),
        trigger: "/demo".to_string(),
        description: "原生命令".to_string(),
        scope: SkillScope::Global,
        enabled: true,
        body: "模板".to_string(),
        engine: "codex".to_string(),
        source: CommandSource::EngineUser,
        argument_hint: None,
    };
    assert!(save_slash_command_to_dir(&commands_dir, readonly).is_err());
}

#[test]
fn protocol_commands_include_real_helm_ui_actions_for_both_engines() {
    let commands_dir = temp_dir("helm-ui-actions");
    for engine in ["claude-code", "codex"] {
        let commands = list_slash_commands_from_dir(&commands_dir, Some(engine)).unwrap();
        let triggers: Vec<_> = commands
            .iter()
            .map(|command| command.trigger.as_str())
            .collect();
        for expected in [
            "/new",
            "/resume",
            "/permissions",
            "/extensions",
            "/context",
            "/status",
            "/stop",
            "/help",
        ] {
            assert!(triggers.contains(&expected), "{engine} 缺少 {expected}");
        }
    }
}

#[test]
fn hook_enabled_state_maps_to_claude_settings_hooks() {
    let settings_path = temp_settings_path("hooks");
    let mut hook = Hook {
        id: "format-before-write".to_string(),
        event: HookEvent::PreToolUse,
        match_pattern: "Edit|Write".to_string(),
        command: "prettier -w \"$FILE\"".to_string(),
        description: "写入文件前格式化".to_string(),
        enabled: true,
        scope: SkillScope::Global,
    };

    save_hook_to_settings_path(&settings_path, hook.clone()).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(raw["hooks"]["PreToolUse"][0]["matcher"], "Edit|Write");
    assert_eq!(
        raw["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "prettier -w \"$FILE\""
    );
    let hooks = list_hooks_from_settings_path(&settings_path).unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].id, "format-before-write");
    assert!(hooks[0].enabled);

    hook.enabled = false;
    save_hook_to_settings_path(&settings_path, hook).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(raw["hooks"]["PreToolUse"].as_array().unwrap().is_empty());
    assert_eq!(
        raw["helmDisabledHooks"][0]["id"],
        serde_json::Value::String("format-before-write".to_string())
    );
    let hooks = list_hooks_from_settings_path(&settings_path).unwrap();
    assert_eq!(hooks.len(), 1);
    assert!(!hooks[0].enabled);

    delete_hook_from_settings_path(&settings_path, "format-before-write").unwrap();

    assert!(list_hooks_from_settings_path(&settings_path)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn mcp_sse_connection_reads_tools_list_from_real_sse_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (events_tx, _) = broadcast::channel::<String>(8);
    let server_task = tokio::spawn(run_sse_mcp_server(listener, events_tx));

    let tools = helm_lib::extensions::test_mcp_connection(&McpServer {
        name: "remote".to_string(),
        command: format!("http://{addr}/sse"),
        args: vec![],
        env: HashMap::new(),
        transport: McpTransport::Sse,
        enabled: true,
        status: McpStatus::Disconnected,
        last_tested_at: None,
        tool_count: None,
        last_error: None,
    })
    .await
    .unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "search");
    assert_eq!(tools[0].description.as_deref(), Some("Search docs"));

    server_task.abort();
}

async fn run_sse_mcp_server(listener: TcpListener, events_tx: broadcast::Sender<String>) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            break;
        };
        let tx = events_tx.clone();
        tokio::spawn(async move {
            let Ok(request) = read_http_request(&mut stream).await else {
                return;
            };
            if request.method == "GET" && request.path == "/sse" {
                let mut rx = tx.subscribe();
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n",
                    )
                    .await;
                let _ = stream
                    .write_all(b"event: endpoint\ndata: /message\r\n\r\n")
                    .await;
                while let Ok(event) = rx.recv().await {
                    let _ = stream.write_all(event.as_bytes()).await;
                }
            } else if request.method == "POST" && request.path == "/message" {
                let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let id = value.get("id").and_then(|id| id.as_i64()).unwrap_or(0);
                let response = match value.get("method").and_then(|method| method.as_str()) {
                    Some("initialize") => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {},
                            "serverInfo": { "name": "test", "version": "1" }
                        }
                    }),
                    Some("tools/list") => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "tools": [
                                { "name": "search", "description": "Search docs" }
                            ]
                        }
                    }),
                    _ => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                };
                let _ = tx.send(format!("event: message\ndata: {response}\r\n\r\n"));
                let _ = stream
                    .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });
    }
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<HttpRequest> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await?;
        buffer.push(byte[0]);
        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = headers.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    if body.len() < content_length {
        let mut rest = vec![0_u8; content_length - body.len()];
        stream.read_exact(&mut rest).await?;
        body.extend(rest);
    }
    Ok(HttpRequest { method, path, body })
}

// ==================== 变更-04 新增行为 ====================

#[test]
fn mcp_http_server_round_trips_claude_and_codex_configs() {
    let claude_settings_path = temp_settings_path("mcp-http-claude");
    let codex_config_path = temp_dir("mcp-http-codex").join("config.toml");
    let server = McpServer {
        name: "remote-http".to_string(),
        command: "http://127.0.0.1:3000/mcp".to_string(),
        args: vec![],
        env: HashMap::new(),
        transport: McpTransport::Http,
        enabled: true,
        status: McpStatus::Disconnected,
        last_tested_at: None,
        tool_count: None,
        last_error: None,
    };

    save_mcp_server_to_settings_path(&claude_settings_path, server.clone()).unwrap();
    save_mcp_server_to_codex_config_path(&codex_config_path, server).unwrap();

    let claude_servers = list_mcp_servers_from_settings_path(&claude_settings_path).unwrap();
    assert_eq!(claude_servers[0].name, "remote-http");
    assert!(matches!(claude_servers[0].transport, McpTransport::Http));
    assert_eq!(claude_servers[0].command, "http://127.0.0.1:3000/mcp");

    let claude_raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&claude_settings_path).unwrap()).unwrap();
    assert_eq!(claude_raw["mcpServers"]["remote-http"]["type"], "http");
    let codex_raw: toml::Value = fs::read_to_string(&codex_config_path)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        codex_raw["mcp_servers"]["remote-http"]["url"].as_str(),
        Some("http://127.0.0.1:3000/mcp")
    );
}

#[test]
fn mcp_status_record_round_trips_and_survives_reload() {
    let status_path = temp_dir("mcp-status").join("mcp-status.json");
    helm_lib::extensions::record_mcp_status_to_path(
        &status_path,
        "filesystem",
        &Ok(vec![helm_lib::extensions::McpTool {
            name: "read".to_string(),
            description: None,
        }]),
    )
    .unwrap();
    helm_lib::extensions::record_mcp_status_to_path(
        &status_path,
        "broken",
        &Err("连接失败".to_string()),
    )
    .unwrap();

    let status = helm_lib::extensions::read_mcp_status(&status_path).unwrap();
    let ok_entry = status.get("filesystem").unwrap();
    assert!(ok_entry.ok);
    assert_eq!(ok_entry.tool_count, Some(1));
    assert!(ok_entry.tested_at > 0);
    let err_entry = status.get("broken").unwrap();
    assert!(!err_entry.ok);
    assert_eq!(err_entry.error.as_deref(), Some("连接失败"));
}

#[test]
fn concurrent_mcp_writes_preserve_all_successful_claude_and_codex_entries() {
    let claude_path = temp_settings_path("concurrent-mcp-claude");
    let codex_path = temp_dir("concurrent-mcp-codex").join("config.toml");
    let mut workers = Vec::new();
    for index in 0..12 {
        let claude_path = claude_path.clone();
        let codex_path = codex_path.clone();
        workers.push(std::thread::spawn(move || {
            let server = McpServer {
                name: format!("server-{index}"),
                command: format!("mcp-{index}"),
                args: vec!["--stdio".to_string()],
                env: HashMap::new(),
                transport: McpTransport::Stdio,
                enabled: true,
                status: McpStatus::Disconnected,
                last_tested_at: None,
                tool_count: None,
                last_error: None,
            };
            save_mcp_server_to_settings_path(&claude_path, server.clone()).unwrap();
            save_mcp_server_to_codex_config_path(&codex_path, server).unwrap();
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    let claude = list_mcp_servers_from_settings_path(&claude_path).unwrap();
    let codex = list_mcp_servers_from_codex_config_path(&codex_path).unwrap();
    assert_eq!(claude.len(), 12);
    assert_eq!(codex.len(), 12);
    for index in 0..12 {
        let name = format!("server-{index}");
        assert!(claude.iter().any(|server| server.name == name));
        assert!(codex.iter().any(|server| server.name == name));
    }
}

#[test]
#[ignore = "requires installed claude and codex CLIs"]
fn real_clis_parse_isolated_configs_written_by_helm() {
    let root = temp_dir("real-cli-config-smoke");
    let claude_dir = root.join(".claude");
    let codex_home = root.join(".codex");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let claude_path = claude_dir.join(".claude.json");
    let codex_path = codex_home.join("config.toml");
    let server = McpServer {
        name: "helm-real-smoke".to_string(),
        command: "node".to_string(),
        args: vec!["-e".to_string(), "process.exit(0)".to_string()],
        env: HashMap::new(),
        transport: McpTransport::Stdio,
        enabled: true,
        status: McpStatus::Disconnected,
        last_tested_at: None,
        tool_count: None,
        last_error: None,
    };
    save_mcp_server_to_settings_path(&claude_path, server.clone()).unwrap();
    save_mcp_server_to_codex_config_path(&codex_path, server).unwrap();

    let claude_bin = if cfg!(windows) {
        "claude.cmd"
    } else {
        "claude"
    };
    let claude = Command::new(claude_bin)
        .args(["mcp", "list"])
        .env("HOME", &root)
        .env("USERPROFILE", &root)
        .env("CLAUDE_CONFIG_DIR", &claude_dir)
        .current_dir(&root)
        .output()
        .expect("failed to start the real Claude CLI");
    let claude_output = format!(
        "{}{}",
        String::from_utf8_lossy(&claude.stdout),
        String::from_utf8_lossy(&claude.stderr)
    );
    assert!(
        claude.status.success(),
        "Claude CLI rejected the generated settings:\n{claude_output}"
    );
    assert!(
        claude_output.contains("helm-real-smoke"),
        "Claude CLI did not load the generated MCP entry:\n{claude_output}"
    );

    let codex_bin = if cfg!(windows) { "codex.cmd" } else { "codex" };
    let codex = Command::new(codex_bin)
        .args(["mcp", "list", "--json"])
        .env("HOME", &root)
        .env("USERPROFILE", &root)
        .env("CODEX_HOME", &codex_home)
        .current_dir(&root)
        .output()
        .expect("failed to start the real Codex CLI");
    let codex_output = format!(
        "{}{}",
        String::from_utf8_lossy(&codex.stdout),
        String::from_utf8_lossy(&codex.stderr)
    );
    assert!(
        codex.status.success(),
        "Codex CLI rejected the generated config:\n{codex_output}"
    );
    assert!(
        codex_output.contains("helm-real-smoke"),
        "Codex CLI did not load the generated MCP entry:\n{codex_output}"
    );
}

#[test]
fn hook_all_nine_events_round_trip_settings_json() {
    let settings_path = temp_settings_path("hooks-nine");
    let events = [
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::UserPromptSubmit,
        HookEvent::Notification,
        HookEvent::Stop,
        HookEvent::SubagentStop,
        HookEvent::PreCompact,
        HookEvent::SessionStart,
        HookEvent::SessionEnd,
    ];
    for (index, event) in events.iter().enumerate() {
        save_hook_to_settings_path(
            &settings_path,
            Hook {
                id: format!("hook-{index}"),
                event: event.clone(),
                match_pattern: "*".to_string(),
                command: format!("echo {index}"),
                description: format!("钩子 {index}"),
                enabled: true,
                scope: SkillScope::Global,
            },
        )
        .unwrap();
    }

    let hooks = list_hooks_from_settings_path(&settings_path).unwrap();
    assert_eq!(hooks.len(), 9);
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    for key in [
        "PreToolUse",
        "PostToolUse",
        "UserPromptSubmit",
        "Notification",
        "Stop",
        "SubagentStop",
        "PreCompact",
        "SessionStart",
        "SessionEnd",
    ] {
        assert!(
            raw["hooks"][key].as_array().is_some_and(|a| !a.is_empty()),
            "hooks.{key} 应写入 settings.json"
        );
    }
}

#[test]
fn market_download_candidates_cover_mirrors_branches_and_layouts() {
    let urls = helm_lib::extensions::market_download_candidates("obra/superpowers", "code-review");
    assert_eq!(urls.len(), 18, "3 镜像 × 2 分支 × 3 布局");
    assert_eq!(
        urls[0],
        "https://raw.githubusercontent.com/obra/superpowers/main/skills/code-review/SKILL.md"
    );
    assert!(urls
        .iter()
        .any(|url| url.starts_with("https://ghfast.top/")));
    assert!(urls
        .iter()
        .any(|url| url.contains("/master/.claude/skills/code-review/SKILL.md")));
}

#[test]
fn market_search_response_parses_skills_sh_payload() {
    let payload = serde_json::json!({
        "query": "review",
        "skills": [
            { "id": "a/b/review", "skillId": "review", "name": "review", "installs": 91123, "source": "a/b" },
            { "skillId": "no-installs", "source": "c/d" },
            { "name": "缺 skillId 应跳过", "source": "e/f" }
        ]
    });
    let skills = helm_lib::extensions::parse_market_search_response(&payload).unwrap();
    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0].skill_id, "review");
    assert_eq!(skills[0].source, "a/b");
    assert_eq!(skills[0].installs, 91123);
    assert_eq!(skills[1].installs, 0);
}

#[test]
fn market_installed_skill_is_listed_with_market_source() {
    let skills_dir = temp_dir("skills-market");
    let skill_dir = skills_dir.join("code-review");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Code Review\n\n审查技能。\n").unwrap();
    fs::write(
        skill_dir.join(".helm-market.json"),
        helm_lib::extensions::market_marker_json("obra/superpowers", "code-review"),
    )
    .unwrap();

    let skills = list_skills_from_dir(&skills_dir).unwrap();
    assert_eq!(skills.len(), 1);
    assert!(matches!(
        skills[0].source,
        helm_lib::extensions::SkillSource::Market
    ));
}

#[test]
fn project_slash_commands_include_disabled_entries() {
    let extension_dir = temp_dir("proj-disabled-ext");
    let project_root = temp_dir("proj-disabled-root");
    let project_commands = project_root.join(".claude").join("commands");
    fs::create_dir_all(project_commands.join(".helm-disabled")).unwrap();
    fs::write(project_commands.join("on.md"), "启用命令").unwrap();
    fs::write(
        project_commands.join(".helm-disabled").join("off.md"),
        "停用命令",
    )
    .unwrap();

    let commands =
        list_slash_commands_from_sources(&extension_dir, None, Some(&project_root), None).unwrap();

    let on = commands.iter().find(|c| c.id == "__proj_on").unwrap();
    assert!(on.enabled);
    let off = commands.iter().find(|c| c.id == "__proj_off").unwrap();
    assert!(!off.enabled);
}

#[test]
fn changing_command_scope_to_project_moves_file_out_of_global_dir() {
    use helm_lib::extensions::save_slash_command_routed;
    let global_dir = temp_dir("scope-move-global");
    let project_commands = temp_dir("scope-move-proj").join(".claude").join("commands");
    // 旧数据形态：scope=project 的命令文件实际存在全局目录
    save_slash_command_to_dir(
        &global_dir,
        SlashCommand {
            id: "deploy".to_string(),
            trigger: "/deploy".to_string(),
            description: "部署".to_string(),
            scope: SkillScope::Global,
            enabled: true,
            body: "部署模板".to_string(),
            engine: "all".to_string(),
            source: CommandSource::Extension,
            argument_hint: None,
        },
    )
    .unwrap();
    assert!(global_dir.join("deploy.md").exists());

    // 用户把作用域改成项目：应写入项目目录并清理全局副本
    save_slash_command_routed(
        &global_dir,
        Some(&project_commands),
        SlashCommand {
            id: "deploy".to_string(),
            trigger: "/deploy".to_string(),
            description: "部署".to_string(),
            scope: SkillScope::Project,
            enabled: true,
            body: "部署模板".to_string(),
            engine: "all".to_string(),
            source: CommandSource::Extension,
            argument_hint: None,
        },
    )
    .unwrap();

    assert!(project_commands.join("deploy.md").exists());
    assert!(!global_dir.join("deploy.md").exists());
    assert!(!global_dir.join(".helm-disabled").join("deploy.md").exists());

    // 编辑已有项目级命令（__proj_ 前缀）：不应误删全局同名的其他命令
    save_slash_command_to_dir(
        &global_dir,
        SlashCommand {
            id: "deploy".to_string(),
            trigger: "/deploy".to_string(),
            description: "全局新版本".to_string(),
            scope: SkillScope::Global,
            enabled: true,
            body: "全局模板".to_string(),
            engine: "all".to_string(),
            source: CommandSource::Extension,
            argument_hint: None,
        },
    )
    .unwrap();
    save_slash_command_routed(
        &global_dir,
        Some(&project_commands),
        SlashCommand {
            id: "__proj_deploy".to_string(),
            trigger: "/deploy".to_string(),
            description: "项目级编辑".to_string(),
            scope: SkillScope::Project,
            enabled: true,
            body: "项目模板 v2".to_string(),
            engine: "claude-code".to_string(),
            source: CommandSource::EngineProject,
            argument_hint: None,
        },
    )
    .unwrap();
    assert!(
        global_dir.join("deploy.md").exists(),
        "编辑项目级命令不应动全局文件"
    );
    let raw = fs::read_to_string(project_commands.join("deploy.md")).unwrap();
    assert!(raw.contains("项目模板 v2"));
}

#[test]
fn plugin_skills_discovered_from_marketplace_directory() {
    let dir = temp_dir("plugin-skills");
    let marketplaces = dir.join("marketplaces");

    // 插件 caveman：skills/caveman/SKILL.md
    let caveman_skill = marketplaces.join("caveman").join("skills").join("caveman");
    fs::create_dir_all(&caveman_skill).unwrap();
    fs::write(
        caveman_skill.join("SKILL.md"),
        "# Caveman Mode\n\nUltra-compressed communication.",
    )
    .unwrap();

    // 插件 caveman：skills/caveman-commit/SKILL.md
    let commit_skill = marketplaces.join("caveman").join("skills").join("caveman-commit");
    fs::create_dir_all(&commit_skill).unwrap();
    fs::write(
        commit_skill.join("SKILL.md"),
        "# Caveman Commit\n\nCommit like caveman.",
    )
    .unwrap();

    // 带点号的目录应跳过
    let hidden = marketplaces.join(".hidden-plugin").join("skills").join("test");
    fs::create_dir_all(&hidden).unwrap();
    fs::write(hidden.join("SKILL.md"), "# Hidden").unwrap();

    // 没有 SKILL.md 的子目录应跳过
    let no_skill = marketplaces.join("empty-plugin").join("skills").join("noskill");
    fs::create_dir_all(&no_skill).unwrap();

    let skills = list_plugin_skills_from_dir(&marketplaces).unwrap();

    // 应发现 2 个 skill（caveman:caveman 和 caveman:caveman-commit）
    assert_eq!(skills.len(), 2);

    let ids: Vec<&str> = skills.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"plugin:caveman:caveman"), "应包含 plugin:caveman:caveman，实际: {ids:?}");
    assert!(ids.contains(&"plugin:caveman:caveman-commit"), "应包含 plugin:caveman:caveman-commit，实际: {ids:?}");

    // 验证 trigger 格式
    let caveman = skills.iter().find(|s| s.id == "plugin:caveman:caveman").unwrap();
    assert_eq!(caveman.trigger, "/caveman:caveman");
    assert_eq!(caveman.engine, "claude-code");
    assert_eq!(caveman.source, helm_lib::extensions::SkillSource::Plugin);
    assert!(caveman.enabled);
    assert_eq!(caveman.scope, SkillScope::Global);

    // 验证隐藏插件和空 skill 目录被跳过
    assert!(!ids.iter().any(|id| id.contains("hidden")));
    assert!(!ids.iter().any(|id| id.contains("noskill")));
}

#[test]
fn plugin_skills_with_nested_plugins_directory() {
    let dir = temp_dir("plugin-nested");
    let marketplaces = dir.join("marketplaces");

    // claude-plugins-official 风格：plugins/<name>/skills/<skill>/SKILL.md
    let nested_skill = marketplaces
        .join("official")
        .join("plugins")
        .join("frontend-design")
        .join("skills")
        .join("frontend-design");
    fs::create_dir_all(&nested_skill).unwrap();
    fs::write(
        nested_skill.join("SKILL.md"),
        "# Frontend Design\n\nDesign skills.",
    )
    .unwrap();

    let skills = list_plugin_skills_from_dir(&marketplaces).unwrap();

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].id, "plugin:official:frontend-design");
    assert_eq!(skills[0].trigger, "/official:frontend-design");
}

#[test]
fn plugin_skills_empty_directory_returns_empty() {
    let dir = temp_dir("plugin-empty");
    let marketplaces = dir.join("marketplaces");
    fs::create_dir_all(&marketplaces).unwrap();

    let skills = list_plugin_skills_from_dir(&marketplaces).unwrap();
    assert!(skills.is_empty());
}

#[test]
fn plugin_skills_nonexistent_directory_returns_empty() {
    let dir = temp_dir("plugin-nonexistent");
    let skills = list_plugin_skills_from_dir(&dir.join("nonexistent")).unwrap();
    assert!(skills.is_empty());
}