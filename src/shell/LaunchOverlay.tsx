import { useEffect, useRef, useState } from 'react';
import { Icon } from './icons';
import { engineDisplayName } from '../home/newTaskViewModel';
import { onAgentEvent } from '../engine/transport';
import type { EngineId } from '@helm/protocol';

/**
 * 启动过渡覆盖层（变更-天气/过渡页修复）：提交后常驻于 App 顶层，跨「新任务页 → 工作区」
 * 导航保持可见。进度由真实后端事件驱动——
 *   turn_stage(preparing_runtime/starting_engine) → 正在启动引擎
 *   turn_stage(waiting_model)                     → 正在创建任务会话
 *   session_started                               → 正在打开工作区（随后交给工作区）
 *   error / turn_complete                         → 立即揭开，让用户看到工作区里的真实错误
 * 不再使用固定延时假动画。safetyMs 兜底，防止后端异常时覆盖层卡死。
 *
 * 生命周期由 App 的 launchingEngine 独立持有（2026-08-30）：不能绑 pendingDraft，
 * 那个状态在 Composer 挂载首帧就被消费清空，覆盖层会只闪一帧。
 */
export function LaunchOverlay({
  engine,
  onClear,
  safetyMs = 25000,
}: {
  engine: EngineId;
  onClear: () => void;
  safetyMs?: number;
}) {
  const [phase, setPhase] = useState(0);
  const [done, setDone] = useState(false);
  const clearedRef = useRef(false);

  useEffect(() => {
    const clear = () => {
      if (clearedRef.current) return;
      clearedRef.current = true;
      onClear();
    };
    let unlisten: (() => void) | undefined;
    // 必须走 onAgentEvent：信封字段是 `event`（packages/protocol events.ts AgentEventEnvelope），
    // 早期本地类型误写成 `payload`，导致所有事件被静默吞掉、过渡页只能靠 safetyMs 兜底消失
    // （2026-08-30 实机走查发现）。协议是唯一真值，这里不再自行解包。
    const stop = onAgentEvent((envelope) => {
      const agentEvent = envelope?.event;
      if (!agentEvent) return;
      if (agentEvent.type === 'turn_stage') {
        if (agentEvent.stage === 'preparing_runtime' || agentEvent.stage === 'starting_engine') {
          setPhase((value) => Math.max(value, 1));
        } else if (agentEvent.stage === 'waiting_model') {
          setPhase((value) => Math.max(value, 2));
        }
      } else if (agentEvent.type === 'session_started') {
        setPhase(3);
        setDone(true);
        window.setTimeout(clear, 700);
      } else if (agentEvent.type === 'error' || agentEvent.type === 'turn_complete') {
        // 引擎起不来 / 首轮直接失败：立刻揭开覆盖层，把真实错误交给工作区呈现，
        // 不让用户对着「正在启动任务」干等 safetyMs。
        clear();
      }
    });
    stop.then((unsub) => {
      unlisten = unsub;
    });
    const safety = window.setTimeout(clear, safetyMs);
    return () => {
      unlisten?.();
      window.clearTimeout(safety);
    };
  }, [onClear, safetyMs]);

  const line =
    phase === 0
      ? '正在解析任务与上下文'
      : phase === 1
        ? `正在启动 ${engineDisplayName(engine)}`
        : phase === 2
          ? '正在创建任务会话'
          : '正在打开工作区';

  return (
    // 只挂 launch-overlay：覆盖层不得带 .cm-start（其 width min(800px,100%) + auto
    // 边距会把 fixed 层收成居中 800px 窄带，2026-08-31 用户截图实锤）。几何为
    // 标题栏以下、主侧栏以右（2026-09-01 用户裁决，对齐 WorkBuddy：侧栏/标题栏可见）。
    <div className="launch-overlay">
      <div className={`cm-launch${done ? ' is-done' : ''}`}>
        <div className="cm-launch__titleline">
          <span className="cm-launch__logo" aria-hidden="true">
            <Icon name="helm" />
          </span>
          <h2 className="cm-launch__title">{done ? '任务已触发' : '正在启动任务'}</h2>
        </div>
        <div className="cm-launch__line">{line}</div>
      </div>
    </div>
  );
}
