export type Answered<T> =
  | { outcome: "ok"; value: T }
  | {
      outcome: "refused";
      message: string;
      needsTheOperator: boolean;
      retryable: boolean;
    }
  | { outcome: "broken"; message: string };

export type SessionRow = {
  session: string;
  provider: string;
  native: string | null;
  workspace: string;
  folder: string;
  hot: boolean;
  doing: string;
  looksStuck: boolean;
};

export type OfferedProvider = {
  id: string;
  displayName: string;
  usable: boolean;
  whyNot: string | null;
};

export type ConversationItem = {
  key: number;
  side: "mine" | "theirs" | "thought" | "meta";
  label: string;
  text: string;
  messageId: string | null;
};

export type FrameEnvelope = {
  session: string;
  frame: string;
};

export type Notice = {
  kind: "refused" | "broken";
  message: string;
};

export type ThemeMode = "light" | "dark";
