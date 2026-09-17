import { describe, expect, it } from 'vitest';
import type { TurnPresentation } from '@helm/protocol';
import type { SessionDetail, SessionTurn } from '../sessions/api';
import {
  itemsFromHistory,
  reduceSessionAction,
  resetSessionState,
  type SessionState,
} from './useSession';

function history(overrides: Partial<SessionDetail> = {}): SessionDetail {
  return {
    id: 'history',
    cliSessionId: 'native',
    title: 'history',
    engine: 'codex',
    model: 'model',
    cwd: '.',
    status: 'done',
    messageCount: 2,
    inputTokens: 0,
    outputTokens: 0,
    costUsd: 0,
    createdAt: 1,
    updatedAt: 2,
    messages: [
      { role: 'user', text: 'request', ts: 100, turnId: 'turn' },
      { role: 'assistant', text: 'completed reply', ts: 500, turnId: 'turn' },
    ],
    toolCalls: [
      {
        id: 'tool',
        name: 'Read',
        input: {},
        status: 'success',
        ts: 300,
        endedAt: 400,
        turnId: 'turn',
      },
    ],
    approvals: [],
    turns: [
      {
        id: 'turn',
        epoch: 1,
        status: 'failed',
        mode: 'build',
        permissionProfile: 'standard',
        startedAt: 100,
        endedAt: 800,
      },
    ],
    ...overrides,
  };
}

describe('TurnPresentation history projection', () => {
  it('interleaves public presentations by timestamp and event sequence, retaining only the latest plan', () => {
    const presentations: TurnPresentation[] = [
      {
        kind: 'message',
        role: 'assistant',
        text: 'partial reply',
        complete: false,
        turnId: 'turn',
        eventSeq: 12,
        ts: 650,
      },
      {
        kind: 'plan',
        steps: [{ text: 'latest plan', status: 'done' }],
        turnId: 'turn',
        eventSeq: 10,
        ts: 600,
      },
      {
        kind: 'thinking',
        text: 'partial thought',
        complete: false,
        turnId: 'turn',
        eventSeq: 11,
        ts: 650,
      },
      {
        kind: 'thinking',
        text: 'completed thought',
        complete: true,
        turnId: 'turn',
        eventSeq: 3,
        ts: 200,
        endedAt: 260,
      },
      {
        kind: 'plan',
        steps: [{ text: 'old plan', status: 'active' }],
        turnId: 'turn',
        eventSeq: 4,
        ts: 210,
      },
    ];
    const originalOrder = [...presentations];
    const items = itemsFromHistory(history({ presentations }));
    expect(items.map((item) => item.kind)).toEqual([
      'user',
      'thinking',
      'tool',
      'assistant',
      'plan',
      'thinking',
      'assistant',
    ]);
    expect(items[1]).toMatchObject({
      kind: 'thinking',
      text: 'completed thought',
      done: true,
      startedAt: 200,
      endedAt: 260,
      turnStatus: 'failed',
    });
    expect(items[4]).toMatchObject({
      kind: 'plan',
      id: 'plan-turn',
      steps: [{ text: 'latest plan', status: 'done' }],
      turnStatus: 'failed',
    });
    expect(items[5]).toMatchObject({
      kind: 'thinking',
      text: 'partial thought',
      done: true,
      endedAt: 800,
      turnStatus: 'failed',
    });
    expect(items[6]).toMatchObject({
      kind: 'assistant',
      text: 'partial reply',
      turnId: 'turn',
      turnStatus: 'failed',
    });
    expect(presentations).toEqual(originalOrder);
  });

  it('selects the newest plan by turn-local sequence even if the clock moves backwards', () => {
    const detail = history({
      presentations: [
        {
          kind: 'plan',
          steps: [{ text: 'older', status: 'pending' }],
          turnId: 'turn',
          eventSeq: 1,
          ts: 400,
        },
        {
          kind: 'plan',
          steps: [{ text: 'newer', status: 'active' }],
          turnId: 'turn',
          eventSeq: 2,
          ts: 300,
        },
        {
          kind: 'plan',
          steps: [{ text: 'other', status: 'pending' }],
          turnId: 'other-turn',
          eventSeq: 1,
          ts: 600,
        },
      ],
    });
    const plans = itemsFromHistory(detail).filter((item) => item.kind === 'plan');
    expect(plans).toHaveLength(2);
    expect(plans[0]).toMatchObject({
      id: 'plan-turn',
      steps: [{ text: 'newer', status: 'active' }],
    });
    expect(plans[1]).toMatchObject({
      id: 'plan-other-turn',
      steps: [{ text: 'other', status: 'pending' }],
    });
  });

  it('deduplicates replayed event identities, not equal text from different messages or turns', () => {
    const partial: TurnPresentation = {
      kind: 'message',
      role: 'assistant',
      text: 'completed reply',
      complete: false,
      turnId: 'turn',
      eventSeq: 10,
      ts: 650,
    };
    const detail = history({
      presentations: [partial, { ...partial }, { ...partial, turnId: 'other-turn' }],
    });
    const replies = itemsFromHistory(detail).filter((item) => item.kind === 'assistant');
    expect(replies).toHaveLength(3);
    expect(new Set(replies.map((item) => item.id)).size).toBe(3);
    expect(replies.map((item) => item.text)).toEqual([
      'completed reply',
      'completed reply',
      'completed reply',
    ]);
  });

  it.each(['succeeded', 'failed', 'interrupted'] as const)(
    'restores partial text as non-streaming with the real %s turn status',
    (status) => {
      const detail = history({
        turns: [
          {
            id: 'turn',
            epoch: 1,
            status,
            mode: 'build',
            permissionProfile: 'standard',
            startedAt: 100,
            endedAt: 800,
          },
        ],
        presentations: [
          {
            kind: 'thinking',
            text: 'public partial',
            complete: false,
            turnId: 'turn',
            eventSeq: 8,
            ts: 700,
          },
          {
            kind: 'message',
            role: 'assistant',
            text: 'partial reply',
            complete: false,
            turnId: 'turn',
            eventSeq: 9,
            ts: 700,
          },
        ],
      });
      const initial = resetSessionState({} as SessionState, { engine: 'codex', cwd: '.' });
      const restored = reduceSessionAction(initial, {
        type: 'resume_history',
        historyId: 'history',
        detail,
      });
      const resumed = reduceSessionAction(restored, {
        type: 'resume_handle',
        handleId: 'handle',
        historyId: 'history',
        working: false,
        detail,
      });
      expect(resumed).toMatchObject({
        status: 'idle',
        openAssistantId: null,
        openThinkingId: null,
      });
      expect(resumed.items.filter((item) => item.kind === 'assistant')).toHaveLength(2);
      expect(resumed.items.find((item) => item.kind === 'thinking')).toMatchObject({
        done: true,
        text: 'public partial',
        turnStatus: status,
      });
      const reply = resumed.items.at(-1);
      expect(reply).toMatchObject({ kind: 'assistant', text: 'partial reply', turnStatus: status });
      expect(reply && 'interrupted' in reply ? reply.interrupted : false).toBe(
        status === 'interrupted',
      );
      expect(resumed.items.map((item) => item.id)).toEqual(restored.items.map((item) => item.id));
    },
  );

  it('propagates reverted and truncated evidence without manufacturing plan steps', () => {
    const steps = [{ text: 'real prefix', status: 'active' as const }];
    const detail = history({
      presentations: [
        {
          kind: 'plan',
          steps,
          truncated: true,
          reverted: true,
          turnId: 'turn',
          eventSeq: 5,
          ts: 500,
        },
        {
          kind: 'thinking',
          text: 'reverted thought',
          complete: true,
          reverted: true,
          turnId: 'turn',
          eventSeq: 6,
          ts: 600,
        },
        {
          kind: 'message',
          role: 'assistant',
          text: 'reverted partial',
          complete: false,
          reverted: true,
          turnId: 'turn',
          eventSeq: 7,
          ts: 700,
        },
      ],
    });
    const items = itemsFromHistory(detail);
    const plan = items.find((item) => item.kind === 'plan');
    expect(plan).toMatchObject({ reverted: true, truncated: true, steps });
    expect(plan?.kind === 'plan' && plan.steps).toBe(steps);
    expect(items.filter((item) => 'reverted' in item && item.reverted)).toHaveLength(3);
  });

  it('does not invent a terminal status for a turn without terminal facts', () => {
    const turn: SessionTurn = {
      id: 'turn',
      epoch: 1,
      status: 'running',
      mode: 'build',
      permissionProfile: 'standard',
      startedAt: 100,
    };
    const items = itemsFromHistory(
      history({
        turns: [turn],
        presentations: [
          {
            kind: 'thinking',
            text: 'completed public summary',
            complete: true,
            turnId: 'turn',
            eventSeq: 2,
            ts: 200,
          },
        ],
      }),
    );
    const thought = items.find((item) => item.kind === 'thinking');
    expect(thought).toMatchObject({ done: true, endedAt: 200 });
    expect(thought?.turnStatus).toBeUndefined();
  });
});
