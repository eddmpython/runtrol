import type {
  CatalogueCoverage,
  ListNativeSessionsParams,
  NativeSessionCatalogue,
} from "@runtrol/runtime-client";

import type { NativeChatCatalogue, NativeChatLine } from "./runtimeTypes";

const MAX_NATIVE_CATALOGUE_PAGES = 32;
const MAX_NATIVE_CHAT_ROWS = 500;

export type NativeCatalogueReader = {
  listNativeSessions(params: ListNativeSessionsParams): Promise<NativeSessionCatalogue>;
};

export async function collectNativeChats(
  reader: NativeCatalogueReader,
  providerId: string,
  /// Where to look. A null entry asks the provider about the whole machine.
  roots: readonly (string | null)[],
  now: () => number = Date.now,
  signal?: AbortSignal,
): Promise<NativeChatCatalogue> {
  const chats = new Map<string, NativeChatLine>();
  const coverages: CatalogueCoverage[] = [];
  const limitations: string[] = [];
  let pageLimitReached = false;
  let rowLimitReached = false;

  roots: for (const root of roots) {
    signal?.throwIfAborted();
    let cursor: string | null = null;
    const seenCursors = new Set<string>();
    for (let page = 0; page < MAX_NATIVE_CATALOGUE_PAGES; page += 1) {
      signal?.throwIfAborted();
      let catalogue: NativeSessionCatalogue;
      try {
        catalogue = await reader.listNativeSessions({
          providerId,
          // Omitted, not null: absence is what asks for the machine, and a provider that can only
          // answer about one folder refuses it by name so the caller can ask per folder instead.
          ...(root === null ? {} : { root }),
          ...(cursor ? { cursor } : {}),
        });
      } catch (error) {
        if (signal?.aborted) signal.throwIfAborted();
        limitations.push(
          root === null
            ? `Discovery across this machine failed: ${errorMessage(error)}`
            : `Discovery under ${root} failed: ${errorMessage(error)}`,
        );
        continue roots;
      }
      signal?.throwIfAborted();
      coverages.push(catalogue.coverage);
      if (catalogue.coverage.kind !== "complete") {
        limitations.push(catalogue.coverage.why);
      }
      for (const session of catalogue.sessions) {
        chats.set(`${providerId}\0${session.nativeSessionId}`, { ...session, providerId });
        if (chats.size >= MAX_NATIVE_CHAT_ROWS) {
          rowLimitReached = true;
          break roots;
        }
      }
      const next = catalogue.nextCursor ?? null;
      if (!next) break;
      if (seenCursors.has(next)) {
        limitations.push("The provider repeated a native chat catalogue cursor.");
        continue roots;
      }
      seenCursors.add(next);
      cursor = next;
      if (page === MAX_NATIVE_CATALOGUE_PAGES - 1) {
        pageLimitReached = true;
      }
    }
  }
  if (rowLimitReached) {
    limitations.push(`Only the first ${MAX_NATIVE_CHAT_ROWS} existing chats are shown.`);
  }
  if (pageLimitReached) {
    limitations.push(`Existing chat discovery stopped after ${MAX_NATIVE_CATALOGUE_PAGES} pages per scope.`);
  }
  const warning = uniqueText(limitations).join(" ") || null;
  return {
    providerId,
    coverage: combinedCoverage(coverages, warning),
    chats: [...chats.values()],
    loadedAtMs: now(),
    warning,
  };
}

function combinedCoverage(
  coverages: readonly CatalogueCoverage[],
  warning: string | null,
): CatalogueCoverage | null {
  if (coverages.length === 0) return null;
  const supported = coverages.find(
    (coverage): coverage is Exclude<CatalogueCoverage, { kind: "unsupported" }> => (
      coverage.kind !== "unsupported"
    ),
  );
  if (!supported) {
    return coverages[0] ?? null;
  }
  if (!warning && coverages.every((coverage) => coverage.kind === "complete")) {
    return { kind: "complete", source: supported.source };
  }
  return {
    kind: "partial",
    source: supported.source,
    why: warning ?? "Existing chat discovery is structurally limited.",
  };
}

function uniqueText(values: readonly string[]): string[] {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))];
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
