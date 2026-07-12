import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Helm 桌面前端（Tauri WebView 内）。
// 端口固定 1420 给 Tauri devUrl 用；@helm/protocol 走源码别名，前后端共享同一份协议类型。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  resolve: {
    alias: {
      '@helm/protocol': fileURLToPath(new URL('./packages/protocol/src/index.ts', import.meta.url)),
    },
  },
});
