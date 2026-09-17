import type { PlanStep } from './events';

export type TurnPresentation = {
  turnId: string;
  eventSeq: number;
  ts: number;
  endedAt?: number;
  reverted?: boolean;
} & (
  | { kind: 'thinking'; text: string; complete: boolean }
  | { kind: 'message'; role: 'user' | 'assistant'; text: string; complete: boolean }
  | { kind: 'plan'; steps: PlanStep[]; truncated?: boolean }
);
