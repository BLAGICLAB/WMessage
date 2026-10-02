import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  // 同 vite.config.ts：lobehub 图标 SVG 在测试里也走文件路径（不内联 data URI），
  // 让 logo 断言能稳定匹配 lobe/<slug> 路径（实测移除后 ?url 返回空串，断言全挂）
  build: {
    assetsInlineLimit: (filePath) =>
      filePath.endsWith(".svg") ? false : undefined,
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    exclude: ["node_modules", "src-tauri", "**/*.d.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/test/**",
        "src/**/*.test.{ts,tsx}",
        "src/**/*.spec.{ts,tsx}",
      ],
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
});
