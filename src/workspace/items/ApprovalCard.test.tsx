import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ApprovalCard } from './ApprovalCard';

function render(
  status: 'pending' | 'applying' | 'resolved' | 'failed',
  error?: string,
  persistable = true,
) {
  return renderToStaticMarkup(
    <ApprovalCard
      item={{
        kind: 'approval',
        id: 'approval-1',
        action: 'Bash',
        detail: 'ls -la',
        status,
        error,
        availableDecisions: persistable
          ? ['allow', 'turn', 'session', 'project', 'always', 'deny']
          : ['allow', 'deny'],
        decision: status === 'resolved' ? 'project' : undefined,
        persistentLabel: persistable ? '此项目永久允许这条命令' : undefined,
        matcherSummary: persistable ? '当前引擎 + 当前项目 + Bash + ls -la' : undefined,
      }}
      onRespond={vi.fn()}
    />,
  );
}

describe('ApprovalCard', () => {
  it('disables all decisions and shows applying feedback while awaiting backend confirmation', () => {
    const html = render('applying');

    expect(html.match(/disabled=""/g)).toHaveLength(6);
    expect(html).toContain('正在应用审批');
    expect(html).not.toContain('已批准');
  });

  it('shows a retryable error and re-enables decisions after failure', () => {
    const html = render('failed', '恢复引擎失败');

    expect(html).toContain('恢复引擎失败');
    expect(html).toContain('重试仅允许一次');
    expect(html).toContain('重试本轮允许');
    expect(html).toContain('重试本会话允许');
    expect(html).toContain('重试此项目允许');
    expect(html).toContain('重试所有项目允许');
    expect(html).toContain('当前引擎 + 当前项目 + Bash + ls -la');
    expect(html).not.toContain('disabled=""');
  });

  it('only shows the terminal handled state after backend confirmation', () => {
    const html = render('resolved');

    expect(html).toContain('已批准此项目');
    expect(html.match(/disabled=""/g)).toHaveLength(6);
  });

  it('hides permanent approval when the backend cannot produce a stable matcher', () => {
    const html = render('pending', undefined, false);

    expect(html).toContain('仅允许一次');
    expect(html).toContain('拒绝');
    expect(html).not.toContain('永久允许');
    expect(html.match(/<button/g)).toHaveLength(2);
  });
});
