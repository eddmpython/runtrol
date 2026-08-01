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

export type SessionListing = {
  sessions: SessionRow[];
  warnings: string[];
};

export type OfferedProvider = {
  id: string;
  displayName: string;
  usable: boolean;
  whyNot: string | null;
};

export type ReasoningChoice = {
  id: string;
  description: string;
};

export type ModelChoice = {
  id: string;
  displayName: string;
  description: string;
  isDefault: boolean;
  reasoningEfforts: ReasoningChoice[];
};

export type ModelCatalog =
  | { kind: "known"; models: ModelChoice[] }
  | { kind: "aliases"; aliases: string[]; why: string }
  | { kind: "unknown"; why: string };

export type ConversationItem = {
  key: number;
  side: "mine" | "theirs" | "thought" | "meta";
  label: string;
  text: string;
  messageId: string | null;
};

export type UsageGauge = {
  used: number | null;
  size: number | null;
  cost: { amount: number; currency: string } | null;
};

export type LimitWindow = {
  usedPercent: number;
  resetsAt: number | null;
  windowMinutes: number | null;
};

export type RateLimitGauge = {
  primary: LimitWindow | null;
  secondary: LimitWindow | null;
  reached: boolean;
};

export type FrameEnvelope = {
  session: string;
  frame: string;
};

export type Notice = {
  kind: "warning" | "refused" | "broken";
  message: string;
};

export type ThemeMode = "light" | "dark";
