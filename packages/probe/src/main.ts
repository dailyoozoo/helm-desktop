// Slice 0 无界面验收程序：通过 Claude Code 适配器拉起真实 `claude` 进程，
// 把流式 AgentEvent 实时打印到控制台。证明「真连上进程、真流式、真解析成协议事件」。
//
// 用法：
//   npm run probe                      # 默认 prompt：列出当前目录文件
//   npm run probe -- 列出当前目录文件
//   HELM_PROBE_MODEL=claude-haiku-4-5-20251001 npm run probe -- 你好
//
// 注意：会真实调用 claude，消耗额度；这不是 mock。

import { ClaudeCodeAdapter, type ClaudeSessionHandle } from '@helm/engine-claude-code';
import type { AgentEvent } from '@helm/protocol';

const GRAY = '\x1b[90m';
const CYAN = '\x1b[36m';
const GREEN = '\x1b[32m';
const YELLOW = '\x1b[33m';
const RED = '\x1b[31m';
const BOLD = '\x1b[1m';
const RESET = '\x1b[0m';

function render(event: AgentEvent): void {
  switch (event.type) {
    case 'session_started':
      process.stdout.write(
        `${GRAY}── 会话开始 ${event.sessionId} · ${event.engine} · ${event.model || '(默认模型)'} · ${event.cwd}${RESET}\n${CYAN}助手：${RESET}`,
      );
      break;
    case 'message_delta':
      // 逐字增量：直接续写，形成打字机效果。
      process.stdout.write(event.text);
      break;
    case 'message_complete':
      process.stdout.write(`\n${GRAY}（本段助手消息定稿）${RESET}\n`);
      break;
    case 'tool_call':
      process.stdout.write(
        `\n${YELLOW}🔧 工具调用 ${event.name}(${JSON.stringify(event.input)}) [${event.id}]${RESET}\n`,
      );
      break;
    case 'tool_progress':
      process.stdout.write(`${GRAY}${event.chunk}${RESET}`);
      break;
    case 'tool_result': {
      const head = (event.output ?? '').split('\n').slice(0, 6).join('\n');
      process.stdout.write(
        `${GREEN}✓ 工具结果 [${event.id}] (${event.status})${RESET}\n${GRAY}${head}${RESET}\n${CYAN}助手：${RESET}`,
      );
      break;
    }
    case 'approval_request':
      process.stdout.write(`\n${YELLOW}🛂 审批请求：${event.action} — ${event.detail}${RESET}\n`);
      break;
    case 'plan_update':
      process.stdout.write(
        `\n${GRAY}计划：${event.steps.map((s) => `[${s.status}] ${s.text}`).join(' | ')}${RESET}\n`,
      );
      break;
    case 'checkpoint':
      process.stdout.write(`\n${GRAY}⏺ 检查点 ${event.label} [${event.id}]${RESET}\n`);
      break;
    case 'token_usage':
      process.stdout.write(
        `\n${GRAY}── 用量：in=${event.inputTokens} out=${event.outputTokens} cost=$${event.costUsd.toFixed(6)}${RESET}\n`,
      );
      break;
    case 'turn_complete':
      process.stdout.write(`${BOLD}── 轮次结束（stopReason=${event.stopReason}）${RESET}\n`);
      break;
    case 'error':
      process.stdout.write(
        `\n${RED}✗ 错误：${event.message}（recoverable=${event.recoverable}）${RESET}\n`,
      );
      break;
  }
}

async function main(): Promise<void> {
  const prompt =
    process.argv.slice(2).join(' ').trim() || '列出当前目录下的文件，并用一句话总结有几个';
  const model = process.env.HELM_PROBE_MODEL ?? '';
  const cwd = process.cwd();

  process.stdout.write(`${BOLD}Helm probe${RESET} ${GRAY}（真实 claude 进程，非 mock）${RESET}\n`);
  process.stdout.write(`${GRAY}prompt: ${prompt}${RESET}\n\n`);

  // 预授权只读工具，让无人值守的 demo 能真实触发一次工具调用而不卡在审批。
  // HELM_PROBE_BIN 可指向一个不存在的可执行文件，用来验收「改错 CLI 路径会报错」。
  const bin = process.env.HELM_PROBE_BIN;
  const adapter = new ClaudeCodeAdapter({
    ...(bin ? { bin } : {}),
    allowedTools: ['Glob', 'Grep', 'Read', 'LS'],
  });
  const handle = (await adapter.start({ model, cwd })) as ClaudeSessionHandle;

  await new Promise<void>((resolve) => {
    handle.events.on('event', render);
    handle.events.once('close', (code: number) => {
      process.stdout.write(`${GRAY}── 进程退出 code=${code}${RESET}\n`);
      resolve();
    });

    adapter.send(handle, prompt);
    adapter.endInput(handle); // 单轮 demo：发完即结束输入，让进程正常收尾。
  });
}

main().catch((err: unknown) => {
  process.stderr.write(`probe 失败：${err instanceof Error ? err.message : String(err)}\n`);
  process.exitCode = 1;
});
