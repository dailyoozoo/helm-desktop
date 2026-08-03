# 安全策略

## 支持范围

Helm 尚处于 0.x 开发阶段。安全修复优先覆盖当前主分支和最新公开版本；旧开发构建不承诺长期支持。

## 报告漏洞

请勿在公开 Issue 中披露密钥、完整提示词、用户文件、会话数据库或可直接利用的漏洞细节。

如果代码仓库启用了 GitHub Private Vulnerability Reporting，请优先通过仓库的 **Security → Report a vulnerability** 私密提交。若该入口不可用，请联系实际分发者并要求提供私密安全渠道。正式公开发布前，项目所有者必须在此补充稳定的安全联系邮箱和响应时限。

报告建议包含：

- 受影响版本、操作系统和 CLI 版本；
- 最小复现步骤与影响范围；
- 已脱敏的日志或截图；
- 是否涉及密钥、任意代码执行、路径穿越、更新签名或权限绕过。

## 敏感信息

不要提交 `.env`、API Key、OAuth token、钥匙串导出、真实用户路径内容或未脱敏会话。诊断材料应只包含必要的版本、错误类别和配置结构。

RuntimeManaged 不等同于 Helm/OS 级隔离：`standard` 读取之外遵循 Runtime，`auto` 只对 Runtime 暴露且满足 `SafeNetworkRead` 的结构化网络审批做本地快放行。搜索词/URL 可能外发；Runtime 自己未暴露给审批桥的动作不经过 Helm 逐动作重分类。需要硬网络边界时必须使用独立 OS sandbox 或受控网络环境；产品不再提供 Helm 保护路径。权限审查应同时记录 Engine、Profile、Runtime capability、是否有 approval 事件和实际 matcher，不能只记录“Global”。

## 发布安全门禁

正式分发前必须完成依赖审计、Windows Authenticode、更新包签名、双版本升级与坏签名拒绝验证。minisign 更新签名不能替代操作系统安装包签名。

模型定价目录使用独立 minisign 公钥验证，并同时执行单调 `sequence` 防降级、2 MiB 目录/64 KiB 签名限制、schema/alias/价格档位校验和原子缓存替换。坏签名、超限、降级或网络失败不得覆盖上一个已验证缓存。价格目录私钥必须留在仓库外；疑似泄露时应停止发布并通过应用更新轮换内置公钥。
