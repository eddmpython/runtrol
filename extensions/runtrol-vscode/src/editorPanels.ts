/// Which coding services own an editor surface of their own in this editor, and how to reveal a conversation
/// there. Data, not behaviour: a provider is added by adding a row, and nothing here branches on one.
///
/// A conversation living in a service's own editor panel has no terminal to attach to (its process talks to
/// that extension over a private pipe), so the one useful thing a click can do is put the person in front of
/// the real surface. The commands are the other extension's public contributions; whether that extension is
/// installed is the caller's question, asked through `installed` so this stays a table.

type EditorPanel = {
  /// The editor extension that owns the surface, as `publisher.name`.
  readonly extension: string;
  /// Its command that opens or reveals one conversation, taking the provider-native session id.
  readonly reveal: string;
};

const EDITOR_PANELS: Readonly<Record<string, EditorPanel>> = {
  claude: { extension: "anthropic.claude-code", reveal: "claude-vscode.editor.open" },
};

/// The installed editor surface for this service, or null when the service has none or it is not installed.
export function editorPanelFor(
  providerId: string,
  installed: (extension: string) => boolean,
): { readonly reveal: string } | null {
  const panel = EDITOR_PANELS[providerId];
  if (!panel || !installed(panel.extension)) return null;
  return { reveal: panel.reveal };
}
