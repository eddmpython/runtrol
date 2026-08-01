type Unlisten = () => void;

type TauriBridge = {
  core: {
    invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  };
  event: {
    listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<Unlisten>;
  };
};

declare global {
  interface Window {
    __TAURI__?: TauriBridge;
  }
}

export const FRAME_EVENT = "session-frame";
export const OVER_EVENT = "session-over";
export const REFRESH_MS = 1_000;

function tauri(): TauriBridge {
  const bridge = window.__TAURI__;
  if (!bridge) {
    throw new Error("runtrol desktop bridge is unavailable");
  }
  return bridge;
}

export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return tauri().core.invoke<T>(command, args);
}

export function listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<Unlisten> {
  return tauri().event.listen<T>(event, handler);
}
