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
import { memo, useSyncExternalStore } from "react";
import type {
  ConversationItem,
  LimitWindow,
  RateLimitGauge,
  SessionRow,
  UsageGauge,
} from "../domain";
import type { ConversationFeed } from "../frames";
import { AgentIcon, CloseIcon } from "../icons";

type ConversationPaneProps = {
  row: SessionRow | null;
  feed: ConversationFeed;
  draft: string;
  sending: boolean;
  preparing: boolean;
  usage: UsageGauge | null;
  rateLimit: RateLimitGauge | null;
  brandLight: string;
  brandDark: string;
  onDraftChange: (value: string) => void;
  onSend: (value: string) => void;
  onRemove: () => void;
  onStart: () => void;
};

const MAX_RENDERED_ITEMS = 48;
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
    return <ChatSystemMessage>{item.text}</ChatSystemMessage>;
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

function ConversationMessages({ feed, isStreaming }: { feed: ConversationFeed; isStreaming: boolean }) {
  const items = useSyncExternalStore(feed.subscribe, feed.snapshot);
  const renderedItems = items.slice(-MAX_RENDERED_ITEMS);
  if (renderedItems.length === 0) {
    return null;
  }
  return (
    <ChatMessageList density="balanced" gap={3} isStreaming={isStreaming}>
      {renderedItems.map((entry) => <Message key={entry.key} item={entry} />)}
    </ChatMessageList>
  );
}

export function ConversationPane({
  row,
  feed,
  draft,
  sending,
  preparing,
  usage,
  rateLimit,
  brandLight,
  brandDark,
  onDraftChange,
  onSend,
  onRemove,
  onStart,
}: ConversationPaneProps) {
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
              onKeyDownCapture={(event) => {
                if (event.key !== "Enter" || event.shiftKey) {
                  return;
                }
                if (event.nativeEvent.isComposing) {
                  event.stopPropagation();
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
                  <span className="composer-context" title={row.workspace}>
                    <Text type="supporting" maxLines={1}>{row.native ?? "첫 턴 전"}</Text>
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
          <ConversationMessages feed={feed} isStreaming={row.hot && row.doing !== "idle"} />
        </ChatLayout>
      </div>
    </section>
  );
}
