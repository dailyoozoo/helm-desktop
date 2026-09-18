// vitest.config.ts
import { fileURLToPath } from "node:url";
import { defineConfig } from "file:///D:/6_%E5%85%B6%E4%BB%96/gmini/helm-desktop/node_modules/vitest/dist/config.js";
var __vite_injected_original_import_meta_url = "file:///D:/6_%E5%85%B6%E4%BB%96/gmini/helm-desktop/vitest.config.ts";
var vitest_config_default = defineConfig({
  resolve: {
    alias: [
      {
        find: "@helm/protocol",
        replacement: fileURLToPath(new URL("./packages/protocol/src/index.ts", __vite_injected_original_import_meta_url))
      },
      {
        find: "@helm/engine-claude-code",
        replacement: fileURLToPath(
          new URL("./packages/engine-claude-code/src/index.ts", __vite_injected_original_import_meta_url)
        )
      },
      { find: /^@\/(.*)/, replacement: fileURLToPath(new URL("./src/$1", __vite_injected_original_import_meta_url)) }
    ]
  },
  test: {
    fileParallelism: false,
    include: [
      "packages/*/test/**/*.test.ts",
      "scripts/change-27l-release-audit.test.mjs",
      "src/**/*.test.ts",
      "src/**/*.test.tsx"
    ]
  }
});
export {
  vitest_config_default as default
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidml0ZXN0LmNvbmZpZy50cyJdLAogICJzb3VyY2VzQ29udGVudCI6IFsiY29uc3QgX192aXRlX2luamVjdGVkX29yaWdpbmFsX2Rpcm5hbWUgPSBcIkQ6XFxcXDZfXHU1MTc2XHU0RUQ2XFxcXGdtaW5pXFxcXGhlbG0tZGVza3RvcFwiO2NvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9maWxlbmFtZSA9IFwiRDpcXFxcNl9cdTUxNzZcdTRFRDZcXFxcZ21pbmlcXFxcaGVsbS1kZXNrdG9wXFxcXHZpdGVzdC5jb25maWcudHNcIjtjb25zdCBfX3ZpdGVfaW5qZWN0ZWRfb3JpZ2luYWxfaW1wb3J0X21ldGFfdXJsID0gXCJmaWxlOi8vL0Q6LzZfJUU1JTg1JUI2JUU0JUJCJTk2L2dtaW5pL2hlbG0tZGVza3RvcC92aXRlc3QuY29uZmlnLnRzXCI7aW1wb3J0IHsgZmlsZVVSTFRvUGF0aCB9IGZyb20gJ25vZGU6dXJsJztcbmltcG9ydCB7IGRlZmluZUNvbmZpZyB9IGZyb20gJ3ZpdGVzdC9jb25maWcnO1xuXG5leHBvcnQgZGVmYXVsdCBkZWZpbmVDb25maWcoe1xuICByZXNvbHZlOiB7XG4gICAgYWxpYXM6IFtcbiAgICAgIHtcbiAgICAgICAgZmluZDogJ0BoZWxtL3Byb3RvY29sJyxcbiAgICAgICAgcmVwbGFjZW1lbnQ6IGZpbGVVUkxUb1BhdGgobmV3IFVSTCgnLi9wYWNrYWdlcy9wcm90b2NvbC9zcmMvaW5kZXgudHMnLCBpbXBvcnQubWV0YS51cmwpKSxcbiAgICAgIH0sXG4gICAgICB7XG4gICAgICAgIGZpbmQ6ICdAaGVsbS9lbmdpbmUtY2xhdWRlLWNvZGUnLFxuICAgICAgICByZXBsYWNlbWVudDogZmlsZVVSTFRvUGF0aChcbiAgICAgICAgICBuZXcgVVJMKCcuL3BhY2thZ2VzL2VuZ2luZS1jbGF1ZGUtY29kZS9zcmMvaW5kZXgudHMnLCBpbXBvcnQubWV0YS51cmwpLFxuICAgICAgICApLFxuICAgICAgfSxcbiAgICAgIHsgZmluZDogL15AXFwvKC4qKS8sIHJlcGxhY2VtZW50OiBmaWxlVVJMVG9QYXRoKG5ldyBVUkwoJy4vc3JjLyQxJywgaW1wb3J0Lm1ldGEudXJsKSkgfSxcbiAgICBdLFxuICB9LFxuICB0ZXN0OiB7XG4gICAgZmlsZVBhcmFsbGVsaXNtOiBmYWxzZSxcbiAgICBpbmNsdWRlOiBbXG4gICAgICAncGFja2FnZXMvKi90ZXN0LyoqLyoudGVzdC50cycsXG4gICAgICAnc2NyaXB0cy9jaGFuZ2UtMjdsLXJlbGVhc2UtYXVkaXQudGVzdC5tanMnLFxuICAgICAgJ3NyYy8qKi8qLnRlc3QudHMnLFxuICAgICAgJ3NyYy8qKi8qLnRlc3QudHN4JyxcbiAgICBdLFxuICB9LFxufSk7XG4iXSwKICAibWFwcGluZ3MiOiAiO0FBQTRSLFNBQVMscUJBQXFCO0FBQzFULFNBQVMsb0JBQW9CO0FBRHdJLElBQU0sMkNBQTJDO0FBR3ROLElBQU8sd0JBQVEsYUFBYTtBQUFBLEVBQzFCLFNBQVM7QUFBQSxJQUNQLE9BQU87QUFBQSxNQUNMO0FBQUEsUUFDRSxNQUFNO0FBQUEsUUFDTixhQUFhLGNBQWMsSUFBSSxJQUFJLG9DQUFvQyx3Q0FBZSxDQUFDO0FBQUEsTUFDekY7QUFBQSxNQUNBO0FBQUEsUUFDRSxNQUFNO0FBQUEsUUFDTixhQUFhO0FBQUEsVUFDWCxJQUFJLElBQUksOENBQThDLHdDQUFlO0FBQUEsUUFDdkU7QUFBQSxNQUNGO0FBQUEsTUFDQSxFQUFFLE1BQU0sWUFBWSxhQUFhLGNBQWMsSUFBSSxJQUFJLFlBQVksd0NBQWUsQ0FBQyxFQUFFO0FBQUEsSUFDdkY7QUFBQSxFQUNGO0FBQUEsRUFDQSxNQUFNO0FBQUEsSUFDSixpQkFBaUI7QUFBQSxJQUNqQixTQUFTO0FBQUEsTUFDUDtBQUFBLE1BQ0E7QUFBQSxNQUNBO0FBQUEsTUFDQTtBQUFBLElBQ0Y7QUFBQSxFQUNGO0FBQ0YsQ0FBQzsiLAogICJuYW1lcyI6IFtdCn0K
