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
  RateLimitGauge,
  SessionListing,
  SessionRow,
  ThemeMode,
  UsageGauge,
  WatchCursor,
  WatchOver,
  WatchStarted,
} from "./domain";
import { ConversationFeed } from "./frames";
import type { PendingFrame } from "./frames";
import { applyTheme, initialTheme } from "./theme";
import { ConversationPane } from "./components/ConversationPane";
import { NoticeCard } from "./components/NoticeCard";
import { SessionRail } from "./components/SessionRail";
import { StartSessionDialog } from "./components/StartSessionDialog";
import { preferredProvider, rememberProvider } from "./preferences";

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const FRAME_QUEUE_FRAMES = 64;
const FRAME_QUEUE_CHARACTERS = 16 * 1024 * 1024 + 64 * 1024;

type ActiveWatch = {
  session: string;
  selection: number;
  intent: number | null;
  view: number | null;
  cursor: WatchCursor | null;
  retries: number;
  over: WatchOver | null;
};

type ParsedFrameBatch = {
  session: string;
  view: number;
  frames: Array<{ pending: PendingFrame; nextExpected: WatchCursor }>;
};

function follows(cursor: WatchCursor, next: WatchCursor): boolean {
  return cursor.stream === next.stream
    && cursor.epoch === next.epoch
    && next.seq === cursor.seq + 1;
}

function sameCursor(left: WatchCursor, right: WatchCursor): boolean {
  return left.stream === right.stream && left.epoch === right.epoch && left.seq === right.seq;
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
  const [usage, setUsage] = useState<UsageGauge | null>(null);
  const [rateLimit, setRateLimit] = useState<RateLimitGauge | null>(null);

  const selectedRef = useRef<string | null>(null);
  const rowsRef = useRef<SessionRow[]>([]);
  const tracingRef = useRef(false);
  const refreshingRef = useRef(false);
  const firstDrawRef = useRef(true);
  const startedAtRef = useRef(performance.now());
  const frameQueueRef = useRef<FrameEnvelope[]>([]);
  const frameQueueCharactersRef = useRef(0);
  const frameFlushRef = useRef<number | null>(null);
  const frameWorkerRef = useRef<Worker | null>(null);
  const frameWorkerBusyRef = useRef(false);
  const selectionRef = useRef(0);
  const nextWatchIntentRef = useRef(0);
  const watchRef = useRef<ActiveWatch | null>(null);
  const retryTimerRef = useRef<number | null>(null);
  const watchSessionRef = useRef<(session: string, selection: number, reconnect: boolean) => void>(() => {});

  useEffect(() => applyTheme(theme), [theme]);

  const trace = useCallback((line: string) => {
    if (!tracingRef.current) {
      return;
    }
    void invoke<void>("trace", { line }).catch((error: unknown) => {
      console.warn("cannot write a GUI trace", error);
    });
  }, []);

  const applyAnswer = useCallback(<T,>(answer: Answered<T>): T | undefined => {
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
  }, []);

  const ask = useCallback(async <T,>(command: string, args?: Record<string, unknown>): Promise<T | undefined> => {
    try {
      return applyAnswer(await invoke<Answered<T>>(command, args));
    } catch (error) {
      setReachable(false);
      setNotice({ kind: "broken", message: messageOf(error) });
      return undefined;
    }
  }, [applyAnswer]);

  const askForSelection = useCallback(async <T,>(
    selection: number,
    command: string,
    args?: Record<string, unknown>,
  ): Promise<Answered<T> | null> => {
    try {
      const answer = await invoke<Answered<T>>(command, args);
      return watchRef.current?.selection === selection ? answer : null;
    } catch (error) {
      if (watchRef.current?.selection !== selection) {
        return null;
      }
      return { outcome: "broken", message: messageOf(error) };
    }
  }, []);

  const scheduleReconnect = useCallback((session: string, selection: number) => {
    const active = watchRef.current;
    if (!active || active.session !== session || active.selection !== selection) {
      return;
    }
    const previousIntent = active.intent;
    active.intent = null;
    active.view = null;
    if (previousIntent !== null) {
      void invoke<Answered<null>>("stop_watch", { intent: previousIntent });
    }
    active.retries += 1;
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current);
    }
    const delay = Math.min(5_000, 250 * 2 ** Math.min(active.retries - 1, 5));
    retryTimerRef.current = window.setTimeout(() => {
      retryTimerRef.current = null;
      watchSessionRef.current(session, selection, true);
    }, delay);
  }, []);

  const finishWatchOver = useCallback((watched: ActiveWatch) => {
    const over = watched.over;
    if (!over || !watched.cursor || !sameCursor(watched.cursor, over.nextExpected)) {
      return;
    }
    feed.status(over.lagged
      ? "화면이 출력 속도를 따라가지 못해 다시 연결한다"
      : "출력 연결이 끝나 마지막으로 그린 위치에서 다시 연결한다");
    trace(`stream over view=${over.view}`);
    scheduleReconnect(watched.session, watched.selection);
  }, [feed, scheduleReconnect, trace]);

  const watchSession = useCallback(async (session: string, selection: number, reconnect: boolean) => {
    const active = watchRef.current;
    if (!active || active.session !== session || active.selection !== selection) {
      return;
    }
    const after = reconnect ? active.cursor : null;
    nextWatchIntentRef.current += 1;
    const intent = nextWatchIntentRef.current;
    active.intent = intent;
    const answer = await askForSelection<WatchStarted | null>(selection, "watch", {
      session,
      after,
      intent,
    });
    const current = watchRef.current;
    if (
      !answer
      || !current
      || current.session !== session
      || current.selection !== selection
      || current.intent !== intent
    ) {
      void invoke<Answered<null>>("stop_watch", { intent });
      return;
    }
    const started = applyAnswer(answer);
    if (!started) {
      if (answer.outcome === "broken" || (answer.outcome === "refused" && answer.retryable)) {
        scheduleReconnect(session, selection);
      }
      return;
    }
    current.view = started.view;
    current.cursor = started.startsAt;
    current.over = null;
    if (started.gap) {
      feed.status("연결이 끊긴 동안 보관 한계를 넘은 출력이 있어 빈 구간을 표시한다");
      current.cursor = started.liveAt;
    }
    const continued = await askForSelection<null>(selection, "continue_watch", { view: started.view });
    if (!continued || applyAnswer(continued) === undefined) {
      scheduleReconnect(session, selection);
      return;
    }
    trace(`watching ${session} view=${started.view}`);
  }, [applyAnswer, askForSelection, feed, scheduleReconnect, trace]);
  watchSessionRef.current = (session, selection, reconnect) => {
    void watchSession(session, selection, reconnect);
  };

  const scheduleFrameFlush = useCallback(() => {
    if (frameFlushRef.current !== null || frameWorkerBusyRef.current) {
      return;
    }
    frameFlushRef.current = window.requestAnimationFrame(() => {
      frameFlushRef.current = null;
      if (frameWorkerBusyRef.current) {
        return;
      }
      const queued = frameQueueRef.current;
      frameQueueRef.current = [];
      frameQueueCharactersRef.current = 0;
      const active = watchRef.current;
      if (!active || active.view === null) {
        return;
      }
      const frames = queued
        .filter((frame) => frame.session === active.session && frame.view === active.view)
        .map(({ frame, nextExpected }) => ({ frame, nextExpected }));
      if (frames.length > 0) {
        frameWorkerBusyRef.current = true;
        frameWorkerRef.current?.postMessage({
          session: active.session,
          view: active.view,
          frames,
        });
      }
    });
  }, []);

  const enqueueFrame = useCallback((envelope: FrameEnvelope) => {
    const active = watchRef.current;
    const nextCharacters = frameQueueCharactersRef.current + envelope.frame.length;
    if (
      frameQueueRef.current.length >= FRAME_QUEUE_FRAMES
      || nextCharacters > FRAME_QUEUE_CHARACTERS
    ) {
      frameQueueRef.current = [];
      frameQueueCharactersRef.current = 0;
      if (active) {
        scheduleReconnect(active.session, active.selection);
      }
      return;
    }
    frameQueueRef.current.push(envelope);
    frameQueueCharactersRef.current = nextCharacters;
    scheduleFrameFlush();
  }, [scheduleFrameFlush, scheduleReconnect]);

  const openSession = useCallback(async (session: string, resumeCold = true) => {
    const row = rowsRef.current.find((entry) => entry.session === session);
    if (selectedRef.current === session && row?.hot && watchRef.current?.view !== null) {
      return;
    }
    const previousIntent = watchRef.current?.intent;
    if (previousIntent !== null && previousIntent !== undefined) {
      void invoke<Answered<null>>("stop_watch", { intent: previousIntent });
    }
    selectionRef.current += 1;
    const selection = selectionRef.current;
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    watchRef.current = {
      session,
      selection,
      intent: null,
      view: null,
      cursor: null,
      retries: 0,
      over: null,
    };
    selectedRef.current = session;
    setSelected(session);
    feed.clear();
    setUsage(null);
    setRateLimit(null);
    if (row && !row.hot) {
      if (!resumeCold) {
        feed.status("세션을 다시 눌러 공급자 원본에 연결할 수 있다");
        return;
      }
      if (!row.native) {
        setNotice({ kind: "broken", message: "공급자가 아직 이 세션에 원본 식별자를 붙이지 않았다." });
        return;
      }
      const resumedAnswer = await askForSelection<string>(selection, "resume", {
        provider: row.provider,
        native: row.native,
        workspace: row.workspace,
      });
      if (!resumedAnswer) {
        return;
      }
      const resumed = applyAnswer(resumedAnswer);
      if (!resumed) {
        return;
      }
      if (watchRef.current?.selection !== selection) {
        return;
      }
      watchRef.current.session = resumed;
      selectedRef.current = resumed;
      setSelected(resumed);
      feed.clear();
      await watchSession(resumed, selection, false);
      return;
    }
    await watchSession(session, selection, false);
  }, [applyAnswer, askForSelection, feed, watchSession]);

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
        const intent = watchRef.current?.intent;
        if (intent !== null && intent !== undefined) {
          void invoke<Answered<null>>("stop_watch", { intent });
        }
        if (retryTimerRef.current !== null) {
          window.clearTimeout(retryTimerRef.current);
          retryTimerRef.current = null;
        }
        selectionRef.current += 1;
        watchRef.current = null;
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
    parser.onmessage = ({ data }: MessageEvent<ParsedFrameBatch>) => {
      frameWorkerBusyRef.current = false;
      scheduleFrameFlush();
      const watched = watchRef.current;
      if (!watched || watched.session !== data.session || watched.view !== data.view || !watched.cursor) {
        return;
      }
      let cursor = watched.cursor;
      const accepted: PendingFrame[] = [];
      let discontinuity = false;
      for (const frame of data.frames) {
        if (!follows(cursor, frame.nextExpected)) {
          discontinuity = true;
          break;
        }
        accepted.push(frame.pending);
        cursor = frame.nextExpected;
      }
      if (accepted.length > 0) {
        feed.append(accepted);
        watched.cursor = cursor;
        watched.retries = 0;
        for (const frame of accepted) {
          if (frame.usage) {
            setUsage(frame.usage);
          }
          if (frame.rateLimit) {
            setRateLimit(frame.rateLimit);
          }
        }
        finishWatchOver(watched);
      }
      if (discontinuity) {
        feed.status("출력 순서가 이어지지 않아 마지막으로 그린 위치에서 다시 연결한다");
        scheduleReconnect(watched.session, watched.selection);
      }
    };
    parser.onerror = () => {
      frameWorkerBusyRef.current = false;
      const watched = watchRef.current;
      if (watched) {
        feed.status("출력 표시 작업자가 멈춰 마지막으로 그린 위치에서 다시 연결한다");
        scheduleReconnect(watched.session, watched.selection);
      }
    };

    async function begin() {
      try {
        const listeners: Array<() => void> = [];
        try {
          listeners.push(await listen<FrameEnvelope>(FRAME_EVENT, ({ payload }) => {
            const watched = watchRef.current;
            if (!watched || payload.session !== watched.session || payload.view !== watched.view) {
              return;
            }
            enqueueFrame(payload);
            trace("frame queued");
          }));
          listeners.push(await listen<WatchOver>(OVER_EVENT, ({ payload }) => {
            const watched = watchRef.current;
            if (!watched || payload.session !== watched.session || payload.view !== watched.view) {
              return;
            }
            watched.over = payload;
            finishWatchOver(watched);
          }));
        } catch (error) {
          listeners.forEach((stop) => stop());
          throw error;
        }
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
      frameQueueCharactersRef.current = 0;
      frameWorkerBusyRef.current = false;
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
      parser.terminate();
      if (frameWorkerRef.current === parser) {
        frameWorkerRef.current = null;
      }
      const intent = watchRef.current?.intent;
      if (intent !== null && intent !== undefined) {
        void invoke<Answered<null>>("stop_watch", { intent });
      }
      selectionRef.current += 1;
      watchRef.current = null;
      unlisten.forEach((stop) => stop());
    };
  }, [enqueueFrame, feed, finishWatchOver, refresh, scheduleFrameFlush, scheduleReconnect, trace]);

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
    setProvider(preferredProvider(offered, selectedRow?.provider));
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
      rememberProvider(provider);
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
    const selection = watchRef.current?.selection;
    if (!session || selection === undefined) {
      return;
    }
    const closedAnswer = await askForSelection<null>(selection, "close", { session, now: false });
    if (!closedAnswer) {
      return;
    }
    const closed = applyAnswer(closedAnswer);
    if (closed === undefined) {
      return;
    }
    const watched = watchRef.current;
    if (!watched || watched.session !== session || watched.selection !== selection) {
      return;
    }
    if (watched.intent !== null) {
      void invoke<Answered<null>>("stop_watch", { intent: watched.intent });
    }
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    selectionRef.current += 1;
    watchRef.current = null;
    selectedRef.current = null;
    setSelected(null);
    feed.clear();
    await refresh();
  }, [applyAnswer, askForSelection, feed, refresh]);

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
          usage={usage}
          rateLimit={rateLimit}
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
