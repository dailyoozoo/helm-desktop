/** 模型推理强度。`auto` 始终表示使用模型自己的默认档位。 */
export type ReasoningEffort =
  | 'auto'
  | 'none'
  | 'minimal'
  | 'low'
  | 'medium'
  | 'high'
  | 'xhigh'
  | 'max';

export type ReasoningEffortSupport = 'supported' | 'unsupported' | 'unknown';

export interface ReasoningEffortCapability {
  support: ReasoningEffortSupport;
  options: ReasoningEffort[];
  defaultEffort?: Exclude<ReasoningEffort, 'auto'>;
  source: 'engine-probe' | 'builtin-catalog' | 'provider-declared';
}
