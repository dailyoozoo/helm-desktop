import { memo } from 'react';
import { Icon } from '../../shell/icons';

/** 错误分类 → 人话标题 / 修复建议 / 修复动作（可靠性检查 §4.5）。 */
const ERROR_KIND_META: Record<
  string,
  { title: string; hint: string; action?: { label: string; page?: string; kind?: 'settings' } }
> = {
  not_installed: {
    title: 'CLI 未安装',
    hint: '没有找到对应的命令行工具。请先安装 CLI 并确认它在 PATH 中，然后在设置里重新检测。',
    action: { label: '去设置检测引擎', page: 'settings' },
  },
  auth_missing: {
    title: '认证失败',
    hint: '引擎没有可用的登录态或 API 密钥。订阅用户请在终端完成登录；API 用户请到服务商页保存密钥。',
    action: { label: '去服务商页', page: 'providers' },
  },
  version_incompatible: {
    title: 'CLI 版本不兼容',
    hint: '当前 CLI 版本不支持 Helm 依赖的协议或受保护工具面。Codex 可切到计划/询问模式，Build 请改用兼容引擎；未来版本需在设置中重新检测通过后才会启用。',
    action: { label: '去设置检测引擎', page: 'settings' },
  },
  cwd_invalid: {
    title: '工作目录无效',
    hint: '工作目录未设置或不存在，进程没有启动。请选择一个有效目录。',
    action: { label: '去设置选择目录', page: 'settings' },
  },
  no_binding: {
    title: '引擎未绑定',
    hint: '当前引擎还没有绑定服务商和模型。完成绑定后即可对话。',
    action: { label: '去服务商页绑定', page: 'providers' },
  },
  model_unavailable: {
    title: '模型不可用',
    hint: '当前绑定的模型不存在或该服务商账号无权访问。请在服务商页重新同步模型并调整生效绑定。',
    action: { label: '去服务商页', page: 'providers' },
  },
  network: {
    title: '网络错误',
    hint: '连接服务商或网络失败。请检查网络、代理或服务商可达性后重试。',
  },
  timeout: {
    title: '长时间无响应',
    hint: '进程长时间没有输出，可能已挂起。可点击停止按钮中断本轮后重试。',
  },
  process_crash: {
    title: '进程异常退出',
    hint: 'CLI 进程非正常结束。可展开详情查看原始输出，或直接重试。',
  },
  tool_stalled: {
    title: '工具运行时间较长',
    hint: '这不是系统故障。你可以点击「停止」中断本轮，或继续等待它自行恢复。',
  },
};

const STALLED_KIND_META: Record<string, { title: string; hint: string }> = {
  waiting_approval: {
    title: '有一项操作正在等待你的确认',
    hint: '在会话里找到对应的审批卡，点击允许或拒绝即可继续。',
  },
};

export const ErrorItem = memo(function ErrorItem({
  message,
  errorKind,
  stalledKind,
}: {
  message: string;
  errorKind?: string;
  stalledKind?: string;
}) {
  const meta = errorKind ? ERROR_KIND_META[errorKind] : undefined;
  const stalledMeta =
    errorKind === 'tool_stalled' && stalledKind ? STALLED_KIND_META[stalledKind] : undefined;
  const displayMessage = message.replace(/^\[codex_probe_tool_surface_[a-z_]+\]\s*/i, '');

  return (
    <div className="item">
      <div className="item__gut" />
      <div className="item__main">
        <div className="ws-error card">
          <Icon name="alert" className="ws-error__icon" />
          <div className="ws-error__body">
            {meta ? (
              <div className="prose ws-error__title">{stalledMeta?.title ?? meta.title}</div>
            ) : null}
            <div className="prose ws-error__detail">{displayMessage}</div>
            {meta ? (
              <>
                <div className="prose ws-error__hint">{stalledMeta?.hint ?? meta.hint}</div>
                <div className="ws-error__actions">
                  {meta.action?.page ? (
                    <button
                      type="button"
                      className="btn btn--sm"
                      onClick={() =>
                        window.dispatchEvent(
                          new CustomEvent('helm:navigate', {
                            detail: { page: meta.action?.page },
                          }),
                        )
                      }
                    >
                      {meta.action.label}
                    </button>
                  ) : null}
                </div>
              </>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
});
