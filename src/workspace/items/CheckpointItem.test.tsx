import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CheckpointItem } from './CheckpointItem';

function render(restorable: boolean, fileCount: number) {
  return renderToStaticMarkup(
    <CheckpointItem
      id="checkpoint-1"
      label="改动前：app.ts"
      ts={1}
      restored={false}
      restorable={restorable}
      fileCount={fileCount}
      reason={restorable ? undefined : 'legacy_empty_snapshot'}
      onRestore={() => {}}
      onUndo={() => {}}
    />,
  );
}

describe('CheckpointItem', () => {
  it('只为具有有效文件快照的检查点显示恢复按钮', () => {
    expect(render(true, 1)).toContain('恢复');
    expect(render(false, 0)).not.toContain('>恢复<');
    expect(render(false, 0)).toContain('不可恢复');
    expect(render(true, 0)).not.toContain('>恢复<');
  });
});
