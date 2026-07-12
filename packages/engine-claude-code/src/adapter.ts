// Claude Code 引擎适配器：拉起真实 `claude` 子进程，以流式 JSON 模式通信，
// 逐行读 stdout 解析为 AgentEvent，通过 events 事件流推给调用方（probe / 未来的 Tauri 桥）。
//
// 驱动方式（已用真实 claude 2.1.x 验证）：
//   claude -p --input-format stream-json --output-format stream-json --verbose
//          --include-partial-messages [--model ...] [--resume ...] [--allowedTools ...]
// 用户消息以 JSONL 写入 stdin；stdout 每行一个事件，--include-partial-messages 提供逐字增量。

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { createInterface } from 'node:readline';
import type { AgentEvent, Decision, EngineAdapter, EngineId, SessionHandle } from '@helm/protocol';
import { parseClaudeLine } from './parse';

export interface ClaudeSessionHandle extends SessionHandle {
  /** 事件流：'event' 携带 AgentEvent，'close' 携带退出码。 */
  readonly events: EventEmitter;
  readonly child: ChildProcessWithoutNullStreams;
}

export interface ClaudeCodeAdapterOptions {
  /** claude 可执行文件，默认走 PATH 上的 `claude`。 */
  bin?: string;
  /**
   * 预授权可自动执行的工具白名单（如只读的 Glob/Read/LS）。
   * 仅为让无人值守的 demo 不卡在审批；正式审批策略在 Slice 2 实现。
   */
  allowedTools?: string[];
}

export class ClaudeCodeAdapter implements EngineAdapter {
  readonly id: EngineId = 'claude-code';
  private readonly bin: string;
  private readonly allowedTools: string[] | undefined;

  constructor(options: ClaudeCodeAdapterOptions = {}) {
    this.bin = options.bin ?? 'claude';
    this.allowedTools = options.allowedTools;
  }

  parseLine(raw: string): AgentEvent[] {
    return parseClaudeLine(raw);
  }

  start(opts: { model: string; cwd: string; resumeId?: string }): Promise<SessionHandle> {
    const args = [
      '-p',
      '--input-format',
      'stream-json',
      '--output-format',
      'stream-json',
      '--verbose',
      '--include-partial-messages',
    ];
    if (opts.model) args.push('--model', opts.model);
    if (opts.resumeId) args.push('--resume', opts.resumeId);
    if (this.allowedTools && this.allowedTools.length > 0) {
      args.push('--allowedTools', this.allowedTools.join(','));
    }

    // Windows 上 npm 安装的 claude 是 .cmd 包装器，需经 shell 解析 PATH。
    const useShell = process.platform === 'win32';
    const child = spawn(this.bin, args, { cwd: opts.cwd, env: process.env, shell: useShell });

    const events = new EventEmitter();
    const handle: ClaudeSessionHandle = { sessionId: '', engine: this.id, events, child };

    const rl = createInterface({ input: child.stdout });
    rl.on('line', (line) => {
      for (const event of parseClaudeLine(line)) {
        if (event.type === 'session_started' && event.sessionId) {
          handle.sessionId = event.sessionId;
        }
        events.emit('event', event);
      }
    });

    let stderrBuf = '';
    child.stderr.on('data', (chunk: Buffer) => {
      stderrBuf += chunk.toString('utf8');
    });

    child.on('error', (err) => {
      const event: AgentEvent = {
        type: 'error',
        message: `无法启动 claude 进程：${err.message}`,
        recoverable: false,
      };
      events.emit('event', event);
    });

    child.on('close', (code) => {
      if (code !== null && code !== 0) {
        const detail = stderrBuf.trim();
        const event: AgentEvent = {
          type: 'error',
          ...(handle.sessionId ? { sessionId: handle.sessionId } : {}),
          message: `claude 进程异常退出（code=${code}）${detail ? `：${detail}` : ''}`,
          recoverable: false,
        };
        events.emit('event', event);
      }
      events.emit('close', code ?? 0);
    });

    return Promise.resolve(handle);
  }

  send(handle: SessionHandle, text: string, _attachments?: string[]): void {
    const child = (handle as ClaudeSessionHandle).child;
    const message = {
      type: 'user',
      message: { role: 'user', content: [{ type: 'text', text }] },
    };
    child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  // 审批回写依赖 control 消息协议，在 Slice 2（工具调用 + 审批）实现。
  approve(_handle: SessionHandle, _requestId: string, _decision: Decision): void {
    throw new Error('approve 尚未实现：审批策略属于 Slice 2');
  }

  interrupt(handle: SessionHandle): void {
    (handle as ClaudeSessionHandle).child.kill('SIGTERM');
  }

  /** 结束本轮输入（让单轮 demo 的进程能正常收尾退出）。 */
  endInput(handle: SessionHandle): void {
    (handle as ClaudeSessionHandle).child.stdin.end();
  }

  stop(handle: SessionHandle): Promise<void> {
    const h = handle as ClaudeSessionHandle;
    return new Promise((resolve) => {
      h.events.once('close', () => resolve());
      try {
        h.child.stdin.end();
      } catch {
        // 忽略：stdin 可能已关闭。
      }
      h.child.kill('SIGTERM');
    });
  }
}
