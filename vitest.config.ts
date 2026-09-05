import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: [
      {
        find: '@helm/protocol',
        replacement: fileURLToPath(new URL('./packages/protocol/src/index.ts', import.meta.url)),
      },
      {
        find: '@helm/engine-claude-code',
        replacement: fileURLToPath(
          new URL('./packages/engine-claude-code/src/index.ts', import.meta.url),
        ),
      },
      { find: /^@\/(.*)/, replacement: fileURLToPath(new URL('./src/$1', import.meta.url)) },
    ],
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
