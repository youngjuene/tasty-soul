// 디자인 미리보기 — Rust · Tauri · API 키 없이 화면만 띄운다.
//
//   npm run design      → http://localhost:1421
//
// Tauri 모듈을 전부 `preview/` 의 스텁으로 바꿔치기한다. 화면 코드는 손대지 않는다.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

const here = (p: string) => path.resolve(__dirname, p);

export default defineConfig({
  root: path.resolve(__dirname, ".."),
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      "@tauri-apps/api/core": here("mock.ts"),
      "@tauri-apps/api/event": here("tauri/event.ts"),
      "@tauri-apps/api/webview": here("tauri/webview.ts"),
      "@tauri-apps/plugin-clipboard-manager": here("tauri/clipboard.ts"),
      "@tauri-apps/plugin-dialog": here("tauri/dialog.ts"),
      "@tauri-apps/plugin-global-shortcut": here("tauri/shortcut.ts"),
      "@tauri-apps/plugin-opener": here("tauri/opener.ts"),
    },
  },
  server: { port: 1421, strictPort: true, open: true },
});
