import type { OfferedProvider } from "./domain";

const LAST_PROVIDER = "runtrol.lastProvider";

function storedProvider(): string | null {
  try {
    return window.localStorage.getItem(LAST_PROVIDER);
  } catch (error) {
    console.warn("cannot read the last provider preference", error);
    return null;
  }
}

export function preferredProvider(
  providers: readonly OfferedProvider[],
  currentProvider?: string,
): string {
  const remembered = storedProvider();
  return providers.find((entry) => entry.usable && entry.id === remembered)?.id
    ?? providers.find((entry) => entry.usable && entry.id === currentProvider)?.id
    ?? providers.find((entry) => entry.usable)?.id
    ?? "";
}

export function rememberProvider(provider: string): void {
  try {
    window.localStorage.setItem(LAST_PROVIDER, provider);
  } catch (error) {
    // A locked-down webview can deny storage. Starting the provider session still takes precedence.
    console.warn("cannot save the last provider preference", error);
  }
}
