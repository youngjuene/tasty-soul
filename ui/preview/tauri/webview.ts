/** `@tauri-apps/api/webview` 스텁. 드래그앤드롭은 미리보기에서 동작하지 않는다. */
export function getCurrentWebview() {
  return {
    label: "main",
    onDragDropEvent: async (_cb: (e: unknown) => void) => () => {},
  };
}
export const getCurrentWebviewWindow = getCurrentWebview;
