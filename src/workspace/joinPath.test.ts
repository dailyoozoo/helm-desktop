import { describe, expect, it } from 'vitest';
import { joinPath } from './ContextPanel';

describe('joinPath（变更-33：文件预览路径解析）', () => {
  it('相对路径拼接到 cwd 并用 / 连接', () => {
    expect(joinPath('D:/work/proj', 'src/main.ts')).toBe('D:/work/proj/src/main.ts');
  });

  it('cwd 尾斜杠去重', () => {
    expect(joinPath('D:/work/proj/', 'src/main.ts')).toBe('D:/work/proj/src/main.ts');
  });

  it('盘符绝对路径原样返回', () => {
    expect(joinPath('D:/work/proj', 'E:/else/file.txt')).toBe('E:/else/file.txt');
  });

  it('UNC/反斜杠路径原样返回', () => {
    expect(joinPath('D:/work/proj', '\\\\server\\share\\file.txt')).toBe(
      '\\\\server\\share\\file.txt',
    );
  });

  it('不改变窗口形式的相对路径（保持 CLI 相对写法）', () => {
    // 相对但无前导斜杠，按 cwd 拼
    expect(joinPath('D:/work', 'docs/a.md')).toBe('D:/work/docs/a.md');
  });

  it('空字符串或仅空白被规范化成空串透传', () => {
    expect(joinPath('D:/work', '')).toBe('');
    expect(joinPath('D:/work', '   ')).toBe('');
  });

  it('以 / 开头的路径若在 cwd 下也拼接到 cwd（不作为绝对根）', () => {
    expect(joinPath('D:/work', '/src/main.ts')).toBe('D:/work/src/main.ts');
  });
});
