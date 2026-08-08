import { defineConfig } from "vite";

// 기본 Vite 설정. Tauri devUrl(tauri.conf.json)과 포트를 맞추기 위해 5173 고정.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
