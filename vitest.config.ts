import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@helm/protocol': fileURLToPath(new URL('./packages/protocol/src/index.ts', import.meta.url)),
      '@helm/engine-claude-code': fileURLToPath(
        new URL('./packages/engine-claude-code/src/index.ts', import.meta.url),
      ),
    },
  },
  test: {
    include: ['packages/*/test/**/*.test.ts', 'src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
});
