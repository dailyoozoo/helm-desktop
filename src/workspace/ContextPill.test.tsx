import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ContextPill, contextPillLabel } from './ContextPill';

describe('ContextPill', () => {
  it('attachment 药丸展示文件名，且可移除', () => {
    const markup = renderToStaticMarkup(
      <ContextPill
        item={{
          kind: 'attachment',
          path: 'D:/work/src/auth/token.ts',
          label: 'token.ts',
        }}
        onRemove={() => {}}
      />,
    );
    expect(markup).toContain('data-kind="attachment"');
    expect(markup).toContain('token.ts');
    expect(markup).toContain('aria-label="移除 token.ts"');
  });

  it('mention 药丸在 cwd 下显示相对路径，点击移除传回绝对路径', () => {
    const removed: string[] = [];
    const markup = renderToStaticMarkup(
      <ContextPill
        item={{
          kind: 'mention',
          path: 'D:/work/src/auth/token.ts',
          label: 'src/auth/token.ts',
        }}
        onRemove={(path) => removed.push(path)}
      />,
    );
    expect(markup).toContain('data-kind="mention"');
    expect(markup).toContain('src/auth/token.ts');
    expect(markup).toContain('title="D:/work/src/auth/token.ts"');
    expect(removed).toEqual([]);
  });

  it('contextPillLabel：mention 在 cwd 前缀下剥离出相对路径', () => {
    expect(contextPillLabel('D:/work/src/auth/token.ts', 'mention', 'D:/work')).toBe(
      'src/auth/token.ts',
    );
    expect(contextPillLabel('D:/work/readme.md', 'mention', 'D:/work/')).toBe('readme.md');
    expect(contextPillLabel('C:/other/file.ts', 'mention', 'D:/work')).toBe('file.ts');
  });

  it('contextPillLabel：attachment 始终取文件名', () => {
    expect(contextPillLabel('D:/work/src/auth/token.ts', 'attachment', 'D:/work')).toBe('token.ts');
  });
});
