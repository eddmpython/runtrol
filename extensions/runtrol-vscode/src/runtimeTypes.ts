export type {
  CatalogueCoverage,
  EventCursor as WatchCursor,
  NativeSessionDescriptor,
  ProviderDescriptor as ProviderLine,
  ProviderUsageCost,
  ProviderUsageGauge,
  ProviderUsageList,
  ProviderUsageWindow,
  RuntimeModelCatalog as ModelCatalog,
  RuntimeModelChoice as ModelChoice,
  RuntimeProviderCapabilities as ProviderCapabilities,
  RuntimeReasoningChoice as ReasoningChoice,
  SessionDescriptor as SessionLine,
  SessionWorkspaceAccess as WorkspaceAccess,
} from "@runtrol/runtime-client";

import type {
  CatalogueCoverage,
  NativeSessionDescriptor,
} from "@runtrol/runtime-client";

export type NativeChatLine = NativeSessionDescriptor & {
  providerId: string;
};

export type NativeChatCatalogue = {
  providerId: string;
  coverage: CatalogueCoverage | null;
  chats: readonly NativeChatLine[];
  loadedAtMs: number;
  warning: string | null;
};
