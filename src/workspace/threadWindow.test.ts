import { describe, expect, it } from 'vitest';
import { expandThreadWindow, threadWindow } from './threadWindow';

describe('threadWindow', () => {
  it('keeps only the latest requested items visible', () => {
    const items = Array.from({ length: 260 }, (_, index) => ({ id: index }));

    const result = threadWindow(items, 200);

    expect(result.hiddenCount).toBe(60);
    expect(result.visibleItems).toHaveLength(200);
    expect(result.visibleItems[0]).toBe(items[60]);
    expect(result.visibleItems.at(-1)).toBe(items[259]);
  });

  it('expands enough to reveal the remaining earlier items', () => {
    expect(expandThreadWindow(200, 60)).toBe(260);
    expect(expandThreadWindow(200, 250)).toBe(400);
  });
});
