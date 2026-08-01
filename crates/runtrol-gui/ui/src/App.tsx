import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppShell, Theme } from "@astryxdesign/core";
import { neutralTheme } from "@astryxdesign/theme-neutral/built";
import brandLight from "../../../../assets/brand/lockup-light.svg";
import brandDark from "../../../../assets/brand/lockup-dark.svg";
import { FRAME_EVENT, OVER_EVENT, REFRESH_MS, invoke, listen } from "./bridge";
import type {
  Answered,
  FrameEnvelope,
  Notice,
  ModelCatalog,
  OfferedProvider,
  SessionListing,
  SessionRow,
  ThemeMode,
} from "./domain";
import { ConversationFeed } from "./frames";
import type { PendingFrame } from "./frames";
import { applyTheme, initialTheme } from "./theme";
import { ConversationPane } from "./components/ConversationPane";
import { NoticeCard } from "./components/NoticeCard";
import { SessionRail } from "./components/SessionRail";
import { StartSessionDialog } from "./components/StartSessionDialog";

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function App() {
  const [rows, setRows] = useState<SessionRow[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [feed] = useState(() => new ConversationFeed());
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState("");
  const [reachable, setReachable] = useState(true);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [theme, setTheme] = useState<ThemeMode>(initialTheme);
  const [startOpen, setStartOpen] = useState(false);
  const [providers, setProviders] = useState<OfferedProvider[]>([]);
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [models, setModels] = useState<ModelCatalog | null>(null);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [workspace, setWorkspace] = useState("");
  const [starting, setStarting] = useState(false);
  const [sending, setSending] = useState(false);

  const selectedRef = useRef<string | null>(null);
  const rowsRef = useRef<SessionRow[]>([]);
  const tracingRef = useRef(false);
  const refreshingRef = useRef(false);
  const firstDrawRef = useRef(true);
  const startedAtRef = useRef(performance.now());
  const frameQueueRef = useRef<string[]>([]);
  const frameFlushRef = useRef<number | null>(null);
  const frameWorkerRef = useRef<Worker | null>(null);

  useEffect(() => applyTheme(theme), [theme]);

  const trace = useCallback((line: string) => {
    if (!tracingRef.current) {
      return;
    }
    void invoke<void>("trace", { line }).catch((error: unknown) => {
      console.warn("cannot write a GUI trace", error);
    });
  }, []);

  const ask = useCallback(async <T,>(command: string, args?: Record<string, unknown>): Promise<T | undefined> => {
    try {
      const answer = await invoke<Answered<T>>(command, args);
      if (answer.outcome === "ok") {
        setReachable(true);
        return answer.value;
      }
      if (answer.outcome === "refused") {
        setReachable(true);
        const where = answer.needsTheOperator ? " (이 기계 앞에서 해결해야 한다)" : "";
        const again = answer.retryable ? " (다시 해 보면 될 수 있다)" : "";
        setNotice({ kind: "refused", message: `${answer.message}${where}${again}` });
        return undefined;
      }
      setReachable(false);
      setNotice({ kind: "broken", message: answer.message });
      return undefined;
    } catch (error) {
      setReachable(false);
      setNotice({ kind: "broken", message: messageOf(error) });
      return undefined;
    }
  }, []);

  const watchSession = useCallback(async (session: string) => {
    const watched = await ask<null>("watch", { session });
    trace(`watching ${session} ${watched === undefined ? "refused" : "ok"}`);
  }, [ask, trace]);

  const enqueueFrame = useCallback((frame: string) => {
    frameQueueRef.current.push(frame);
    if (frameFlushRef.current !== null) {
      return;
    }
    frameFlushRef.current = window.requestAnimationFrame(() => {
      frameFlushRef.current = null;
      const frames = frameQueueRef.current;
      frameQueueRef.current = [];
      const session = selectedRef.current;
      if (session && frames.length > 0) {
        frameWorkerRef.current?.postMessage({ session, frames });
      }
    });
  }, []);

  const openSession = useCallback(async (session: string, resumeCold = true) => {
    const row = rowsRef.current.find((entry) => entry.session === session);
    if (selectedRef.current === session && row?.hot) {
      return;
    }
    selectedRef.current = session;
    setSelected(session);
    feed.clear();
    if (row && !row.hot) {
      if (!resumeCold) {
        feed.status("세션을 다시 눌러 공급자 원본에 연결할 수 있다");
        return;
      }
      if (!row.native) {
        setNotice({ kind: "broken", message: "공급자가 아직 이 세션에 원본 식별자를 붙이지 않았다." });
        return;
      }
      const resumed = await ask<string>("resume", {
        provider: row.provider,
        native: row.native,
        workspace: row.workspace,
      });
      if (!resumed) {
        return;
      }
      selectedRef.current = resumed;
      setSelected(resumed);
      feed.clear();
      await watchSession(resumed);
      return;
    }
    await watchSession(session);
  }, [ask, feed, watchSession]);

  const refresh = useCallback(async () => {
    if (refreshingRef.current) {
      return;
    }
    refreshingRef.current = true;
    const askedAt = performance.now();
    try {
      const listing = await ask<SessionListing>("sessions");
      if (!listing) {
        return;
      }
      const nextRows = listing.sessions;
      rowsRef.current = nextRows;
      setRows(nextRows);
      if (listing.warnings.length > 0) {
        setNotice({ kind: "warning", message: listing.warnings.join("\n") });
      }
      const current = selectedRef.current;
      if (current && !nextRows.some((row) => row.session === current)) {
        selectedRef.current = null;
        setSelected(null);
        feed.clear();
      }

      const drawnAt = performance.now();
      if (firstDrawRef.current) {
        trace(`first list at ${(drawnAt - startedAtRef.current).toFixed(0)} ms with ${nextRows.length} rows`);
        for (const row of nextRows) {
          trace(`row ${row.provider} ${row.session} folder=${row.folder} native=${row.native ?? "-"}`);
        }
      }
      trace(`list refreshed in ${(drawnAt - askedAt).toFixed(1)} ms with ${nextRows.length} rows`);

      const opening = firstDrawRef.current;
      firstDrawRef.current = false;
      if (opening && !selectedRef.current && nextRows.length > 0) {
        await openSession(nextRows[0].session, nextRows[0].hot);
      }
    } finally {
      refreshingRef.current = false;
    }
  }, [ask, feed, openSession, trace]);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let unlisten: Array<() => void> = [];
    const parser = new Worker(new URL("./frameWorker.ts", import.meta.url), { type: "module" });
    frameWorkerRef.current = parser;
    parser.onmessage = ({ data }: MessageEvent<{ session: string; frames: PendingFrame[] }>) => {
      if (data.session !== selectedRef.current) {
        return;
      }
      feed.append(data.frames);
    };

    async function begin() {
      try {
        const listeners = await Promise.all([
          listen<FrameEnvelope>(FRAME_EVENT, ({ payload }) => {
            if (payload.session !== selectedRef.current) {
              return;
            }
            enqueueFrame(payload.frame);
            trace("frame queued");
          }),
          listen<string>(OVER_EVENT, ({ payload }) => {
            if (payload !== selectedRef.current) {
              return;
            }
            feed.status("이 세션의 흐름이 끝났다");
            trace("stream over");
          }),
        ]);
        if (!active) {
          listeners.forEach((stop) => stop());
          return;
        }
        unlisten = listeners;
        tracingRef.current = await invoke<boolean>("tracing");
        await refresh();
        if (active) {
          timer = window.setInterval(() => void refresh(), REFRESH_MS);
        }
      } catch (error) {
        if (active) {
          setReachable(false);
          setNotice({ kind: "broken", message: messageOf(error) });
        }
      }
    }

    void begin();
    return () => {
      active = false;
      if (timer !== undefined) {
        window.clearInterval(timer);
      }
      if (frameFlushRef.current !== null) {
        window.cancelAnimationFrame(frameFlushRef.current);
        frameFlushRef.current = null;
      }
      frameQueueRef.current = [];
      parser.terminate();
      if (frameWorkerRef.current === parser) {
        frameWorkerRef.current = null;
      }
      unlisten.forEach((stop) => stop());
    };
  }, [enqueueFrame, feed, refresh, trace]);

  const selectedRow = useMemo(
    () => rows.find((row) => row.session === selected) ?? null,
    [rows, selected],
  );

  const openStart = useCallback(async () => {
    const offered = await ask<OfferedProvider[]>("providers");
    if (!offered) {
      return;
    }
    setProviders(offered);
    const firstProvider = offered.find((entry) => entry.usable)?.id ?? "";
    setProvider(firstProvider);
    setModel("");
    setModels(null);
    if (!workspace && selectedRow) {
      setWorkspace(selectedRow.workspace);
    }
    setStartOpen(true);
  }, [ask, selectedRow, workspace]);

  useEffect(() => {
    if (!startOpen || !provider) {
      setModels(null);
      setModelsLoading(false);
      return;
    }
    let active = true;
    setModel("");
    setModels(null);
    setModelsLoading(true);
    void ask<ModelCatalog>("models", { provider }).then((catalogue) => {
      if (active && catalogue) {
        setModels(catalogue);
      }
      if (active) {
        setModelsLoading(false);
      }
    });
    return () => {
      active = false;
    };
  }, [ask, provider, startOpen]);

  const startSession = useCallback(async () => {
    const where = workspace.trim();
    if (!provider || !where) {
      setNotice({ kind: "broken", message: "공급자와 작업 폴더가 모두 필요하다." });
      return;
    }
    setStarting(true);
    try {
      const session = await ask<string>("start", {
        provider,
        workspace: where,
        model: model || null,
      });
      if (!session) {
        return;
      }
      setStartOpen(false);
      await refresh();
      await openSession(session);
    } finally {
      setStarting(false);
    }
  }, [ask, model, openSession, provider, refresh, workspace]);

  const send = useCallback(async (text: string) => {
    const session = selectedRef.current;
    if (!session || !text.trim() || sending) {
      return;
    }
    setDraft("");
    setSending(true);
    try {
      await ask<null>("prompt", { session, text });
    } finally {
      setSending(false);
    }
  }, [ask, sending]);

  const closeSession = useCallback(async () => {
    const session = selectedRef.current;
    if (!session) {
      return;
    }
    const closed = await ask<null>("close", { session, now: false });
    if (closed === undefined) {
      return;
    }
    selectedRef.current = null;
    setSelected(null);
    feed.clear();
    await refresh();
  }, [ask, feed, refresh]);

  const selectSession = useCallback((session: string) => {
    void openSession(session);
  }, [openSession]);

  const showStart = useCallback(() => {
    void openStart();
  }, [openStart]);

  const toggleTheme = useCallback(() => {
    setTheme((current) => current === "dark" ? "light" : "dark");
  }, []);

  return (
    <Theme theme={neutralTheme} mode={theme}>
      <AppShell
        variant="wash"
        height="fill"
        contentPadding={0}
        mobileNav={false}
        sideNav={
          <SessionRail
            rows={rows}
            selected={selected}
            query={query}
            reachable={reachable}
            theme={theme}
            onQueryChange={setQuery}
            onSelect={selectSession}
            onStart={showStart}
            onToggleTheme={toggleTheme}
          />
        }
      >
        <ConversationPane
          row={selectedRow}
          feed={feed}
          draft={draft}
          sending={sending}
          brandLight={brandLight}
          brandDark={brandDark}
          onDraftChange={setDraft}
          onSend={(text) => void send(text)}
          onClose={() => void closeSession()}
          onStart={showStart}
        />
        <StartSessionDialog
          isOpen={startOpen}
          providers={providers}
          provider={provider}
          model={model}
          models={models}
          modelsLoading={modelsLoading}
          workspace={workspace}
          starting={starting}
          onOpenChange={setStartOpen}
          onProviderChange={setProvider}
          onModelChange={setModel}
          onWorkspaceChange={setWorkspace}
          onStart={() => void startSession()}
        />
        {notice ? <NoticeCard notice={notice} onClose={() => setNotice(null)} /> : null}
      </AppShell>
    </Theme>
  );
}
