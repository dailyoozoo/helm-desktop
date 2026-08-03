import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Composer, settleQueuedMessage } from './Composer';

describe('Composer', () => {
  it('自动发送失败时保留消息和附件，成功后才移出队列', () => {
    const first = { text: '继续修复', attachments: ['D:/work/log.txt'] };
    const second = { text: '再跑测试', attachments: [] };
    const queue = [first, second];

    expect(settleQueuedMessage(queue, first, false)).toEqual({ queue, paused: true });
    expect(settleQueuedMessage(queue, first, true)).toEqual({ queue: [second], paused: false });
  });

  it('常驻展示模型、推理强度、上下文和花费，并解释权限档位', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        mode="build"
        engine="codex"
        model="gpt-5-codex"
        reasoningEffort="high"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
        cost={{
          inputTokens: 100,
          outputTokens: 20,
          contextTokens: 80,
          contextWindow: 100,
          costUsd: 0.1234,
        }}
      />,
    );

    expect(markup).toContain('模型 gpt-5-codex');
    expect(markup).toContain('强度 高');
    expect(markup).toContain('上下文 80%');
    expect(markup).toContain('花费 $0.1234');
    expect(markup).toContain('危险操作会询问');
  });
});
