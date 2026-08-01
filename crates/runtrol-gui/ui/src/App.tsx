import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppShell, Theme } from "@astryxdesign/core";
import { neutralTheme } from "@astryxdesign/theme-neutral";
import brandLight from "../../../../assets/brand/lockup-light.svg";
import brandDark from "../../../../assets/brand/lockup-dark.svg";
import { FRAME_EVENT, OVER_EVENT, REFRESH_MS, invoke, listen } from "./bridge";
import type {
  Answered,
  ConversationItem,
  FrameEnvelope,
  Notice,
  OfferedProvider,
  SessionRow,
  ThemeMode,
} from "./domain";
import { appendFrame, appendStatus, frameToItem } from "./frames";
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
  const [items, setItems] = useState<ConversationItem[]>([]);
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState("");
  const [reachable, setReachable] = useState(true);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [theme, setTheme] = useState<ThemeMode>(initialTheme);
  const [startOpen, setStartOpen] = useState(false);
  const [providers, setProviders] = useState<OfferedProvider[]>([]);
  const [provider, setProvider] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [starting, setStarting] = useState(false);
  const [sending, setSending] = useState(false);

  const selectedRef = useRef<string | null>(null);
  const tracingRef = useRef(false);
  const refreshingRef = useRef(false);
  const firstDrawRef = useRef(true);
  const startedAtRef = useRef(performance.now());

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

  const openSession = useCallback(async (session: string) => {
    if (selectedRef.current === session) {
      return;
    }
    selectedRef.current = session;
    setSelected(session);
    setItems([]);
    const watched = await ask<null>("watch", { session });
    trace(`watching ${session} ${watched === undefined ? "refused" : "ok"}`);
  }, [ask, trace]);

  const refresh = useCallback(async () => {
    if (refreshingRef.current) {
      return;
    }
    refreshingRef.current = true;
    const askedAt = performance.now();
    try {
      const nextRows = await ask<SessionRow[]>("sessions");
      if (!nextRows) {
        return;
      }
      setRows(nextRows);
      const current = selectedRef.current;
      if (current && !nextRows.some((row) => row.session === current)) {
        selectedRef.current = null;
        setSelected(null);
        setItems([]);
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
        await openSession(nextRows[0].session);
      }
    } finally {
      refreshingRef.current = false;
    }
  }, [ask, openSession, trace]);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let unlisten: Array<() => void> = [];

    async function begin() {
      try {
        const listeners = await Promise.all([
          listen<FrameEnvelope>(FRAME_EVENT, ({ payload }) => {
            if (payload.session !== selectedRef.current) {
              return;
            }
            const next = frameToItem(payload.frame);
            setItems((current) => appendFrame(current, next.item, next.isDelta));
            trace(`frame ${next.item.side}`);
          }),
          listen<string>(OVER_EVENT, ({ payload }) => {
            if (payload !== selectedRef.current) {
              return;
            }
            setItems((current) => appendStatus(current, "이 세션의 흐름이 끝났다"));
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
      unlisten.forEach((stop) => stop());
    };
  }, [refresh, trace]);

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
    setProvider(offered.find((entry) => entry.usable)?.id ?? "");
    if (!workspace && selectedRow) {
      setWorkspace(selectedRow.workspace);
    }
    setStartOpen(true);
  }, [ask, selectedRow, workspace]);

  const startSession = useCallback(async () => {
    const where = workspace.trim();
    if (!provider || !where) {
      setNotice({ kind: "broken", message: "공급자와 작업 폴더가 모두 필요하다." });
      return;
    }
    setStarting(true);
    try {
      const session = await ask<string>("start", { provider, workspace: where });
      if (!session) {
        return;
      }
      setStartOpen(false);
      await refresh();
      await openSession(session);
    } finally {
      setStarting(false);
    }
  }, [ask, openSession, provider, refresh, workspace]);

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
    setItems([]);
    await refresh();
  }, [ask, refresh]);

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
            onSelect={(session) => void openSession(session)}
            onStart={() => void openStart()}
            onToggleTheme={() => setTheme((current) => current === "dark" ? "light" : "dark")}
          />
        }
      >
        <ConversationPane
          row={selectedRow}
          items={items}
          draft={draft}
          sending={sending}
          brandLight={brandLight}
          brandDark={brandDark}
          onDraftChange={setDraft}
          onSend={(text) => void send(text)}
          onClose={() => void closeSession()}
          onStart={() => void openStart()}
        />
        <StartSessionDialog
          isOpen={startOpen}
          providers={providers}
          provider={provider}
          workspace={workspace}
          starting={starting}
          onOpenChange={setStartOpen}
          onProviderChange={setProvider}
          onWorkspaceChange={setWorkspace}
          onStart={() => void startSession()}
        />
        {notice ? <NoticeCard notice={notice} onClose={() => setNotice(null)} /> : null}
      </AppShell>
    </Theme>
  );
}
