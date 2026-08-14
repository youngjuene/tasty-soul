// 앱 빌드 설정. 프런트엔드 소스는 전부 `ui/` 안에 있다.
//
// 디자인 작업용 독립 실행은 `ui/preview/vite.config.ts` 를 쓴다 (`npm run design`).
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/target/**"] } },
  build: { target: "es2021", sourcemap: false, outDir: "../dist", emptyOutDir: true },
});
