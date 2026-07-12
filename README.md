# Helm

Helm 是一个用于统一管理多个 CLI Agent 引擎的桌面客户端，目前支持驱动 Claude Code 和 Codex 本地 CLI 进程。

Helm 不重新实现 Agent，而是负责启动和管理本地 CLI、解析流式事件，并以桌面图形界面呈现会话、工具调用、文件差异、审批和用量等信息。

## 功能

- Claude Code、Codex 多引擎会话
- 流式回复、工具调用和文件差异展示
- 会话保存、搜索、恢复与并行运行
- Provider、模型和引擎绑定管理
- 构建、计划、询问三种轮次模式
- 审批、检查点和回溯
- Skills、MCP、子代理、命令和钩子管理
- Token 用量、成本和预算提醒
- Windows 桌面集成与应用更新

## 技术栈

- Tauri 2
- React 18
- TypeScript
- Rust
- SQLite

## 本地开发

前置要求：

- Node.js 18+
- Rust 1.77+
- Claude Code CLI 或 Codex CLI（至少安装一个）

安装依赖并启动开发模式：

```bash
npm install
npm run tauri dev
```

运行完整检查：

```bash
npm run check
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
```

## 当前状态

Helm 仍处于 `0.x` 开发阶段。核心功能已经接入真实 CLI，但正式分发前仍需要完成安装包签名、正式更新源和更多平台真机验证。

## 安全与隐私

- API Key 保存在操作系统钥匙串中，不写入 SQLite 或普通配置文件。
- 会话、消息、用量和应用设置默认保存在本机。
- 安全问题请遵循 [SECURITY.md](SECURITY.md) 中的私密报告方式。
- 数据流说明见 [PRIVACY.md](PRIVACY.md)。

## 许可证

本仓库目前尚未添加开源许可证。源码公开可见不代表已经授权复制、修改、分发或商业使用。
