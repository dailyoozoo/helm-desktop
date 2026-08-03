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
    fileParallelism: false,
    include: [
      'packages/*/test/**/*.test.ts',
      'scripts/change-27l-release-audit.test.mjs',
      'src/**/*.test.ts',
      'src/**/*.test.tsx',
    ],
  },
});
