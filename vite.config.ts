import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// Helm 桌面前端（Tauri WebView 内）。
// 端口固定 1420 给 Tauri devUrl 用；@helm/protocol 走源码别名，前后端共享同一份协议类型。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  build: {
    // 生产包只收录正式入口；visual-audit.html 与其 fixture 仅供本地审计脚本使用。
    rollupOptions: {
      input: fileURLToPath(new URL('./index.html', import.meta.url)),
    },
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  preview: {
    // dev:tauri 以 preview 提供构建产物；同样锁死 127.0.0.1:1420。
    // strictPort：端口被占用时必须失败退出，禁止静默漂移端口——否则 Tauri 仍连旧服务，出现"改了没生效"的假象。
    host: '127.0.0.1',
    port: 1420,
    strictPort: true,
  },
  resolve: {
    alias: {
      '@helm/protocol': fileURLToPath(new URL('./packages/protocol/src/index.ts', import.meta.url)),
      '@': fileURLToPath(new URL('./src/', import.meta.url)),
    },
  },
});
