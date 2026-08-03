# 支持与诊断

Helm 当前为开发阶段软件。遇到问题时，先运行：

```powershell
npm run verify
```

提交问题时请提供：

- Windows/macOS 版本；
- Helm、Claude Code、Codex CLI 版本；
- 受影响模块和可重复步骤；
- 已脱敏的错误信息；
- 是否可在新会话、干净目录或默认设置下复现。

请勿提供 API Key、OAuth token、完整提示词、私有代码、用户文件内容或未经脱敏的数据库。

## 订阅登录与现有 CLI 配置

Helm 的 Claude/Codex 订阅登录使用应用配置目录下的独立 Profile：

```text
<app_config_dir>/cli-profiles/claude-subscription
<app_config_dir>/cli-profiles/codex-subscription
```

升级后如果 Helm 显示订阅未登录，请在 Helm 内重新完成官方登录。不要复制用户全局
`~/.claude`、`~/.codex/auth.json` 或 token 到上述目录；Helm 也不会自动迁移这些认证。
Helm 内退出订阅只影响独立 Profile，不应改变其他终端工具的登录态。

权限相关问题优先记录 Helm 显示的稳定错误码，例如：

- `[codex_subscription_auth_mismatch]`：当前登录态不是 ChatGPT 订阅登录；
- `[codex_provider_unreachable]`：当前服务商或必要网络端点不可达；
- `[codex_provider_rejected]`：服务商拒绝当前 Codex CLI 请求；确认其支持 Responses 接口和当前客户端版本；
- `[runtime_web_search_unavailable]`：当前 Engine/Provider 没有可用的原生搜索能力；这不是网络查询成功，且不应通过反复重试已关闭工具来恢复。
- `[operation_tools_not_disableable]`：当前 Engine 没有可验证的原生无工具启动合同；后台模型输入未发送。
- `[operation_tool_not_allowed]`：无工具后台任务仍返回了工具或审批事件，任务已失败且不会显示审批卡。
- `[operation_frozen_launch_unavailable]`：当前配置无法复现任务冻结的启动规格；如需使用新 Binding，应创建新任务而不是重试旧任务。

这些错误已经脱敏。仍不要附带 `codex doctor --json` 的完整输出，因为其中可能包含本机路径与环境结构。

若是“永久允许后仍反复询问”，请同时记录 ApprovalCard 显示的 matcher（WebSearch 工具族、GET/HEAD + origin、精确 URL 或其他）、Engine、项目和 Profile；不要只写“已全局允许”。

模型价格异常时，依次检查“设置 → 通用 → 模型定价目录”中的来源、目录版本、发布时间和最近错误：

- 国内网络无法访问厂商官网是预期场景；客户端应使用国内镜像、已验签缓存或安装包内置目录。
- 立即更新失败时先确认 JSON 与同地址 `.sig` 都已发布、CDN 没有改写正文；主源失败会自动尝试备用源。
- 离线导入必须同时提供同目录同名 `.json` 与 `.sig`。签名损坏、目录超过限制或 `sequence` 倒退会被拒绝，旧缓存不会被清除。
- 兼容网关采用官方参考价时只表示估算；账单不一致应改用 Provider 报价、倍率或模型手动覆盖，并保留 Provider 账单作为最终依据。

维护者排障与回滚流程见 `docs/模型定价目录运维.md`。

当前仓库尚未声明正式支持渠道和服务等级。若仓库启用了 Issues，可用于普通缺陷和功能建议；安全问题必须遵循 `SECURITY.md` 的私密报告流程。公开发布前，项目所有者应补充正式支持地址、响应范围和版本生命周期。
