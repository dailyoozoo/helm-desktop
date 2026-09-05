import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { useRef } from 'react';
import type { ThreadItem } from '../engine/useSession';
import type { SessionTurn } from '../sessions/api';
import { ThreadTurnRail, firstQuestionByTurn, turnRailMarkers } from './ThreadTurnRail';

function turn(id: string, status: SessionTurn['status'] = 'succeeded'): SessionTurn {
  return {
    id,
    epoch: 1,
    mode: 'build',
    permissionProfile: 'standard',
    status,
    startedAt: 1_767_000_000_000,
  };
}

function user(id: string, turnId: string, text: string): ThreadItem {
  return { kind: 'user', id, text, turnId };
}

function Harness({ turns, items }: { turns: SessionTurn[]; items: ThreadItem[] }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div ref={ref}>
      <ThreadTurnRail scrollRef={ref} turns={turns} items={items} />
    </div>
  );
}

describe('turnRailMarkers', () => {
  it('空 ledger 返回空数组', () => {
    expect(turnRailMarkers(null)).toEqual([]);
    expect(turnRailMarkers([])).toEqual([]);
  });

  it('按顺序从 1 编号并保留状态与开始时间', () => {
    const markers = turnRailMarkers([turn('t1', 'failed'), turn('t2')]);
    expect(markers).toHaveLength(2);
    expect(markers[0]).toMatchObject({ turnId: 't1', index: 1, status: 'failed' });
    expect(markers[1]).toMatchObject({ turnId: 't2', index: 2, status: 'succeeded' });
  });
});

describe('firstQuestionByTurn', () => {
  it('每个轮次取第一条用户提问', () => {
    const map = firstQuestionByTurn([
      user('i1', 't1', '第一问'),
      user('i2', 't1', '重复'),
      user('i3', 't2', '第二问'),
    ]);
    expect(map.get('t1')).toBe('第一问');
    expect(map.get('t2')).toBe('第二问');
  });
});

describe('ThreadTurnRail', () => {
  it('≤3 轮不渲染（原型 is-on 门禁）', () => {
    const html = renderToStaticMarkup(
      <Harness turns={[turn('t1'), turn('t2'), turn('t3')]} items={[]} />,
    );
    expect(html).not.toContain('ws-turnrail');
  });

  it('>3 轮渲染：is-on 容器 + 每轮一条短横线刻度（对齐原型 .turn-rail）', () => {
    const turns = [turn('t1'), turn('t2', 'failed'), turn('t3'), turn('t4')];
    const items = [user('i1', 't1', '上海今天的天气怎么样'), user('i2', 't2', '第二问')];
    const html = renderToStaticMarkup(<Harness turns={turns} items={items} />);
    expect(html).toContain('ws-turnrail is-on');
    expect(html).toContain('ws-turnrail__zone');
    expect(html.match(/ws-turnrail__item/g)).toHaveLength(4);
    expect(html).toContain('ws-turnrail__line');
    expect(html).toContain('第 1 轮');
    expect(html).toContain('失败');
    // 「回到最新」已迁出轨道（D-10b 用户裁决 B 形态：底部中央渐隐浮层 .ws-jumplatest）
    expect(html).not.toContain('ws-turnrail__latest');
    expect(html).not.toContain('回到最新');
    // 刻度线不着色、状态只进 title/气泡（对齐原型，无 is-fail 刻度类）
    expect(html).not.toContain('ws-turnrail__tick');
  });
});
