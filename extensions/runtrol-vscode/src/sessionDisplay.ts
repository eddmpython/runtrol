import type { ProviderLine, SessionLine } from "./runtimeTypes";

type SessionNameSource = Pick<SessionLine, "providerId" | "label" | "workspace">;

export function sessionTitle(
  session: SessionNameSource,
  providerName = readableProvider(session.providerId),
): string {
  const label = session.label?.trim();
  return label || `${workspaceName(session.workspace)} · ${providerName}`;
}

export function uniqueSessionTitle(
  session: SessionLine,
  sessions: readonly SessionLine[],
  providers: readonly ProviderLine[] = [],
): string {
  const providerName = providerDisplayName(session.providerId, providers);
  const title = sessionTitle(session, providerName);
  const duplicates = sessions.filter(
    (candidate) => sessionTitle(
      candidate,
      providerDisplayName(candidate.providerId, providers),
    ) === title,
  );
  return duplicates.length > 1 ? `${title} · #${shortIdentity(session)}` : title;
}

export function sessionContext(
  session: SessionLine,
  providers: readonly ProviderLine[] = [],
): string {
  const provider = providerDisplayName(session.providerId, providers);
  return `${workspaceName(session.workspace)} · ${provider}`;
}

export function providerDisplayName(provider: string, providers: readonly ProviderLine[] = []): string {
  return providers.find((candidate) => candidate.providerId === provider)?.displayName ?? readableProvider(provider);
}

export function workspaceName(workspace: string): string {
  const parts = workspace.replaceAll("\\", "/").split("/").filter(Boolean);
  return parts.at(-1) ?? workspace;
}

function shortIdentity(session: SessionLine): string {
  const identity = session.nativeSessionId || session.sessionId;
  const compact = identity.replaceAll(/[^A-Za-z0-9]/g, "");
  return (compact.slice(-6) || identity.slice(-6)).toUpperCase();
}

function readableProvider(provider: string): string {
  return provider
    .split(/[-_.]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toLocaleUpperCase("en-US") + part.slice(1))
    .join(" ") || provider;
}
