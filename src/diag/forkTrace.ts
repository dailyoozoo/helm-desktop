/** 分叉→打开链路持久化诊断（2026-09-04，方案 A）。
 *  背景：分叉三度复报「点了不跳转」，后端六关日志全绿，断裂发生在前端
 *  「分叉成功 → dispatchEvent → App 置 pendingSessionId → Workspace 打开会话」
 *  这段事件链上，且该链路此前零持久化日志——断了查不到断在哪一环。
 *
 *  用法：forkTrace('stage', '详情')。fire-and-forget：日志失败静默丢弃，
 *  绝不阻塞分叉/跳转主流程（诊断通道自身故障不能放大成用户可见故障）。
 *  日志经后端 append_runtime_log 落 helm-runtime.log，前缀 [helm-frontend-fork]，
 *  与 [helm-resume] 后端六关日志衔接成完整证据链。
 */
import { appendRuntimeLog } from '../engine/transport';

const FORK_TRACE_PREFIX = '[helm-frontend-fork]';

export function forkTrace(stage: string, detail: string = ''): void {
  const line = detail
    ? `${FORK_TRACE_PREFIX} stage=${stage} ${detail}`
    : `${FORK_TRACE_PREFIX} stage=${stage}`;
  void appendRuntimeLog(line).catch(() => {});
}
