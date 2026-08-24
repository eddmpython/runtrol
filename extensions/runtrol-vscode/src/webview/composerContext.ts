export type ComposerContextKind = "Project" | "Branch" | "Agent";

/// The visible target above the message field. The label is intentional: a project whose name matches the product
/// must still read as a project, and an agent must not look like another folder or branch choice.
export function composerContextLabel(kind: ComposerContextKind, value: string): string {
  return value ? `${kind}: ${value}` : "";
}
