import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';

export default tseslint.config(
  {
    // 只 lint 我们自己的代码；原型、Rust、构建产物、录制样本不参与 lint。
    ignores: [
      '**/node_modules/**',
      '**/dist/**',
      '.cargo-target-*/**',
      '.helm-cargo-target/**',
      '**/.worktrees/**',
      'target/**',
      '.scratch/**',
      'tmp/**',
      '.tmp/**',
      '**/test/fixtures/**',
      'prototype/**',
      'src-tauri/**',
      '.agent/**',
      // vite/vitest 加载配置时生成的临时副本（进程异常退出未清理），非项目代码，不参与 lint
      '**/vite.config.ts.timestamp-*.mjs',
      '**/vitest.config.ts.timestamp-*.mjs',
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // 通用规则（所有 TS/JS）
    rules: {
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
    },
  },
  {
    // Node 侧：packages（CLI/适配器/协议）+ 根配置脚本
    files: ['packages/**/*.ts', 'scripts/**/*.mjs', '*.ts', '*.js'],
    languageOptions: { globals: { ...globals.node } },
  },
  {
    // 浏览器侧：前端 React（Tauri WebView）
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: { globals: { ...globals.browser } },
    plugins: { 'react-hooks': reactHooks },
    rules: {
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'warn',
    },
  },
);
