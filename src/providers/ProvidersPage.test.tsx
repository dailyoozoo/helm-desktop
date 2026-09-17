import { setImmediate } from 'node:timers';
import { isValidElement, useEffect, useState, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReasoningEffortCapability } from '@helm/protocol';
import { useReasoningEffortCapability } from '../engine/useReasoningEffortCapability';
import { AddProviderModal, EngineDetailPanel, ProviderDrawer } from './ProvidersPage';
import {
  loginCliAccount,
  saveBindingConfig,
  saveProviderConfig,
  syncProviderModels,
  type AppConfig,
} from './api';
import { createProviderDraft } from './providerViewModel';

vi.mock('react', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react')>();
  return { ...actual, useState: vi.fn(actual.useState), useEffect: vi.fn(actual.useEffect) };
});
vi.mock('../engine/useReasoningEffortCapability', () => ({
  useReasoningEffortCapability: vi.fn(),
}));
vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./api')>()),
  loginCliAccount: vi.fn(),
  saveBindingConfig: vi.fn(),
  saveProviderConfig: vi.fn(),
  syncProviderModels: vi.fn(),
}));

const supported: ReasoningEffortCapability = {
  support: 'supported',
  options: ['auto', 'low', 'high'],
  defaultEffort: 'high',
  source: 'engine-probe',
};

function configuration(
  template: 'codex-subscription' | 'claude-subscription' = 'codex-subscription',
): AppConfig {
  const provider = { ...createProviderDraft(template, 1), ready: true };
  const engineId = template === 'claude-subscription' ? 'claude-code' : 'codex';
  return {
    defaultEngine: engineId,
    defaultModel: 'official-primary',
    providers: [provider],
    models: [
      {
        id: 'official-primary',
        providerId: provider.id,
        displayName: 'Official Primary',
        enabled: true,
        inputPricePerMtok: 2,
        cachedInputPricePerMtok: 0.2,
        outputPricePerMtok: 8,
        priceSource: 'builtin',
      },
      {
        id: 'official-fast',
        providerId: provider.id,
        displayName: 'Official Fast',
        enabled: false,
        inputPricePerMtok: 1,
        outputPricePerMtok: 4,
        priceSource: 'builtin',
      },
    ],
    engines: [
      {
        id: engineId,
        name: engineId === 'codex' ? 'Codex' : 'Claude Code',
        bin: engineId === 'codex' ? 'codex' : 'claude',
        defaultModel: 'official-primary',
        status: 'ready',
        version: null,
      },
    ],
    bindings: [
      {
        engineId,
        providerId: provider.id,
        primaryModel: 'official-primary',
        fastModel: null,
        reasoningEffort: 'auto',
      },
    ],
  };
}

type ElementProps = {
  children?: ReactNode;
  onClick?: () => void;
  onChange?: (event: { target: { value: string } }) => void;
  value?: string;
  type?: string;
  disabled?: boolean;
  checked?: boolean;
  readOnly?: boolean;
  'aria-label'?: string;
};

function elements(node: ReactNode): ReactElement<ElementProps>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<ElementProps>(node)) return [];
  return [node, ...elements(node.props.children)];
}

function textOf(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  return isValidElement<ElementProps>(node) ? textOf(node.props.children) : '';
}

function button(tree: ReactNode, text: string): ReactElement<ElementProps> {
  const found = elements(tree).find(
    (element) => element.type === 'button' && textOf(element).trim() === text,
  );
  if (!found) throw new Error(`Button not found: ${text}`);
  return found;
}

function modelToggle(tree: ReactNode, modelId: string): ReactElement<ElementProps> {
  const found = elements(tree).find((element) => element.props['aria-label'] === `启用 ${modelId}`);
  if (!found) throw new Error(`Model toggle not found: ${modelId}`);
  return found;
}

function renderStatefully(render: () => ReactElement) {
  const values: unknown[] = [];
  let index = 0;
  vi.mocked(useState).mockImplementation(((initialValue: unknown) => {
    const at = index++;
    if (at === values.length) {
      values.push(typeof initialValue === 'function' ? initialValue() : initialValue);
    }
    return [
      values[at],
      (next: unknown) => {
        values[at] = typeof next === 'function' ? next(values[at]) : next;
      },
    ];
  }) as typeof useState);
  vi.mocked(useEffect).mockImplementation(() => undefined);
  return () => {
    index = 0;
    return render();
  };
}

function settleRequests(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

beforeEach(async () => {
  const actualReact = await vi.importActual<typeof import('react')>('react');
  vi.mocked(useState).mockImplementation(actualReact.useState);
  vi.mocked(useEffect).mockImplementation(actualReact.useEffect);
  vi.mocked(useReasoningEffortCapability).mockReturnValue({
    capability: supported,
    loading: false,
    error: null,
  });
  vi.mocked(saveBindingConfig).mockReset().mockResolvedValue(configuration());
  vi.mocked(saveProviderConfig).mockReset().mockResolvedValue(configuration());
  vi.mocked(syncProviderModels).mockReset().mockResolvedValue(configuration());
  vi.mocked(loginCliAccount).mockReset().mockResolvedValue({
    state: 'ok',
    authMethod: 'subscription',
    detail: 'signed in',
  });
});

afterEach(() => vi.clearAllMocks());

describe('C08 引擎偏好使用真实能力', () => {
  it('旧档位不在新选项时显示不可用的原值，不伪装成自动，也不自动保存', () => {
    const config = configuration();
    config.bindings[0].reasoningEffort = 'max';
    const markup = renderToStaticMarkup(
      <EngineDetailPanel
        config={config}
        engine={config.engines[0]}
        onConfig={vi.fn()}
        onNotice={vi.fn()}
      />,
    );
    expect(markup).toMatch(
      /<option(?=[^>]*value="max")(?=[^>]*disabled="")(?=[^>]*selected="")[^>]*>最大（当前不可用）<\/option>/,
    );
    expect(markup).toContain('不会自动改写偏好');
    expect(markup).toContain('value="low"');
    expect(markup).toContain('value="high"');
    expect(markup).not.toContain('value="medium"');
    expect(saveBindingConfig).not.toHaveBeenCalled();
  });

  it('auto 保持模型默认，不擅自替换为 capability 的默认档位', () => {
    const config = configuration();
    const markup = renderToStaticMarkup(
      <EngineDetailPanel
        config={config}
        engine={config.engines[0]}
        onConfig={vi.fn()}
        onNotice={vi.fn()}
      />,
    );
    expect(markup).toMatch(/<option(?=[^>]*value="auto")(?=[^>]*selected="")[^>]*>自动<\/option>/);
    expect(saveBindingConfig).not.toHaveBeenCalled();
  });

  it.each(['loading', 'unknown', 'unsupported', 'error'] as const)(
    '%s 不提供未经证实的档位，保留旧值提示',
    (state) => {
      const config = configuration();
      config.bindings[0].reasoningEffort = 'high';
      vi.mocked(useReasoningEffortCapability).mockReturnValue({
        capability:
          state === 'unknown' || state === 'unsupported' ? { ...supported, support: state } : null,
        loading: state === 'loading',
        error: state === 'error' ? 'probe unavailable' : null,
      });
      const markup = renderToStaticMarkup(
        <EngineDetailPanel
          config={config}
          engine={config.engines[0]}
          onConfig={vi.fn()}
          onNotice={vi.fn()}
        />,
      );
      expect(markup).toMatch(
        /<option(?=[^>]*value="high")(?=[^>]*disabled="")(?=[^>]*selected="")/,
      );
      expect(markup).not.toContain('value="low"');
      if (state === 'loading') {
        expect(markup).toMatch(/<select(?=[^>]*aria-label="默认推理强度")(?=[^>]*disabled="")/);
      }
      if (state === 'error') expect(markup).toContain('probe unavailable');
      expect(saveBindingConfig).not.toHaveBeenCalled();
    },
  );

  it('无合同开关始终禁用；用户重新选择有效 effort 后一次保存真实偏好', async () => {
    const config = configuration('claude-subscription');
    config.bindings[0].reasoningEffort = 'max';
    config.bindings[0].thinkingEnabled = true;
    config.bindings[0].context1m = true;
    const read = renderStatefully(() =>
      EngineDetailPanel({
        config,
        engine: config.engines[0],
        onConfig: vi.fn(),
        onNotice: vi.fn(),
      }),
    );
    const tree = read();
    const inert = elements(tree).filter(
      (element) =>
        element.type === 'input' &&
        element.props.type === 'checkbox' &&
        element.props.disabled &&
        element.props.readOnly,
    );
    expect(inert).toHaveLength(2);
    expect(
      inert.every((element) => element.props.checked === false && !element.props.onChange),
    ).toBe(true);
    const select = elements(tree).find((element) => element.props['aria-label'] === '默认推理强度');
    select?.props.onChange?.({ target: { value: 'low' } });
    await settleRequests();
    expect(saveBindingConfig).toHaveBeenCalledTimes(1);
    expect(saveBindingConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        reasoningEffort: 'low',
        thinkingEnabled: false,
        context1m: false,
      }),
    );
    expect(config.bindings[0].reasoningEffort).toBe('max');
  });
});

describe('C04 订阅目录只读与原子勾选', () => {
  it.each(['codex-subscription', 'claude-subscription'] as const)(
    '%s 只允许勾选官方模型，最后选择只发一条保存命令',
    async (template) => {
      const config = configuration(template);
      const onConfig = vi.fn();
      let complete!: (saved: AppConfig) => void;
      vi.mocked(saveProviderConfig).mockReturnValue(new Promise((resolve) => (complete = resolve)));
      const read = renderStatefully(() =>
        ProviderDrawer({
          config,
          activeProvider: config.providers[0],
          onConfig,
          onNotice: vi.fn(),
          onClose: vi.fn(),
          onUnbindJump: vi.fn(),
        }),
      );
      const tree = read();
      expect(textOf(tree)).not.toContain('添加模型');
      expect(elements(tree).some((element) => element.props['aria-label'] === '移除模型')).toBe(
        false,
      );
      expect(
        elements(tree).some(
          (element) =>
            element.type === 'input' &&
            config.models.some((model) => model.id === element.props.value),
        ),
      ).toBe(false);
      for (const modelId of [
        'official-primary',
        'official-fast',
        'official-primary',
        'official-primary',
      ]) {
        modelToggle(read(), modelId).props.onChange?.({ target: { value: '' } });
      }
      button(read(), '保存修改').props.onClick?.();
      const savingTree = read();
      expect(button(savingTree, '保存修改').props.disabled).toBe(true);
      expect(modelToggle(savingTree, 'official-primary').props.disabled).toBe(true);
      button(savingTree, '保存修改').props.onClick?.();
      const savedModels = config.models.map((model) => ({
        ...model,
        enabled: model.id === 'official-fast',
      }));
      expect(saveProviderConfig).toHaveBeenCalledTimes(1);
      expect(saveProviderConfig).toHaveBeenCalledWith(
        config.providers[0],
        undefined,
        savedModels,
        [],
      );
      const saved = {
        ...config,
        models: savedModels,
        providers: [{ ...config.providers[0], name: 'Authoritative Name' }],
      };
      complete(saved);
      await settleRequests();
      expect(onConfig).toHaveBeenCalledWith(saved);
      expect(button(read(), '保存修改').props.disabled).toBe(false);
      expect(
        elements(read()).some(
          (element) => element.type === 'input' && element.props.value === 'Authoritative Name',
        ),
      ).toBe(true);
      expect(config.models[0].enabled).toBe(true);
    },
  );

  it('同步保留尚未保存的勾选，但目录字段始终取最新官方快照', async () => {
    const config = configuration();
    const refreshed = {
      ...config,
      models: config.models.map((model) => ({ ...model, inputPricePerMtok: 5 })),
    };
    vi.mocked(syncProviderModels).mockResolvedValue(refreshed);
    const read = renderStatefully(() =>
      ProviderDrawer({
        config,
        activeProvider: config.providers[0],
        onConfig: vi.fn(),
        onNotice: vi.fn(),
        onClose: vi.fn(),
        onUnbindJump: vi.fn(),
      }),
    );
    modelToggle(read(), 'official-primary').props.onChange?.({ target: { value: '' } });
    button(read(), '同步模型').props.onClick?.();
    expect(button(read(), '保存修改').props.disabled).toBe(true);
    await settleRequests();
    expect(modelToggle(read(), 'official-primary').props.checked).toBe(false);
    button(read(), '保存修改').props.onClick?.();
    expect(saveProviderConfig).toHaveBeenCalledWith(
      config.providers[0],
      undefined,
      refreshed.models.map((model) => ({ ...model, enabled: false })),
      [],
    );
    await settleRequests();
  });

  it('空订阅目录不能手填；失败保存保留用户勾选并允许重试', async () => {
    const config = configuration();
    const notice = vi.fn();
    vi.mocked(saveProviderConfig).mockRejectedValue(new Error('official catalog changed'));
    const read = renderStatefully(() =>
      ProviderDrawer({
        config,
        activeProvider: config.providers[0],
        onConfig: vi.fn(),
        onNotice: notice,
        onClose: vi.fn(),
        onUnbindJump: vi.fn(),
      }),
    );
    modelToggle(read(), 'official-fast').props.onChange?.({ target: { value: '' } });
    button(read(), '保存修改').props.onClick?.();
    await settleRequests();
    expect(modelToggle(read(), 'official-fast').props.checked).toBe(true);
    expect(button(read(), '保存修改').props.disabled).toBe(false);
    expect(notice).toHaveBeenCalledWith('official catalog changed');
    const emptyConfig = { ...config, models: [] };
    const empty = renderStatefully(() =>
      ProviderDrawer({
        config: emptyConfig,
        activeProvider: config.providers[0],
        onConfig: vi.fn(),
        onNotice: vi.fn(),
        onClose: vi.fn(),
        onUnbindJump: vi.fn(),
      }),
    )();
    expect(textOf(empty)).toContain('请先登录并同步官方目录');
    expect(textOf(empty)).not.toContain('添加模型');
  });

  it.each(['codex-subscription', 'claude-subscription'] as const)(
    '添加 %s 登录后进入详情选择，不提交前端编造的目录',
    async (template) => {
      const config = configuration(template);
      vi.mocked(saveProviderConfig).mockImplementation(async (provider) => ({
        ...config,
        providers: [provider],
        models: [],
      }));
      vi.mocked(syncProviderModels).mockImplementation(async (providerId) => ({
        ...config,
        providers: [{ ...config.providers[0], id: providerId }],
        models: config.models.map((model) => ({ ...model, providerId })),
      }));
      const onOpenExisting = vi.fn();
      const read = renderStatefully(() =>
        AddProviderModal({
          providers: [],
          onConfig: vi.fn(),
          onNotice: vi.fn(),
          onClose: vi.fn(),
          onOpenExisting,
        }),
      );
      const card = elements(read()).find((element) => element.key === template);
      expect(card).toBeDefined();
      card?.props.onClick?.();
      button(read(), '前往登录').props.onClick?.();
      await settleRequests();
      expect(loginCliAccount).toHaveBeenCalledWith(
        template === 'claude-subscription' ? 'claude-code' : 'codex',
      );
      expect(saveProviderConfig).toHaveBeenCalledTimes(1);
      const args = vi.mocked(saveProviderConfig).mock.calls[0];
      expect(args[0].kind).toBe('subscription');
      expect(args[0].baseUrl).toBe('');
      expect(args[1]).toBeUndefined();
      expect(args[2]).toBeUndefined();
      expect(syncProviderModels).toHaveBeenCalledTimes(1);
      expect(syncProviderModels).toHaveBeenCalledWith(args[0].id);
      expect(onOpenExisting).toHaveBeenCalledWith(args[0].id);
    },
  );

  it('登录结果未确认订阅身份时不创建服务商或声称登录成功', async () => {
    vi.mocked(loginCliAccount).mockResolvedValue({
      state: 'missing',
      authMethod: 'unknown',
      detail: 'login canceled',
    });
    const notice = vi.fn();
    const onOpenExisting = vi.fn();
    const read = renderStatefully(() =>
      AddProviderModal({
        providers: [],
        onConfig: vi.fn(),
        onNotice: notice,
        onClose: vi.fn(),
        onOpenExisting,
      }),
    );
    elements(read())
      .find((element) => element.key === 'codex-subscription')
      ?.props.onClick?.();
    button(read(), '前往登录').props.onClick?.();
    await settleRequests();
    expect(saveProviderConfig).not.toHaveBeenCalled();
    expect(onOpenExisting).not.toHaveBeenCalled();
    expect(notice).toHaveBeenCalledWith('login canceled');
  });

  it('登录成功但目录同步失败时进入已创建的详情，说明真实状态而不是假报登录失败', async () => {
    const config = configuration();
    const notice = vi.fn();
    const onOpenExisting = vi.fn();
    vi.mocked(saveProviderConfig).mockImplementation(async (provider) => ({
      ...config,
      providers: [provider],
      models: [],
    }));
    vi.mocked(syncProviderModels).mockRejectedValue(new Error('catalog unavailable'));
    const read = renderStatefully(() =>
      AddProviderModal({
        providers: [],
        onConfig: vi.fn(),
        onNotice: notice,
        onClose: vi.fn(),
        onOpenExisting,
      }),
    );
    elements(read())
      .find((element) => element.key === 'codex-subscription')
      ?.props.onClick?.();
    button(read(), '前往登录').props.onClick?.();
    await settleRequests();
    expect(onOpenExisting).toHaveBeenCalledWith(vi.mocked(saveProviderConfig).mock.calls[0][0].id);
    expect(notice).toHaveBeenCalledWith('登录已完成，但模型同步失败：catalog unavailable');
    expect(notice).not.toHaveBeenCalledWith('登录未完成');
  });
});
