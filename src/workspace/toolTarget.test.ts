import { describe, expect, it } from 'vitest';
import { collectTurnDeliverables, toolFilePath, toolTarget } from './toolTarget';

describe('toolFilePath（交付物行「触碰文件」口径，2026-08-30）', () => {
  it('结构化文件入参正常取值', () => {
    expect(toolFilePath({ file_path: 'D:/repo/src/main.ts' })).toBe('D:/repo/src/main.ts');
    expect(toolFilePath({ notebook_path: 'D:/repo/notes.ipynb' })).toBe('D:/repo/notes.ipynb');
    expect(toolFilePath({ path: 'src/App.tsx' })).toBe('src/App.tsx');
  });

  it('Bash 命令不算文件：查天气类轮次不应产生「查看全部文件」', () => {
    expect(toolFilePath({ command: 'pwsh -Command \'echo "websearch probe"\'' })).toBe('');
    // 对照：toolTarget 仍会为工具抬头回落到命令行，两者口径不同是有意为之
    expect(toolTarget('Bash', { command: 'echo hi' })).toBe('echo hi');
  });

  it('Grep 的搜索模式不算文件', () => {
    expect(toolFilePath({ pattern: 'TODO|FIXME' })).toBe('');
    expect(toolTarget('Grep', { pattern: 'TODO|FIXME' })).toBe('TODO|FIXME');
  });

  it('URL 不算本地文件（含 path 字段承载 URL 的抓取类工具）', () => {
    expect(toolFilePath({ url: 'https://example.com/a.txt' })).toBe('');
    expect(toolFilePath({ path: 'https://example.com/a.txt' })).toBe('');
    expect(toolFilePath({ path: 'file://D:/repo/a.txt' })).toBe('');
  });

  it('空入参与非对象入参返回空串', () => {
    expect(toolFilePath(undefined)).toBe('');
    expect(toolFilePath(null)).toBe('');
    expect(toolFilePath('D:/repo/a.ts')).toBe('');
    expect(toolFilePath({ file_path: '   ' })).toBe('');
    expect(toolFilePath({ file_path: 42 })).toBe('');
  });
});

describe('collectTurnDeliverables（轮次交付物计数）', () => {
  it('只跑过 shell/搜索/抓取的轮次：三项计数全为空', () => {
    const result = collectTurnDeliverables([
      { input: { command: 'pwsh.exe -Command \'echo "websearch probe"\'' } },
      { input: { pattern: 'TODO' } },
      { input: { url: 'https://example.com' } },
    ]);
    expect(result).toEqual({ documents: [], fileCount: 0, changeCount: 0 });
  });

  it('读文件计入触碰但不计入变更', () => {
    const result = collectTurnDeliverables([{ input: { file_path: 'src/a.ts' } }]);
    expect(result.fileCount).toBe(1);
    expect(result.changeCount).toBe(0);
  });

  it('没有 file_path 的写工具，其真实 diff 仍计入变更与触碰', () => {
    // 回归：diff 的收集一度被路径判空的 continue 提前跳过，Runtime 给出的写入会漏计
    const result = collectTurnDeliverables([
      { input: { command: 'apply patch' }, diff: { path: 'src/main.ts' } },
    ]);
    expect(result.changeCount).toBe(1);
    expect(result.fileCount).toBe(1);
    expect(result.documents).toEqual(['src/main.ts']);
  });

  it('同一文件既读又写不重复计数', () => {
    const result = collectTurnDeliverables([
      { input: { file_path: 'src/main.ts' } },
      { input: { file_path: 'src/main.ts' }, diff: { path: 'src/main.ts' } },
    ]);
    expect(result.fileCount).toBe(1);
    expect(result.changeCount).toBe(1);
  });

  it('已回溯的工具不计入', () => {
    const result = collectTurnDeliverables([
      { input: { file_path: 'src/a.ts' }, diff: { path: 'src/a.ts' }, reverted: true },
    ]);
    expect(result).toEqual({ documents: [], fileCount: 0, changeCount: 0 });
  });
});
