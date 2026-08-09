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
        availableDecisions: persistable ? ['allow', 'session', 'deny'] : ['allow', 'deny'],
        decision: status === 'resolved' ? 'session' : undefined,
        persistentLabel: persistable ? '本会话总是允许这条命令' : undefined,
        matcherSummary: persistable ? '当前引擎 + 当前项目 + Bash + ls -la' : undefined,
      }}
      onRespond={vi.fn()}
    />,
  );
}

describe('ApprovalCard', () => {
  it('disables all decisions and shows applying feedback while awaiting backend confirmation', () => {
    const html = render('applying');

    expect(html.match(/disabled=""/g)).toHaveLength(3);
    expect(html).toContain('正在应用审批');
    expect(html).not.toContain('已批准');
  });

  it('shows a retryable error and re-enables decisions after failure', () => {
    const html = render('failed', '恢复引擎失败');

    expect(html).toContain('恢复引擎失败');
    expect(html).toContain('重试当次允许');
    expect(html).toContain('重试总是允许');
    expect(html).toContain('重试拒绝');
    expect(html).toContain('选择&quot;总是允许&quot;后，本会话执行该程序不再逐条确认。');
    expect(html).toContain('不再逐条确认');
    expect(html).not.toContain('disabled=""');
  });

  it('only shows the terminal handled state after backend confirmation', () => {
    const html = render('resolved');

    expect(html).toContain('已批准本会话');
    expect(html.match(/disabled=""/g)).toHaveLength(3);
  });

  it('hides session-wide approval when the backend cannot produce a stable matcher', () => {
    const html = render('pending', undefined, false);

    expect(html).toContain('当次允许');
    expect(html).toContain('拒绝');
    expect(html).not.toContain('总是允许');
    expect(html.match(/<button/g)).toHaveLength(2);
  });
});
