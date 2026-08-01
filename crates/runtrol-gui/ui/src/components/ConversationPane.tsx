import {
  Badge,
  Button,
  ChatComposer,
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
import type { ConversationItem, SessionRow } from "../domain";
import type { ConversationFeed } from "../frames";
import { AgentIcon, CloseIcon } from "../icons";

type ConversationPaneProps = {
  row: SessionRow | null;
  feed: ConversationFeed;
  draft: string;
  sending: boolean;
  brandLight: string;
  brandDark: string;
  onDraftChange: (value: string) => void;
  onSend: (value: string) => void;
  onClose: () => void;
  onStart: () => void;
};

const MAX_RENDERED_ITEMS = 48;

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
  brandLight,
  brandDark,
  onDraftChange,
  onSend,
  onClose,
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

  const statusLabel = row.looksStuck ? `${row.doing}, 응답이 없다` : row.doing;
  return (
    <section className="conversation" aria-label={`${row.folder} 세션`} data-testid="conversation-pane">
      <header className="conversation-header">
        <div className="conversation-title">
          <Text type="large" weight="semibold" as="h1" maxLines={1}>{row.folder}</Text>
          <Badge label={row.provider} variant="neutral" />
        </div>
        <div className="conversation-status">
          <StatusDot
            variant={row.looksStuck ? "warning" : row.hot ? "success" : "neutral"}
            label={statusLabel}
            isPulsing={row.hot && row.doing !== "idle"}
          />
          <Text type="supporting">{statusLabel}</Text>
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
            <ChatComposer
              value={draft}
              onChange={onDraftChange}
              onSubmit={onSend}
              placeholder="무엇이든 요청해 보세요"
              isDisabled={sending}
              status={sending ? { type: "warning", message: "요청을 전달하고 있다" } : undefined}
              headerContext={
                <span className="composer-context" title={row.workspace}>
                  <Text type="supporting" maxLines={1}>{row.native ?? "첫 턴 전"}</Text>
                </span>
              }
              footerActions={
                <Button
                  label="세션 닫기"
                  tooltip="세션 닫기"
                  variant="ghost"
                  size="sm"
                  icon={<CloseIcon />}
                  onClick={onClose}
                />
              }
            />
          }
        >
          <ConversationMessages feed={feed} isStreaming={row.hot && row.doing !== "idle"} />
        </ChatLayout>
      </div>
    </section>
  );
}
