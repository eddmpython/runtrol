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

export function uniqueChatTitle(
  session: SessionLine,
  sessions: readonly SessionLine[],
): string {
  const title = session.label?.trim() || workspaceName(session.workspace);
  const duplicates = sessions.filter(
    (candidate) => (candidate.label?.trim() || workspaceName(candidate.workspace)) === title,
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

/// The glyph that stands for a coding service, by the name the editor knows it under.
///
/// Read from what the service declared and carried here through the protocol, never chosen by a table in this
/// file. A table here would mean adding a coding service required editing this extension, and the whole point of
/// a manifest is that it does not.
///
/// The fallback is the glyph the editor itself uses for a chat provider it has no mark for, so a service the
/// editor does not know still reads as a coding service rather than as a missing image.
export function providerIcon(provider: string, providers: readonly ProviderLine[] = []): string {
  return providers.find((candidate) => candidate.providerId === provider)?.icon || "sparkle";
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
