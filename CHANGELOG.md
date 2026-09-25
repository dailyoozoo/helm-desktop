# Changelog

本项目遵循 Keep a Changelog 的基本结构；0.x 阶段可能包含不兼容的配置或数据变更。

## [0.6.2] - 2026-09-25

### Fixed

- 角色模式（非订阅 Anthropic 兼容）模型目录语义修正：用户手动保存的模型（price_source = Manual）重新保存服务商时不再被清空，避免绑定在启动 / 解析时找不到模型；同步残留目录项照常清理。同步 / 发现所得的带价模型来源归入 Provider（厂商价目），与 Manual（用户手动录入）区分，定价口径不变（`normalize_saved_model` + 角色模式保存清理逻辑）。
- 依赖安全更新：`xlsx` 从 npm 0.18.5 切换到 SheetJS 官方 CDN 0.20.3，修复 GHSA-4r6h-8v6p-xvw6（原型污染）与 GHSA-5pgg-2g8v-p4x9（ReDoS）；cargo-audit 配置迁移至 `src-tauri/.cargo/audit.toml`（0.22 版仅识别项目根 `.cargo/` 位置）。

### Removed

- 检查点/回溯能力整体退役（2026-09-09 按用户决议）：不再自动创建检查点（删除 `create_auto_checkpoint_for_tool`、`checkpoint_target_path`、`checkpoint_id_for_tool` 及 Claude 解析循环调用点），删除 `restore_checkpoint`/`undo_revert` Tauri 命令、Rust `AgentEvent::Checkpoint` 协议变体、`save_checkpoint`/`get_checkpoint`/`revert_messages_after`/`unrevert_messages`/`clear_cli_session` 持久化接口与 `SessionCheckpoint`/`CheckpointRecord` 结构；`packages/protocol` 删除 `checkpoint` 事件、`restore_checkpoint` 命令与校验分支。前端删除 `CheckpointItem` 组件、线程检查点渲染、`restoreCheckpoint`/`undoRevert` 传输与 reducer、右栏活动日志检查点项与 `.ckpt` 样式。SQLite `checkpoint` 表、schema 迁移与历史快照保留（不再写入、无 schema 升版）；历史 reverted 标记仍只读展示；删除会话仍清理历史快照。**交付物判定同日改走同业口径**：写族工具（`Write`/`Edit`/`MultiEdit`/`NotebookEdit`/`apply_patch`）成功调用 + 结构化文件路径即计入产出（`WRITE_FAMILY_TOOLS`），不再要求行级 diff——修复新版 Claude Code CLI 的 Write 结果无 diff 块导致「写文件但不显示交付物行、无法打开右栏」的断链。PRD、术语表、AGENTS.md、已知限制同步。验证：`npm run check` 89 文件 892/892 通过；`cargo check --tests` 通过，`session_history`/`turn_supervisor` 定向测试见当日验收记录。

### Changed

- 原型侧栏视觉修正（2026-08-21）：① 最近任务「视图选项」弹框按「分组方式 / 排序方式」两节归类（`floatmenu` 新增 `head` 小节标题，排序项标签去重、图标区分）；② 侧栏最近任务、全部任务页（任务行 + 引擎筛选 chip）、设置页内嵌任务列表的引擎标识统一换用官方品牌 logo（Codex → `assets/brands/openai.svg`，Claude Code → `assets/brands/claude.svg`，深色下 OpenAI 反色），替代语义模糊的 `cpu`/`zap`；③ 主导航「AI 配置」图标由 `slidershorizontal` 改为 `server`，消除与「视图选项」按钮同屏撞车（`commercial.js`/`app.js` 三处同源，`prototype-audit.mjs` 断言同步；实现层 `src/shell/Rail.tsx` 同步改用 `server`，与 HomePage 快速入口一致）。原型审计恢复 209/234 既有基线。
- 变更-34/35 切片 B（可靠性检查整改）：修正 Turn 与压缩事实（P0-03/P0-04/P1-05）。**P0-03 StatusBar 本轮口径修正**：`StatusBar` 改为 active Turn 投影——`statusBarModel(items, activeTurnId)` 按 `turnId` 过滤只统计本轮工具/diff，不混入上一轮；`StatusBarData` 新增 `turnCostUsd`（本轮真实累计成本，由 `useSession` 按 envelope `turnId` 累加 `token_usage.costUsd`，`send` 清零/`turn_complete` 不清零/`resume_handle` 重置），无本轮 Usage 事实时不显示成本（不用会话累计 `cost.costUsd` 代替）；`StatusBarData` 移除 `cost` 字段。**P0-04 Codex contextCompaction 生命周期**：共享协议 `events.ts` + Rust `protocol.rs` 新增 `context_compaction` 事件（status: submitted/running/succeeded/failed + 可选 summary/error）与 `ContextCompactionStatus` 枚举；Rust `parse_codex_app_server_notification` 解析 `item.type=contextCompaction`（started → Running，completed → Succeeded/Failed，不补写虚构摘要）；`sessions.rs`/`turn_supervisor.rs` 的 AgentEvent match 补 `ContextCompaction` 分支（当前不单独落库，实时 UI 靠前端 reducer）；前端 `useSession` 新增 `ThreadItem kind=compact` + `context_compaction` reducer（同 id 更新不重复追加）；`Thread.tsx` 渲染 `CompactItem`（`src/workspace/items/CompactItem.tsx`）可展开记录卡；`Workspace.handleCompactContext` 提交后只 toast「已提交压缩」。**P1-05 压缩提醒按 Session 隔离**：`compactDismissed` 从组件级布尔值改为 `compactDismissedRef: Set<string>`（key = historyId/sessionId/handleId），切换/新建会话时新身份不在集合中 → 重新提醒；`compactLastPercentRef: Map<string, number>` 按会话追踪上次占用百分比，占用回落 80% 以下后再次跨阈值时清除 dismiss 允许重新提醒。前端新增 11 项测试，Rust 新增 3 项测试。`npm run check` 全绿（60 文件 446/446）、`cargo check --tests` 无错误。

- 变更-34/35 切片 C（可靠性检查整改）：补齐工作区监督面（P1-01/P1-06）。**P1-01 工作区侧栏 F1/F2**：`SessionSidebar` 新增状态筛选 chip 条（全部/等审批/运行中/失败/已归档），复用 `filterSessions`（与 SessionsPage 同一套派生逻辑，非第二套判断）；新增纯函数 `sidebarStatusCounts`——归档只计入「已归档」不计入「全部」，等审批/运行中/失败由 `pendingApproval`/`active+currentTool`/`lastTurnFailed` 真实字段派生；会话卡第三行 `sitem__do` 复用 `currentActionText()`/`changeScaleText()`，无动作且无 diff 时不渲染；右键/kebab 菜单增加「归档/取消归档」，复用 `setSessionArchived` API（与 SessionsPage 同一命令），归档会话 `is-archived` 降透明度。**P1-06 后台任务停止范围语义**：`BackgroundTask` 停止按钮「停止」→「停止本轮」；`TasksPanel` 后台命令区在存在运行中任务且有 `onStopTask` 时顶部显示停止范围说明（中断当前轮次，同轮其他工作一并停止，暂不支持单任务取消）。新增图标 `archive`/`dot`（对齐原型）。前端新增 4 项 `sidebarStatusCounts` 测试 + TasksPanel 测试更新。`npm run check` 全绿（60 文件 450/450）。

- 变更-34/35 切片 A（可靠性检查整改）：恢复交付物主路径（P0-01/P0-02/P0-05/P1-04）。**右栏常驻 tab** 从「活动/文件/上下文/工具」收敛为「变更/活动」两个（对齐原型 `workspace.html`），默认激活 tab 从 `context` 改为 `changes`；files/context/tools 改为按需打开的动态 tab（Composer 圆环点击 → 打开 `context` 动态 tab）。**有 diff 自动打开右栏**：宽屏（≥1280px）+ 有真实 diff 时自动 `setShowCtx(true)`（`autoOpenedForDiff` ref 防重复触发）。**关闭动态 tab 回退到变更**：`closeDynTab` 不再返回 `null`，而是回退到常驻的 `changes`。**工具 diff 块新增「变更」入口**：`ToolBlock` 新增 `onOpenChanges` prop + 「变更」按钮（有 diff 时出现，点击打开右栏变更审阅面）。**计划/终端动态 tab 读取真实 Ledger**：新增 `PlanPanel`（投影真实 `PlanItem` 步骤，含状态/来源/定位回线程）和 `TermPanel`（聚合真实终端工具调用，展示命令/状态/完整有界输出/定位回线程），删除所有「切片 4 接入」占位文案；**预览 tab** 因当前无真实 dev server 预览能力从动态列表移除（不保留永远不可达的占位）。**ThreadNav 定位修正**：`.thread__scroll` 添加 `position: relative`，使 navfab 浮标锚定滚动区底部而非整个线程列底部（不侵入 Composer 发送区）；移除 `.thread` 的 `position: relative` 覆盖。**右栏宽度 token 同步**：`--ctx-w` 从 `330px`（紧凑 `306px`）同步为原型的 `clamp(360px, 34vw, 560px)`；`ResizablePane` `MIN_WIDTH` 从 300 提升到 360；挂载与最大化还原时旧持久化值低于 360px 自动丢弃。测试更新 3 项（`closeDynTab` 回退到 `changes`、`ResizablePane.test.ts` 最小宽度 300→360、`openContextRequest` 依赖项修正）。`npm run check` 全绿（60 文件 435/435）。

- 变更-34/35 切片 A（可靠性检查整改）：恢复交付物主路径（P0-01/P0-02/P0-05/P1-04）。**右栏常驻 tab** 从「活动/文件/上下文/工具」收敛为「变更/活动」两个（对齐原型 `workspace.html`），默认激活 tab 从 `context` 改为 `changes`；files/context/tools 改为按需打开的动态 tab（Composer 圆环点击 → 打开 `context` 动态 tab）。**有 diff 自动打开右栏**：宽屏（≥1280px）+ 有真实 diff 时自动 `setShowCtx(true)`（`autoOpenedForDiff` ref 防重复触发）。**关闭动态 tab 回退到变更**：`closeDynTab` 不再返回 `null`，而是回退到常驻的 `changes`。**工具 diff 块新增「变更」入口**：`ToolBlock` 新增 `onOpenChanges` prop + 「变更」按钮（有 diff 时出现，点击打开右栏变更审阅面）。**计划/终端动态 tab 读取真实 Ledger**：新增 `PlanPanel`（投影真实 `PlanItem` 步骤，含状态/来源/定位回线程）和 `TermPanel`（聚合真实终端工具调用，展示命令/状态/完整有界输出/定位回线程），删除所有「切片 4 接入」占位文案；**预览 tab** 因当前无真实 dev server 预览能力从动态列表移除（不保留永远不可达的占位）。**ThreadNav 定位修正**：`.thread__scroll` 添加 `position: relative`，使 navfab 浮标锚定滚动区底部而非整个线程列底部（不侵入 Composer 发送区）；移除 `.thread` 的 `position: relative` 覆盖。**右栏宽度 token 同步**：`--ctx-w` 从 `330px`（紧凑 `306px`）同步为原型的 `clamp(360px, 34vw, 560px)`；`ResizablePane` `MIN_WIDTH` 从 300 提升到 360；挂载与最大化还原时旧持久化值低于 360px 自动丢弃。测试更新 3 项（`closeDynTab` 回退到 `changes`、`ResizablePane.test.ts` 最小宽度 300→360、`openContextRequest` 依赖项修正）。`npm run check` 全绿（60 文件 435/435）。

### Added

- 首启安装引导（2026-09-02）：新装用户进入工作台自动弹出安装向导 `SetupWizardModal`（从设置页「关于 → 进入安装向导」抽出为 `src/settings/SetupWizardModal.tsx` 共享组件，两处同一实现）。触发 = `get_readiness_report` + `detect_workspace_deps` 探测 CLI/Git/服务商/工作目录四项未全就绪且未跳过过（老用户全就绪无感）；「跳过，稍后再说」或完成后写 localStorage `helm:setup-wizard-dismissed`（纯 UI 偏好，不进 app_settings），此后不再自动弹。CLI 行跟随**引导引擎** `selectGuideEngine`——只装 Codex 按 Codex 引导并自动把 `defaultEngine` 同步为 codex；只装 Claude Code / 两个都装 / 两个都没装默认 Claude Code。CLI 项就绪口径放宽为「任装其一」（`setupWizardAllReady`）。`App.persistSettings` 对无变化 updater 短路避免冗余保存。单测 `SetupWizardModal.test.tsx` 9 例。**注意**：与 2026-07-07 Removed 的旧版总览首屏五步 `OnboardingWizard`（`onboardingCompleted` 状态）非同一实现；商业化方案 §4.3 五步向导（订阅/API 分路径）仍为后续项 P1-3。详见 `docs/已知限制.md` 2026-09-02 首启安装引导节。

- 变更-37：CLI 环境引导。用户没有装 Claude Code / Codex 时，发送前就地拦截并渲染引导卡（`SetupGuide`，`src/workspace/SetupGuide.tsx`），不弹窗、不跳设置页；按依赖分两层——**Node/git 共享依赖**（装一次两引擎受益）+ **当前引擎独立 CLI**（Claude 装好切到 Codex 会话时 Codex CLI 行回到缺失、重新引导）。`handleSend`（`Workspace.tsx`）在绑定/目录校验**之前**插入「Node/git/当前引擎 CLI」检查，缺任一项触发 `sendBlocker.action='setup'`；已有 `handleId` 的会话已通过此闸门，复检由运行期 `ErrorItem.not_installed` 兜底覆盖。git 缺失时引导卡常驻、无「知道了/跳过」按钮（强制前置）。**安装源走国内可直连镜像，不引导科学上网**：CLI 安装（`install_cli_engine` 改造）尊重用户已有 registry → 官方源失败自动切 `registry.npmmirror.com` 兜底；Node 安装（`install_node`）从 npmmirror 镜像下载 LTS MSI + SHA-256SUMS 验签 + `msiexec /quiet /norestart ALLUSERS=2 MSIINSTALLPERUSER=1` per-user 静默安装；git 安装（`install_git`）从 git-for-windows 国内二进制镜像下载 64 位 exe + SHA-256SUMS 校验 + `/VERYSILENT /CURRENTUSER` 静默安装。每项安装成功立即复检（PATH + 已知安装目录），PATH 未刷新标记 `restartRequired=true` 并提示「重启 Helm 后生效」。npm 不可用时返回「先装 Node」引导。**进度红线**：状态机 missing → installing → ok，无假百分比进度条；`error` 文案对齐诊断文本，不伪造成功。原型走查 `scripts/proto-setup-guide-audit.mjs` 18/18 断言通过；前端 Vitest 8 项、Rust 单测 10 项全绿。详见 `docs/变更-37-CLI环境引导方案.md`。

### Changed

- 移除工作目录独占 lease（原 `WorkspaceExecutionCoordinator` / `[workspace_busy]`）：同目录或父子重叠目录的多个 Build 轮次允许并行，与 `claude -p` / `codex` 原生行为一致；原先「同一会话运行时开另一个同目录会话会报 `[workspace_busy]`」的拦截已删除。同文件竞争由 Diff 展示与检查点兜底（检查点快照可能混入另一会话改动，还原会连同覆盖）。删除 `workspace_execution.rs` 模块与其两处轮次内 acquire（Claude/Codex）、注册与 `lib.rs` 管理项；`codex_interrupt_abort_releases_workspace_lease_before_returning` 测试改写为 `codex_interrupt_abort_clears_busy_and_awaits_task`（仍覆盖 Stop 兜底回收）。PRD、已知限制、术语表同步。

### Removed

- 首启引导向导整体移除：删除 `OnboardingWizard` 组件、`settings.general.onboardingCompleted` 状态与设置页"重新运行引导"入口，以及对应 `.ob-*` 样式；首启不再以总览页+向导为第一屏，默认直进工作区。就绪度命令、发送前置校验、设置页 CLI 安装与错误分类保留。
- 移除手动会话文件夹概念：Session 一律按 canonical cwd 自动归档，`create_session` 不再接受 `folderId`；删除 `create_folder` / `rename_folder` / `delete_folder` / `set_session_folder` 四个 Tauri 命令及其前端 API、侧栏「新建/移动到文件夹」菜单、线程头文件夹 chip 与对应原型/视觉。Store 层同名方法保留供既有集成测试与向后兼容。

### Added

- 附件内容注入（`prompt_with_attachments`，`src-tauri/src/adapter.rs`）：@ 提及/附加文件发送时不再只把路径丢给 agent 自行读取，而是**发送端提取内容注入 prompt**——文本/代码类直接读入（前 1KB 含 NUL 判为二进制不注入）；`.docx`（zip→`word/document.xml` 剥标签+实体反转义）、`.pptx`（`ppt/slides/slide*.xml` 逐页）、`.xlsx`（`xl/sharedStrings.xml` 文本单元格）走 zip 解包提取；`.pdf` 走 `pdf-extract` 解析；图片/未知二进制保留路径（视觉能力可读），提取失败自动回退路径列表。上限：单文件 24K 字符、累计 80K 字符，超出截断并注明「（内容过长已截断）」。覆盖 Claude、Codex、Codex app-server 三条路径（共用同一函数）。新增 `zip` 与 `pdf-extract` 依赖；新增 4 项 Rust 单测（文本注入、docx+xlsx 提取、二进制 NUL 回退、旧路径列表回归）。
- 变更-34/35 切片8（2026-08-12）：上下文压缩 banner + 常驻执行状态条 + 同引擎派生 + **Codex 原生压缩按钮（2026-08-12 更正）**。**压缩 banner**（`CompactBanner`，`src/workspace/CompactBanner.tsx`）：`context_usage` 最近一次真实输入规模 ≥80% 时在 Composer 上方出现，纯提醒不拦截发送；**Codex** 提供真实「压缩上下文」按钮——Codex app-server 官方提供 `thread/compact/start` RPC（真实 headless 契约，早前「无原生压缩命令」结论系错误，已更正），后端新增 `compact_context` 命令（handle→session→`CodexRpcClient.compact_thread`，busy 时禁用/拒绝），前端 `compactContext` 传输层；**Claude** 显示「接近上限时自动压缩」（`claude -p` headless 无 `/compact` 注入契约，官方 issue #1131 实证，不造假按钮）；两者统一提供「从摘要派生同引擎新会话」（后端 `freeze_fork_input` 已放开同引擎限制，新增 `freeze_allows_same_engine_fork` / `freeze_allows_cross_engine_fork` / `freeze_rejects_busy_source_turn` 三个 Rust 测试），可手动关闭本会话提醒。两引擎接近上限都会自动压缩（Codex 默认 90% 触发）。**执行状态条**（`StatusBar`，`src/workspace/StatusBar.tsx`）：Turn 运行时显示于 Composer 上方，只展示后端真实事件——当前动作（真实 `TurnStage` + 工具名/目标中文文案 `statusBarLabel`）、真实经过时长（由 `turnStartedAt` 时间差每秒刷新，无伪进度）、本轮真实工具调用数、改动文件数与 ±行数（`statusBarModel` 聚合真实 diff 行，无 diff 为 0）、真实用量成本；含「停止」与「补充要求」按钮（补充要求聚焦 Composer 输入），仅显示 `working` 期间。新增 Rust 测试：`compact_thread_issues_thread_compact_start_rpc`、`compact_thread_propagates_rpc_error`、`claude_compact_context_rejects_without_contract`。**同时移除（原型/术语表/文档）**：F3 建议任务 chip（真实 CLI 不产生「范围外问题」信号，无真实数据源）与 D2 插队转向（`interrupt` 已覆盖，排队「等本轮结束」+「停止后手动发送」满足需求），术语表删除 `Steer`/`SuggestedTask`，原型 `workspace.html` 删除 `sugg`/`qsteer`/`steerTurn`/`swch` steer 分支与相关 CSS，方案文档/实施计划同步 7→5 术语与 DoD。`npm run check` 全绿。
- 变更-34/35 切片5 D4 / 切片7 E2 / A5 补齐（2026-08-12）。**上下文圆环**（`ContextRing`，`src/workspace/ContextRing.tsx`）：Composer 右下角常驻圆环指示器，三级递进——圆环显示最近一次调用真实 `context_usage` 占用百分比（≥80% warn、≥95% danger），hover 摘要显示 文件数/MCP 数/总输入 token，点击展开完整 popover（占用口径 + 归因 + 计费四账 + 会话 + 上下文中的文件 + MCP + 在右栏打开）；数据来自 `context_usage` 真实字段与 `billingSummary` 纯函数，无逐调用数据时显示占位符「—」，不拿累计计费值估算上下文（AGENTS.md 红线）。**占用归因**（`AttributionView` + `attributionViewModel.ts`）：按来源列出输入规模、`is-hot` 标注占比最高项并给出降低建议；当前引擎未按来源报告规模时整体显示「暂无归因数据」，不伪造逐项占比。**A5 最大化/收起**：ContextPanel tabbar 右侧新增「最大化/还原」（`--ctx-w` 顶到 `min(92vw,1440px)`，还原回 localStorage 拖拽记忆值）与「收起右栏」按钮。C3 跟随Agent 按原型 v3 决策标记为已移除（同业无此功能，收益不抵常驻按钮成本），不排期实现层。新增 `crosshair`/`expand`/`compress` 图标与 `.ctxring/.attview/.attrow/.atttip/.ctx__panebtn` 样式（对齐原型）。`npm run check` 全绿（56 文件 401/401）。
- 变更-34/35 切片6：工具展示增强（C1/C2/C4/E1）完成。**并行子代理卡**（`SubagentCard`）：Claude `Task` / Codex `Agent`/`subagent` 工具按连续同 Turn 聚合为一张卡（`threadGroups` 新增 `subagent` 渲染条目，进入轮次过程容器），显示 名称/任务/状态/耗时/可展开产出，头部「查看全部任务」一键打开任务 tab；耗时与任务描述从真实工具 input/startedAt/endedAt 提取（`taskViewModel` 纯函数）。**后台命令**（`BackgroundTask`）：终端工具带 `is_background` 或 `timeout ≥ 10 分钟` 才被识别为后台命令（不靠猜），在线程终端卡照常展示、任务 tab 内提供「停止」（真实能力为中断当前轮次）。**失败终态增强**（`FailureCard`）：错误分类（权限/网络/凭据/超时/工具/模型，优先级：Runtime 拒绝 > 输出文本特征 > 工具名），说明「重试能否自愈」（权限/凭据不可自愈），显示同 Turn 同名工具真实重试次数（`retryCountFor`，不伪造），「重试这一步」把失败工具连同输出摘要作为一条真实用户消息发回 Agent（`retryRequestText`），「复制报错」走剪贴板；轮次仍在运行时重试按钮禁用。**任务 tab**（`TasksPanel`）：右栏动态 tab 新增「任务」（`ArtifactPaneTab` 扩展），列出子代理与后台命令，支持在线程中定位与展开子代理产出。新增 `--agent-*` 设计 token（light/dark/system 三态）与 `.sagent/.sarow/.failc/.taskpanel` 样式，全部对齐原型 `workspace.html`。

- 变更-34/35 切片7（F1/F2）：会话筛选与归档、会话卡信息升级完成（E2 占用归因当时未排期，已于 2026-08-12 补齐实现层，见下方「切片5 D4 / 切片7 E2 / A5」条目）。状态筛选 chip 扩展为「全部 / 等审批 / 运行中 / 失败 / 已归档」：等审批由 `approval.pending` 推导，运行中由 `tool_call.pending` + active 推导，失败由最近 `turn_snapshot` 状态推导，已归档由 `session.archived` 列读取，均非伪造状态。会话行菜单新增「归档/取消归档」（`set_session_archived`，可逆、保留历史与用量、区别于删除），归档会话整行灰色标记。会话卡显示「当前在做什么」（最新 pending 工具名 + input 目标字段如 path/command 提取的目标摘要，如「正在写文件 auth.ts」）与变更规模 `+N -M`（跨轮累计 `tool_call.diff_json` 真实 diff 行数按 add/del 聚合，无 diff 显示暂无），并新增按变更规模排序。SQLite 升至 schema v31（`session.archived` 列）。术语表 `SessionArchive` 核对一致。
- 变更-34/35 切片3：行级审阅意见与自评审（A2/A3）完成。`DiffView` diff 行 hover 出现「就这一行留下意见」入口，行号标记与原型对齐（newNo 优先、del 回退 oldNo）；行内草稿编辑器 `ReviewNoteEditor`（回车/记下/取消）；`ReviewNoteBatch` 攒批条位于变更审阅面底部，显示「N 条审阅意见待交回」+ 清空/交回，`Ctrl+Enter` 快捷键交回。回灌时 `reviewNotesToText` 生成「审阅意见 · N 条」文本（`file:line — 意见` 列表）作为一条真实用户消息经 `Workspace.handleSend` 发出，不落盘、不产生伪撤销。「让 Helm 自评审」按钮调用后端 `review_changes`（`src-tauri/src/self_review.rs`）：用当前 Binding 的真实 fast model（缺失回落 primary）经 ModelOnlyOperationPolicy + 真实 CLI Adapter 审阅会话当前 diff，只报编译/逻辑/安全与明显 bug、明确不报风格，结果幂等缓存（相同 diff digest 复用）；AI 意见以 `.rnote.is-ai` 样式与人工意见区分。契约测试红灯随前端 `invoke('review_changes')` 接线关闭。人工验收（真实 CLI + Provider 端到端、录屏/截图）待补。
- 变更-34/35 切片4：视图密度两态与轮次容器完成。视图密度按原型 v3 最终态实现为 `std` 标准 / `lite` 专注两态（对齐 Claude Code focusView）：`TranscriptDensity` store 持久化到 localStorage `helm:density`；开关在设置页「外观 · 专注模式」，工作台不放控件，`Ctrl+O` 两态循环保留为调试快捷键。专注模式下 `.dens-lite` 通过 `data-kind` 属性隐藏思考/工具/工具组/终端/活动行/子代理，只留提问、结论与交付物，轮次收成一行。轮次摘要头 `TurnSummaryHead` 显示 第N轮·模型·耗时·工具数·±行数（缺项不显示、可折叠整轮），数据来自 `turnSummary` 纯函数（模型取 TurnLedger `SessionTurn.routedModelId`）；`ThreadNav` 线程导航浮标支持 回到开头 / 上一个提问 / 下一个提问 / 回到最新。
- 变更-34/35 切片5 D3：旁路提问（SideQuery）实现层打通。后端新增 `side_query` 命令（`src-tauri/src/commands.rs`）：读当前 Session 最近若干条可见消息与工作目录，经真实 Claude Code CLI 以无工具 ModelOnlyOperationPolicy（capability 动态协商，Codex 等无原生无工具合同 fail-closed）跑一次性问答，结果直接返回前端；**零持久化**——不写回 `SessionContext`、不产生 Turn/Operation/用量记录、SQLite 无旁路提问痕迹。前端 `src/workspace/SideQuery.tsx` 右滑面板（`Ctrl+;` 开关，视觉对齐原型 `.side`）+ `transport.sideQuery` + 协议 `SideQueryArgs`。旁路提问标注「临时提问 · 不影响主线程」。
- 变更-34/35 切片1：右栏动态 tab 系统基础完成。右栏（`ArtifactPane` 容器）在活动/文件/上下文/工具常驻 tab 之外，支持「变更/计划/终端/预览」交付物动态 tab 按需出现：线程计划卡新增「在交付物区查看完整计划」、Bash 终端卡新增「在交付物区查看全部命令输出」；动态 tab 带 `×` 可关闭，关闭当前动态 tab 自动回落常驻页。新增 `ResizablePane`（`src/workspace/ResizablePane.tsx`）：右栏宽度可拖拽并持久化到 localStorage（`helm:ctxw`），最小 300px、最大 60vw，drawer 态（≤1279px）隐藏拖拽手柄。动态 tab 状态机抽为纯函数（`openDynTab`/`closeDynTab`）并补单测；`clampPaneWidth` 覆盖宽度钳制。
- 变更-34/35 切片2：变更审阅面（A1）完成，右栏「变更」动态 tab 由占位换为真实审阅面。`ChangeReview`（`src/workspace/ChangeReview.tsx`）上文件清单（`FileTree`，状态徽标 + 累计 `+N −M`）+ 下 diff（`DiffView`）：统一/并排双视图、折叠未变更行（展开显示真实行号区间）、跨 hunk 上/下一处导航（`DiffNavigation` + `flattenHunkTargets` 纯函数）、导航命中 hunk 高亮。数据源为 `state.items` 中携带真实 Diff 的工具项（回滚项排除），按文件聚合、行号重算、状态 a/m/d 推断，全部抽为纯函数 `changeReviewFiles` 并补单测。注意：真实协议 Diff 只携带变更行，折叠区 "完整内容" 不可得，展开态按设计注明「内容未随变更记录」。
- 变更-34/35 切片0：实现层导航统一完成。7 个页面统一由共享 `Rail`（`src/shell/Rail.tsx`，`RAIL` 单一真值对齐原型）、`.body` 布局统一为 `grid-template-columns: var(--rail-w) 1fr`；workspace 无侧栏导航菜单按钮、sessions 无页头品牌导航，清理 `.body--flat`/`.pagenav` 旧样式（变更-35 v3，推翻 v2 移除图标栏决策）。新增 `Rail.test.tsx` 覆盖 7 入口、当前页高亮与新建会话入口。
- 变更-34/35：原型层工作台升级与导航统一完成（实现层待排期）。21项改进包括：交付物区(右栏动态tab，变更/计划/终端/预览按需出现)、视图密度(初版三档，v3 已收敛为 std/lite 两态)、轮次容器与摘要头、行级审阅意见回灌、让Helm自评审、旁路提问(`Ctrl+;`)、Composer上下文三级递进(圆环→hover→popover)、子代理卡、失败终态增强、任务面板、占用归因、会话筛选与归档。**导航决策演进**：v2曾移除图标栏改用PageNav，v3(最终态)恢复图标栏统一到全部7页，单一真值在`app.js`的`RAIL`常量。原型走查57/57断言通过。详见`docs/变更-34-35-工作台原型升级统一方案.md`（已整合变更-34与变更-35，旧文档归档至`docs/archive/`）。
- 变更-31：Codex Responses 交互 Runtime 接入原生 hosted WebSearch；Responses Lite + hosted-only Provider 使用同 binary bundled catalog 的摘要绑定兼容目录，能力由 Provider 合同、模型模式和真实 ToolCall 观察共同裁决。
- 变更-32：Session 范围的 ProcessExec 授权从"精确 argv 指纹"放宽为"可执行文件身份"（canonical executable + SHA-256 + 引擎 + cwd，ADR 0016）；一次"本会话允许"后同会话内同一可执行文件不再逐条弹框，Turn/Project/Always 仍精确 argv 匹配。
- 变更-33：ContextPanel 内嵌文件/附件预览面板，历史附件、工作区文件变更、Git 暂存区文件点击直接内嵌预览；后端新增两条只读命令 `read_file_preview`（文本 UTF-8 复用 `redaction` 脱敏、图片返回 base64、二进制返回类型标记，敏感路径拒绝，文本 256 KiB / 图片 2 MiB 上限）与 `open_path_in_system`（`opener` crate 系统默认程序打开，敏感路径拒绝）。二进制/非图片/超大文件提供"用系统默认程序打开"按钮。
- Codex app-server 协议 Trace 增强：`HELM_CODEX_PROTOCOL_TRACE`（非 "0" 的值即启用）时，`trace_outbound` / `trace_inbound` 额外输出 `thread/start`、`turn/start` 的 cwd / writableRoots / sandboxPolicy / approvalPolicy，以及 `requestApproval` 的 action type / files / additionalPermissions，便于定位工作区写审批的边界事实。

### Changed

- 审批弹窗收敛为三选项：当次允许（`allow`）/ 总是允许（仅当前会话 `session`）/ 拒绝（`deny`）；「本轮/此项目/所有项目」跨会话持久范围不再从弹窗下发。后端 `available_approval_decisions` 只下发该三值（无稳定 matcher 时仅当次/拒绝），前端仅渲染后端权威集合。底层枚举、`PermissionScope`、规则构成与设置页撤销/审计逻辑保留，既有 project/global 历史记录仍可展示与撤销；提交端 fail-closed 白名单校验不变。
- SafeFileWrite 快路径：`auto` 档会话对 `FileWrite`（含 Codex `fileChange`、Claude `Write`/`Edit`）在工作区内且非保护敏感/非 ADS 的写目标不再逐个弹审批，直接 Allow；越界/敏感 fail-closed 为 Deny，`standard`/`full_access`（持久化 standard）保持原有 Ask 行为不变，显式 Deny 优先级仍高于本快路径。

### Fixed

- @ 提及文件联想修复（`search_workspace_files`，`src-tauri/src/commands.rs`）：①搜索只匹配文件名、目录名不参与匹配，`@docs` 永远搜不到 `docs/` 下的文件——改为对完整相对路径匹配（目录名/路径前缀命中即返回其下文件）；②`SKIP_DIRS` 只精确跳过 `target`，`target-change*` 等变体构建目录漏网，`@doc` 返回的 30 条结果被 `displaydoc-*.d/.dll/.pdb` 产物占满——改为同时跳过 `target-*` 前缀目录；③遍历中途满 30 条就 break，排序只作用于先遇到的根目录文件，子目录文件永远进不了菜单——改为遍历完（`MAX_SCANNED` 兜底）再按路径长度排序并截断 30 条。新增 2 项 Rust 单测（目录名命中等价路径匹配 + `target-*` 跳过与 30 条截断）。
- 变更-36：订阅会话技能隔离修复。**病灶 ①**：订阅隔离目录（`cli-profiles/codex-subscription`、`cli-profiles/claude-subscription`）此前只随登录自动生成 `.system` 内置技能，用户自定义技能从不复制，订阅 Codex 会话中 `$neat-freak` 等自定义技能全部「不可用」；现每次会话启动前（`ensure_binding_runtime_ready` 订阅分支）把真实 `~/.codex/skills` / `~/.claude/skills` 的用户技能镜像同步进隔离目录：排除 `.system`（Codex 运行时自维护）与 `.helm-disabled`（Claude 停用区），目标缺失或源内容更新才整目录换新，源已删除的技能目录镜像删除，只操作 `skills` 子树、不触碰 `auth.json` 与订阅登录态，同步失败仅日志、不阻断会话启动；新增/修改/删除技能后重开订阅会话即生效。**病灶 ②**：UI 技能扫描器 `read_skills_from_dir` 只扫一层，`skills/.system/<name>/SKILL.md` 嵌套结构整目录被跳过，Codex 内置 6 技能在技能中心/右栏/` 补全中永不显示；现对 `.system` 递归展开其直接子目录为 `source=builtin` 技能（`id`=子目录名、`trigger`=`$<id>`，名称/描述仍从子目录 `SKILL.md` 解析），技能中心与补全显示内置技能。Claude 内置技能在 CLI 二进制内、磁盘不可扫描，UI 不显示为已知限制（后续独立变更）。
- WebSearch 成功后把 capability projection 持久化到当前 Session；Codex Runtime Profile 校验兼容精确 UI/project 自写状态，同时继续拒绝非 trusted project 和其他配置篡改，避免重启 resume 被误阻断。

## [0.6.1] - 2026-09-18

### Fixed

- 角色模式（非订阅 Anthropic 兼容）模型目录语义修正：用户手动保存的模型（price_source = Manual）重新保存服务商时不再被清空，避免绑定在启动 / 解析时找不到模型；同步残留目录项照常清理。同步 / 发现所得的带价模型来源归入 Provider（厂商价目），与 Manual（用户手动录入）区分，定价口径不变。
- 依赖安全更新：`xlsx` 从 npm 上的 0.18.5 切换到 SheetJS 官方 CDN 的 0.20.3，修复 GHSA-4r6h-8v6p-xvw6（原型污染）与 GHSA-5pgg-2g8v-p4x9（ReDoS）两个 high 级漏洞；npm 注册表的 `xlsx` 0.18.5 为最终版本、无修复可用，官方建议改用 cdn.sheetjs.com 分发。API 兼容（`XLSX.read` / `sheet_to_html`），仅影响工作区右栏表格预览。
- CI 修复：修正 `claude_runtime_managed_api_binding_does_not_load_external_setting_sources` 测试在 Windows runner 上的 false negative——原断言订阅模式 args 数量为 0，未计入 `build_command` 在 Windows 上包裹的 `cmd /C` 前缀；改为校验策略注入前后 args 增量，Windows / Linux 均稳定通过。
- 依赖安全更新：将 `rustls` 升级至 0.23.45，修复 RUSTSEC-2026-0285（TLS 1.3 握手越界）。`lopdf` 的 RUSTSEC-2026-0187（深度嵌套 PDF 栈溢出 DoS）经 `pdf-extract` 间接引入，其 patched 版本需跨多小版本升级 `pdf-extract` 0.12，API 破坏性较大，暂以 `src-tauri/audit.toml` 忽略并在后续单独排期真修；该漏洞仅影响本地解析用户自选 PDF 附件的场景。

## [0.6.0] - 2026-09-18

### Added

- 拖拽附加文件：把文件直接拖到输入框即可附加，与「+」选择文件走同一条链路，拖入时给出落点提示。
- 任务标题与会话摘要自动生成：首轮结束后用当前引擎绑定的快速模型生成（缺快速模型时回落主模型），设置中可关闭；内容只发给当前绑定的服务商。
- 订阅登录与官方模型目录同步：订阅账号登录后拉取官方模型目录并勾选启用，登录与同步状态分开展示，失败给出可操作提示。
- 推理档位真实能力检测：档位来自当前服务商、模型和 CLI 的真实探测结果，不可用或待确认的档位明确标注，不再提供无法生效的开关。
- 服务商环境变量与引擎配置：变量值统一存入系统钥匙串（预览不回显明文）；Helm 引擎配置片段可直接编辑 CLI 路径与环境变量，不读写用户全局 CLI 配置。
- 探活缓存：连接测试与模型同步结果短期缓存，配置变更即失效，重复打开 AI 配置页不再反复请求网关。
- 订阅会话使用独立 CLI 配置目录，并把用户自己的技能镜像进去；不读取或迁移全局登录态。

### Changed

- 交付物判定改为同业口径：写族工具（Write / Edit 等）成功调用且带文件路径即计入本轮变更，不再依赖 CLI 是否返回 diff——修复新版 Claude Code CLI 写文件后交付物行消失、右栏打不开的问题。
- 消息链路与模型配置专项修复：流保存失败会给出独立通知，界面不再一直停在「工作中」；错误按轮次 / 尝试 / 生成代隔离，不互相覆盖；模型与服务商的配置接口与消息链路对齐。
- 工具输出、思考与计划统一按 64 KiB 上限有界展示，超出部分显式标记截断，长任务不再无限膨胀。

### Removed

- 检查点 / 回溯能力整体退役：界面不再出现检查点卡片与还原入口，历史中的已还原标记保留只读展示；删除会话仍会清理历史快照。

### Fixed

- Claude Code 新版 CLI 的写文件与 diff 结果解析：变更文件识别与变更行数统计更准确。

## [0.5.0] - 2026-09-05

### Added

- 工作区右栏收敛为「变更 / 活动」两个常驻标签页，上下文、文件与工具改为按需打开的动态标签页，有真实 diff 时自动打开右栏方便审阅。
- 变更审阅增强：支持跨 hunk 上 / 下一处导航、折叠未变更行并保留真实行号区间、命中 hunk 高亮展示。
- 会话侧栏新增状态筛选条（全部 / 等审批 / 运行中 / 失败 / 已归档），并支持对会话归档 / 取消归档。
- 新增 Codex 上下文压缩事件完整生命周期展示与记录，压缩提醒按会话隔离以避免跨会话误提醒。
- 统一导航与品牌标识：7 个工作页面统一采用图标导航栏，Codex 与 Claude Code 使用官方品牌标识区分。

### Changed

- 状态栏成本口径修正为只统计当前轮次真实累计，不再混入上一轮数据；无本轮事实时不展示成本。
- @提及文件联想搜索改进：支持目录名与路径前缀匹配、跳过构建产物目录、结果按路径长度排序后截断限。
- 后台任务停止改为停止本轮，并补充停止范围说明。

### Fixed

-修复 @提及文件搜索仅匹配文件名的问题，docs 目录下文件现在可被 @提及正常检索到。

## [0.4.0] - 2026-08-10

### Added

- 服务商与模型页重构：页头与标签改为粘性定位，表单按「基础配置 / 连接与状态」分区展示。
- 服务商连接失败按类别提示（网络 / 认证 / 超时 / 未知），并提供去修复入口；测试可达性并入连接状态区。

### Fixed

- Windows 下调用 git 与 where.exe 时不再闪现命令行窗口（子进程使用 CREATE_NO_WINDOW）。

## [0.3.0] - 2026-08-09

### Added

-会话范围内的进程授权放宽：一次本会话允许后同一可执行文件不再逐条弹窗确认；跨会话持久范围仍保持精确参数匹配。
-工作台 Context 面板内嵌文件与附件预览；二进制、非图片或超大文件可调用系统默认程序打开，敏感路径保持拒绝。
- Codex app-server 协议 Trace 增强：输出工作目录、可写根、沙箱与审批策略以及请求审批的动作范围，便于定位工作区写审批的边界事实。

### Changed

-审批弹窗收敛为当次允许 / 总是允许（仅当前会话）/ 拒绝三个选项，跨会话持久范围不再从弹窗下发；界面只渲染后端供应的权威选项。
- auto 档会话对工作区内且非敏感路径的写目标走快速放行，越界或敏感写目标失败即拒绝；其余档位保持逐次确认行为。
-联网搜索成功后持久化会话能力，重启恢复不再被应用自写状态误阻断。

### Removed

-移除首启引导向导，首次启动默认直接进入工作区。
-移除手动会话文件夹，会话一律按工作目录自动归档。

### Fixed

-修复 Codex 重启恢复时对运行配置的误拦截，同时继续拒绝非可信项目与其他配置篡改。

## [0.2.0] - 2026-08-06

### Added

- Codex Responses 模型接入原生联网搜索；Responses Lite 模型使用与当前 Codex binary 绑定的兼容目录。

### Fixed

-搜索成功后会持久化当前会话能力；运行配置重启校验不再把 Codex 自己写入的 UI 状态误报为篡改。

## [0.1.1] - 2026-08-04

### Added

-工作台加入真实轮次阶段和运行活动展示。
-加入 Helm 内置命令动作，以及 Claude Code 和 Codex Skills 的原生触发方式。
-长会话时间线支持按窗口加载历史内容。

### Changed

- Codex 后续轮次复用 Session-owned app-server 的原生 thread，并通过 turn/start 延续上下文。
- Codex API 使用持久且不保存密钥的运行配置，发送消息前完成 Windows 沙箱就绪检查；界面会区分准备运行环境、配置沙箱和长时间无活动。
-流式回复完成后再启用完整 Markdown 渲染，降低长回复期间的渲染开销。
-权限策略统一在设置页管理。
-普通 Codex 会话按真实 app-server 协议握手判断兼容性，不再要求用户维护精确 CLI 版本；保护模式继续严格验证。

### Fixed

-修复新会话多轮上下文、预算硬停、发送前持久化和 OAuth 状态误判问题。
-改进进程清理、钥匙串清理、审批恢复和工作区隔离的失败处理。
- Codex 未完成命令/文件 item 现在以失败终态收口，不再误报成功；Windows 带空格 executable 的审批 matcher 保留完整路径。
-修复 Codex 停止后后台轮次仍占用工作目录，以及等待审批时停止被误报为工具未完成的问题。
-改进键盘可达性、响应式布局、错误状态和设置保存行为。

## [0.1.0] - 2026-07-09

### Added

- Tauri 2 + React 桌面应用基础结构。
- Claude Code 与 Codex 真实 CLI 会话、流式事件和历史持久化。
- Provider、模型、绑定、扩展、用量、设置和更新基础链路。

[0.6.2]: ./
[0.6.1]: ./
[0.6.0]: ./
[0.5.0]: ./
[0.4.0]: ./
[0.3.0]: ./
[0.2.0]: ./
[0.1.1]: ./
[0.1.0]: ./