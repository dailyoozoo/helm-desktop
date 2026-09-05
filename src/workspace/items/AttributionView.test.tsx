import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { AttributionView } from './AttributionView';
import type { AttributionEntry } from '../attributionViewModel';

const entries: AttributionEntry[] = [
  {
    icon: 'file',
    label: '附件',
    sublabel: '2 个文件',
    value: '45%',
    ratio: 0.45,
    isHot: true,
    tip: '移除大附件可节省空间',
  },
  { label: '历史', value: '30%', ratio: 0.3 },
];

describe('AttributionView', () => {
  it('无条目显示暂无归因数据', () => {
    const markup = renderToStaticMarkup(<AttributionView entries={[]} />);
    expect(markup).toContain('暂无归因数据');
    expect(markup).toContain('attview__empty');
  });

  it('有条目渲染来源行、占比最高高亮与建议', () => {
    const markup = renderToStaticMarkup(<AttributionView entries={entries} />);
    expect(markup).toContain('附件');
    expect(markup).toContain('45%');
    expect(markup).toContain('is-hot');
    expect(markup).toContain('历史');
    expect(markup).toContain('移除大附件可节省空间');
    expect(markup).toContain('atttip');
  });

  it('无 isHot 时不渲染建议条', () => {
    const markup = renderToStaticMarkup(
      <AttributionView entries={[{ label: '附件', value: '45%' }]} />,
    );
    expect(markup).not.toContain('atttip');
  });
});
