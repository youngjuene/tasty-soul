/** `@tauri-apps/api/event` 스텁. 디자인 미리보기에는 이벤트 소스가 없다. */
export async function listen(_e: string, _cb: (p: unknown) => void) {
  return () => {};
}
export async function emit() {}
export async function once() {
  return () => {};
}
export const TauriEvent = {} as Record<string, string>;
