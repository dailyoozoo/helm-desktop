import { useEffect, useState } from 'react';
import { Icon, type IconName } from '../shell/icons';
import { showResultToast } from '../components/toast';
import { getPermissionRules, removePermissionRule } from './api';
import { describePermissionRule, type PermissionRule } from './permissionRules';

/**
 * 已保存授权抽屉：对齐原型 settings.html 的纯列表形态——只展示与撤销已保存的跨任务授权，
 * 不在抽屉里提供危险全局开关。显式拒绝规则的创建与权限审计管理属真实后端能力，
 * 命令仍在 api.ts 保留以备其他入口，本抽屉不再调用（AGENTS.md 权限红线）。
 */
export function AuthorizationDrawer({ onClose }: { onClose: () => void }) {
  const [permissionRules, setPermissionRules] = useState<PermissionRule[]>([]);
  const [permissionLoadError, setPermissionLoadError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    getPermissionRules()
      .then((rules) => {
        if (active) setPermissionRules(rules);
      })
      .catch((error: unknown) => {
        if (active)
          setPermissionLoadError(error instanceof Error ? error.message : '读取持久权限失败');
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [onClose]);

  const revokePermissionRule = async (rule: PermissionRule) => {
    try {
      const result = await removePermissionRule(rule.id);
      setPermissionRules(result.rules);
      showResultToast(
        result.revocationTooLateCount > 0
          ? '已撤销规则；' + result.revocationTooLateCount + ' 个操作已开始，无法追回，已写入审计'
          : '已撤销「' + describePermissionRule(rule).title + '」',
      );
    } catch (error) {
      showResultToast('撤销失败：' + (error instanceof Error ? error.message : String(error)));
    }
  };

  return (
    <div className="cm-drawer-backdrop is-open" onClick={onClose}>
      <aside
        className="cm-drawer"
        role="dialog"
        aria-modal="true"
        aria-label="已保存授权"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="cm-drawer__head">
          <div>
            <h2>已保存授权</h2>
            <p>只能查看和撤销已有记录。</p>
          </div>
          <button className="btn-icon" type="button" aria-label="关闭" onClick={onClose}>
            <Icon name="x" />
          </button>
        </div>

        <div className="cm-drawer__body">
          <div className="cm-note">
            <Icon name="shield" />
            <span>显式拒绝和当前任务权限仍优先；这里不会创建"全部放开"的全局授权。</span>
          </div>

          {permissionLoadError ? (
            <div className="settings-inline-error" role="alert" style={{ marginTop: 16 }}>
              <span>{permissionLoadError}</span>
              <button
                className="btn btn--subtle btn--sm"
                type="button"
                onClick={() => {
                  setPermissionLoadError(null);
                  void getPermissionRules()
                    .then(setPermissionRules)
                    .catch((error: unknown) =>
                      setPermissionLoadError(
                        error instanceof Error ? error.message : '读取持久权限失败',
                      ),
                    );
                }}
              >
                重试
              </button>
            </div>
          ) : null}

          <div className="mt-16">
            {permissionRules.map((rule) => {
              const card = authCardForRule(rule);
              return (
                <div className="cm-auth-card" key={rule.id}>
                  <span className="cm-auth-card__icon">
                    <Icon name={card.icon} />
                  </span>
                  <div className="cm-auth-card__main">
                    <b className="mono">{card.title}</b>
                    <small>{card.meta}</small>
                    <span className="cm-auth-card__type">
                      <Icon name={card.typeIcon} />
                      {card.typeLabel}
                    </span>
                  </div>
                  <button
                    className="cm-action cm-auth-card__action"
                    type="button"
                    onClick={() => void revokePermissionRule(rule)}
                  >
                    撤销
                  </button>
                </div>
              );
            })}
            {!permissionLoadError && permissionRules.length === 0 ? (
              <div className="st-empty" style={{ marginTop: 16 }}>
                <b>暂无已保存授权</b>
                <p>审批卡上选择「总是允许」后会出现在这里。</p>
              </div>
            ) : null}
          </div>
        </div>
      </aside>
    </div>
  );
}

/** 把一条权限规则映射成原型授权卡的展示字段（图标/标题/元数据/类型胶囊）。 */
function authCardForRule(rule: PermissionRule): {
  icon: IconName;
  title: string;
  meta: string;
  typeIcon: IconName;
  typeLabel: string;
} {
  const engine =
    rule.engine === 'claude-code' ? 'Claude Code' : rule.engine === 'codex' ? 'Codex' : '所有引擎';
  const scope =
    rule.scope === 'global'
      ? '全局'
      : rule.scope === 'project' && rule.scopeBinding?.projectRoot
        ? rule.scopeBinding.projectRoot
        : rule.scope === 'session'
          ? '本会话'
          : rule.scope === 'turn'
            ? '本轮'
            : '仅一次';
  const date = formatRuleDate(rule.createdAt);
  const meta = `${engine} · ${scope} · ${date}`;

  switch (rule.capability) {
    case 'process_exec':
      return {
        icon: 'terminal',
        title: rule.operation || '执行命令',
        meta,
        typeIcon: 'cpu',
        typeLabel: '进程执行',
      };
    case 'network_request':
      return {
        icon: 'upright',
        title: rule.resourcePattern || '访问网络',
        meta,
        typeIcon: 'upright',
        typeLabel: '网络读取',
      };
    case 'mcp_invoke':
      return {
        icon: 'puzzle',
        title: rule.operation || '调用 MCP',
        meta,
        typeIcon: 'puzzle',
        typeLabel: 'MCP 调用',
      };
    case 'file_write':
      return {
        icon: 'edit',
        title: rule.resourcePattern || '修改文件',
        meta,
        typeIcon: 'edit',
        typeLabel: '写入文件',
      };
    case 'file_read':
      return {
        icon: 'file',
        title: rule.resourcePattern || '读取文件',
        meta,
        typeIcon: 'file',
        typeLabel: '读取文件',
      };
    case 'directory_list':
      return {
        icon: 'folder',
        title: rule.resourcePattern || '浏览目录',
        meta,
        typeIcon: 'folder',
        typeLabel: '浏览目录',
      };
    default: {
      const unknown = rule.capability.startsWith('unknown:')
        ? rule.capability.slice('unknown:'.length)
        : rule.capability;
      return {
        icon: 'dot',
        title: rule.resourcePattern || `未知能力（${unknown}）`,
        meta,
        typeIcon: 'dot',
        typeLabel: '未知',
      };
    }
  }
}

function formatRuleDate(timestamp: number): string {
  const date = new Date(timestamp);
  return `${date.getMonth() + 1}月${date.getDate()}日`;
}
