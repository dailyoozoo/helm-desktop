# Changelog

本项目遵循 Keep a Changelog 的基本结构。`0.x` 阶段可能包含不兼容的配置或数据变更。

## [0.2.0] - 2026-08-06

### Added

- Codex Responses 模型接入原生联网搜索；Responses Lite 模型使用与当前 Codex binary 绑定的兼容目录。

### Fixed

- 搜索成功后会持久化当前会话能力；运行配置重启校验不再把 Codex 自己写入的 UI 状态误报为篡改。

## [0.1.1] - 2026-08-04

### Added

- 工作台加入真实轮次阶段和运行活动展示。
- 加入 Helm 内置命令动作，以及 Claude Code 和 Codex Skills 的原生触发方式。
- 长会话时间线支持按窗口加载历史内容。

### Changed

- Codex 后续轮次复用 Session-owned app-server 的原生 thread，并通过 `turn/start` 延续上下文。
- Codex API 使用持久且不保存密钥的运行配置，发送消息前完成 Windows 沙箱就绪检查；界面会区分准备运行环境、配置沙箱和长时间无活动。
- 流式回复完成后再启用完整 Markdown 渲染，降低长回复期间的渲染开销。
- 权限策略统一在设置页管理。
- 普通 Codex 会话按真实 app-server 协议握手判断兼容性，不再要求用户维护精确 CLI 版本；保护模式继续严格验证。

### Fixed

- 修复新会话多轮上下文、预算硬停、发送前持久化和 OAuth 状态误判问题。
- 改进进程清理、钥匙串清理、审批恢复和工作区隔离的失败处理。
- Codex 未完成命令/文件 item 现在以失败终态收口，不再误报成功；Windows 带空格 executable 的审批 matcher 保留完整路径。
- 修复 Codex 停止后后台轮次仍占用工作目录，以及等待审批时停止被误报为工具未完成的问题。
- 改进键盘可达性、响应式布局、错误状态和设置保存行为。

## [0.1.0] - 2026-07-09

### Added

- Tauri 2 + React 桌面应用基础结构。
- Claude Code 与 Codex 真实 CLI 会话、流式事件和历史持久化。
- Provider、模型、绑定、扩展、用量、设置和更新基础链路。

[0.2.0]: ./
[0.1.1]: ./
[0.1.0]: ./
