import {
  Badge,
  Button,
  ChatComposer,
  ChatSendButton,
  ChatLayout,
  ChatMessage,
  ChatMessageBubble,
  ChatMessageList,
  ChatSystemMessage,
  EmptyState,
  StatusDot,
  Text,
} from "@astryxdesign/core";
import { memo, useEffect, useRef, useSyncExternalStore } from "react";
import type {
  ConversationItem,
  LimitWindow,
  RateLimitGauge,
  SessionRow,
  UsageGauge,
} from "../domain";
import type { ConversationFeed } from "../frames";
import { AgentIcon, CloseIcon } from "../icons";

type RenderCheckpoint = {
  id: string;
  view: number;
  seq: number;
  items: number;
  characters: number;
};

type ConversationPaneProps = {
  row: SessionRow | null;
  feed: ConversationFeed;
  checkpoint: RenderCheckpoint | null;
  draft: string;
  sending: boolean;
  preparing: boolean;
  usage: UsageGauge | null;
  rateLimit: RateLimitGauge | null;
  /** Provider frames runtrol relayed without reading. Reported as a count, never as conversation. */
  unreadFrames: number;
  brandLight: string;
  brandDark: string;
  onDraftChange: (value: string) => void;
  onSend: (value: string) => void;
  onRemove: () => void;
  onInputTrace: (line: string) => void;
  onStart: () => void;
};

const MAX_RENDERED_ITEMS = 48;
const COMPOSITION_COMMIT_ENTER_GUARD_MS = 80;
const counts = new Intl.NumberFormat("ko-KR", { maximumFractionDigits: 2 });

function usageText(usage: UsageGauge): string {
  const context = usage.used !== null && usage.size !== null
    ? `문맥 ${counts.format(usage.used)} / ${counts.format(usage.size)} 토큰`
    : usage.used !== null
      ? `문맥 ${counts.format(usage.used)} 토큰 사용`
      : usage.size !== null
        ? `문맥 한도 ${counts.format(usage.size)} 토큰`
        : "문맥 사용량 수치 없음";
  return usage.cost
    ? `${context} · ${counts.format(usage.cost.amount)} ${usage.cost.currency}`
    : context;
}

function resetText(resetsAt: number | null): string {
  if (resetsAt === null) {
    return "";
  }
  const remainingMinutes = Math.ceil((resetsAt - Date.now()) / 60_000);
  if (remainingMinutes <= 0) {
    return " · 재설정 시각 지남";
  }
  if (remainingMinutes < 60) {
    return ` · ${remainingMinutes}분 후 재설정`;
  }
  if (remainingMinutes < 24 * 60) {
    const hours = Math.floor(remainingMinutes / 60);
    const minutes = remainingMinutes % 60;
    return ` · ${hours}시간${minutes ? ` ${minutes}분` : ""} 후 재설정`;
  }
  return ` · ${new Intl.DateTimeFormat("ko-KR", {
    month: "numeric",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(resetsAt))} 재설정`;
}

function windowText(label: string, window: LimitWindow | null): string | null {
  return window
    ? `${label} ${counts.format(window.usedPercent)}%${resetText(window.resetsAt)}`
    : null;
}

function rateLimitText(rateLimit: RateLimitGauge): string {
  const windows = [
    windowText("단기", rateLimit.primary),
    windowText("장기", rateLimit.secondary),
  ].filter((entry): entry is string => Boolean(entry));
  const detail = windows.length > 0 ? windows.join(" · ") : "한도 수치 없음";
  return rateLimit.reached ? `한도 도달 · ${detail}` : detail;
}

const Message = memo(function Message({ item }: { item: ConversationItem }) {
  if (item.side === "meta") {
    return <ChatSystemMessage><span className="verbatim">{item.text}</span></ChatSystemMessage>;
  }
  const isMine = item.side === "mine";
  return (
    <ChatMessage sender={isMine ? "user" : "assistant"} className={item.side === "thought" ? "thought-row" : undefined}>
      <ChatMessageBubble
        name={item.label}
        variant={item.side === "thought" ? "ghost" : "filled"}
      >
        <span className="verbatim">{item.text}</span>
      </ChatMessageBubble>
    </ChatMessage>
  );
});

function ConversationMessages({
  feed,
  checkpoint,
  isStreaming,
  onTrace,
}: {
  feed: ConversationFeed;
  checkpoint: RenderCheckpoint | null;
  isStreaming: boolean;
  onTrace: (line: string) => void;
}) {
  const items = useSyncExternalStore(feed.subscribe, feed.snapshot);
  const renderedItems = items.slice(-MAX_RENDERED_ITEMS);
  const paintSentinelRef = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!checkpoint) {
      return;
    }
    let paintFrame: number | null = null;
    const commitFrame = requestAnimationFrame(() => {
      paintFrame = requestAnimationFrame(() => {
        const list = paintSentinelRef.current?.parentElement;
        if (!list) {
          return;
        }
        const rendered = Array.from(list.querySelectorAll<HTMLElement>(".verbatim"));
        const characters = rendered.reduce(
          (total, node) => total + (node.textContent?.length ?? 0),
          0,
        );
        onTrace(
          `feed painted checkpoint=${checkpoint.id} view=${checkpoint.view} seq=${checkpoint.seq} items=${rendered.length} characters=${characters}`,
        );
      });
    });
    return () => {
      cancelAnimationFrame(commitFrame);
      if (paintFrame !== null) {
        cancelAnimationFrame(paintFrame);
      }
    };
  }, [checkpoint, items, onTrace]);
  if (renderedItems.length === 0) {
    return null;
  }
  return (
    <ChatMessageList density="balanced" gap={3} isStreaming={isStreaming}>
      {renderedItems.map((entry) => <Message key={entry.key} item={entry} />)}
      <span ref={paintSentinelRef} hidden aria-hidden="true" />
    </ChatMessageList>
  );
}

export function ConversationPane({
  row,
  feed,
  checkpoint,
  draft,
  sending,
  preparing,
  usage,
  rateLimit,
  unreadFrames,
  brandLight,
  brandDark,
  onDraftChange,
  onSend,
  onRemove,
  onInputTrace,
  onStart,
}: ConversationPaneProps) {
  const composingRef = useRef(false);
  const compositionEndedAtRef = useRef(Number.NEGATIVE_INFINITY);
  const commitBreakGuardGenerationRef = useRef<number | null>(null);
  const commitBreakGenerationRef = useRef(0);
  const commitBreakGuardTimerRef = useRef<number | null>(null);
  const composerHostRef = useRef<HTMLDivElement>(null);
  const onInputTraceRef = useRef(onInputTrace);
  onInputTraceRef.current = onInputTrace;
  useEffect(() => {
    const host = composerHostRef.current;
    if (!host) {
      return;
    }
    let editable: HTMLElement | null = null;
    const resetInputLifecycle = () => {
      composingRef.current = false;
      compositionEndedAtRef.current = Number.NEGATIVE_INFINITY;
      commitBreakGenerationRef.current += 1;
      commitBreakGuardGenerationRef.current = null;
      if (commitBreakGuardTimerRef.current !== null) {
        window.clearTimeout(commitBreakGuardTimerRef.current);
        commitBreakGuardTimerRef.current = null;
      }
    };
    const blockCommitBreak = (event: Event) => {
      const inputEvent = event as InputEvent;
      const generation = commitBreakGuardGenerationRef.current;
      if (generation === null || event.target !== editable || !inputEvent.cancelable ||
        (inputEvent.inputType !== "insertParagraph" && inputEvent.inputType !== "insertLineBreak")) {
        return;
      }
      inputEvent.preventDefault();
      if (!inputEvent.defaultPrevented || commitBreakGuardGenerationRef.current !== generation) {
        return;
      }
      commitBreakGuardGenerationRef.current = null;
      if (commitBreakGuardTimerRef.current !== null) {
        window.clearTimeout(commitBreakGuardTimerRef.current);
        commitBreakGuardTimerRef.current = null;
      }
      onInputTraceRef.current("composer composition commit break blocked");
    };
    const attachEditable = () => {
      const next = host.querySelector<HTMLElement>('[contenteditable="true"]');
      if (next === editable) {
        return;
      }
      resetInputLifecycle();
      editable?.removeEventListener("beforeinput", blockCommitBreak, { capture: true });
      editable = next;
      editable?.addEventListener("beforeinput", blockCommitBreak, { capture: true });
    };
    attachEditable();
    const observer = new MutationObserver(attachEditable);
    observer.observe(host, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      editable?.removeEventListener("beforeinput", blockCommitBreak, { capture: true });
      editable = null;
      resetInputLifecycle();
    };
  }, [row?.session]);
  if (!row) {
    return (
      <section className="welcome" aria-label="시작">
        <picture>
          <source srcSet={brandDark} media="(prefers-color-scheme: dark)" />
          <img src={brandLight} alt="runtrol" />
        </picture>
        <Text type="display-2" as="h1">무엇을 시킬까요?</Text>
        <Text type="supporting" as="p">로컬 CLI 세션을 한곳에서 열고 이어서 작업합니다.</Text>
        <Button label="새 세션 시작" variant="primary" icon={<AgentIcon />} onClick={onStart} />
      </section>
    );
  }

  const statusLabel = preparing
    ? "공급자 준비 중"
    : row.looksStuck
      ? `${row.doing}, 응답이 없다`
      : row.doing;
  const submitLocked = preparing || sending;
  return (
    <section className="conversation" aria-label={`${row.folder} 세션`} data-testid="conversation-pane">
      <header className="conversation-header">
        <div className="conversation-title">
          <Text type="large" weight="semibold" as="h1" maxLines={1}>{row.folder}</Text>
          <Badge label={row.provider} variant="neutral" />
        </div>
        <div className="conversation-meta">
          <div className="conversation-metrics">
            {usage ? (
              <span className="metric" data-testid="usage-status" title="현재 문맥 사용량">
                {usageText(usage)}
              </span>
            ) : null}
            {rateLimit ? (
              <span
                className="metric"
                data-reached={rateLimit.reached}
                data-testid="rate-limit-status"
                title="공급자 계정 한도"
              >
                {rateLimitText(rateLimit)}
              </span>
            ) : null}
            {unreadFrames > 0 ? (
              <span
                className="metric"
                data-testid="unread-frames"
                title="공급자가 보냈고 runtrol 이 해석하지 않는 프레임입니다. 대화가 아니므로 본문에 그리지 않고 수만 보여줍니다."
              >
                미해석 프레임 {counts.format(unreadFrames)}
              </span>
            ) : null}
          </div>
          <div className="conversation-status">
            <StatusDot
              variant={preparing || row.looksStuck ? "warning" : row.hot ? "success" : "neutral"}
              label={statusLabel}
              isPulsing={preparing || (row.hot && row.doing !== "idle")}
            />
            <Text type="supporting">{statusLabel}</Text>
          </div>
        </div>
      </header>
      <div className="conversation-body">
        <ChatLayout
          density="balanced"
          emptyState={
            <EmptyState
              title="이 세션의 새 흐름"
              description="프롬프트를 보내면 공급자 CLI가 내보내는 이벤트가 여기에 표시됩니다."
              icon={<AgentIcon />}
            />
          }
          composer={
            <div
              ref={composerHostRef}
              onCompositionStartCapture={() => {
                composingRef.current = true;
                compositionEndedAtRef.current = Number.NEGATIVE_INFINITY;
                commitBreakGenerationRef.current += 1;
                commitBreakGuardGenerationRef.current = null;
                if (commitBreakGuardTimerRef.current !== null) {
                  window.clearTimeout(commitBreakGuardTimerRef.current);
                  commitBreakGuardTimerRef.current = null;
                }
                onInputTrace("composer composition started");
              }}
              onCompositionUpdateCapture={() => onInputTrace("composer composition updated")}
              onCompositionEndCapture={() => {
                composingRef.current = false;
                compositionEndedAtRef.current = performance.now();
                onInputTrace("composer composition ended");
              }}
              onCopyCapture={() => onInputTrace("composer copied selection")}
              onKeyDownCapture={(event) => {
                if (event.key !== "Enter" || event.shiftKey) {
                  return;
                }
                if (composingRef.current || event.nativeEvent.isComposing) {
                  onInputTrace("composer composing enter blocked");
                  event.stopPropagation();
                  return;
                }
                if (performance.now() - compositionEndedAtRef.current <= COMPOSITION_COMMIT_ENTER_GUARD_MS) {
                  commitBreakGenerationRef.current += 1;
                  const generation = commitBreakGenerationRef.current;
                  commitBreakGuardGenerationRef.current = generation;
                  if (commitBreakGuardTimerRef.current !== null) {
                    window.clearTimeout(commitBreakGuardTimerRef.current);
                  }
                  onInputTrace("composer composition commit enter blocked");
                  event.stopPropagation();
                  const timer = window.setTimeout(() => {
                    if (commitBreakGuardGenerationRef.current === generation) {
                      commitBreakGuardGenerationRef.current = null;
                    }
                    if (commitBreakGuardTimerRef.current === timer) {
                      commitBreakGuardTimerRef.current = null;
                    }
                  }, 0);
                  commitBreakGuardTimerRef.current = timer;
                  return;
                }
                if (submitLocked) {
                  event.preventDefault();
                  event.stopPropagation();
                }
              }}
            >
              <ChatComposer
                value={draft}
                onChange={onDraftChange}
                onSubmit={onSend}
                placeholder="무엇이든 요청해 보세요"
                sendButton={<ChatSendButton isDisabled={submitLocked || !draft.trim()} />}
                status={preparing
                  ? { type: "warning", message: "공급자를 준비 중이다. 작성한 내용은 보존된다" }
                  : sending
                    ? { type: "warning", message: "요청을 전달하고 있다. 계속 입력할 수 있다" }
                    : undefined}
                headerContext={
                  // Where the agent works, which is what a person needs to see before they send an
                  // instruction. The provider's own identifier for the conversation used to sit here as a
                  // raw UUID: it is machine data, it told nobody anything, and it is one hover away.
                  <span
                    className="composer-context"
                    title={`${row.workspace}\n${row.native ?? "이 공급자는 첫 턴 전까지 대화를 만들지 않는다"}`}
                  >
                    <Text type="supporting" maxLines={1}>{row.trail}</Text>
                  </span>
                }
                footerActions={
                  <Button
                    label="목록에서 삭제"
                    tooltip="runtrol 목록에서 삭제"
                    variant="ghost"
                    size="sm"
                    icon={<CloseIcon />}
                    onClick={onRemove}
                  />
                }
              />
            </div>
          }
        >
          <ConversationMessages
            feed={feed}
            checkpoint={checkpoint}
            isStreaming={row.hot && row.doing !== "idle"}
            onTrace={onInputTrace}
          />
        </ChatLayout>
      </div>
    </section>
  );
}
