import path from "node:path";

import { workspaceCovers } from "./workspaceCollision";

/// Where a conversation that belongs to no project runs.
///
/// A coding CLI always runs somewhere: every one of them takes a working directory and files its
/// transcript under it. So "a chat with no project" is not a chat with no folder, it is a chat whose
/// folder the person did not choose. That folder is this one, inside the extension's own global
/// storage: per user, persistent (a temp directory is swept, and a swept folder makes the saved
/// conversation unresumable: measured 2026-08-18 on this machine, both opencode sessions sat in deleted
/// temp folders and could not be reopened), and outside every credential directory the security wall refuses.
///
/// One folder rather than one per chat. A person who starts three quick questions has started three
/// chats, not three projects, and the tree treats everything under this root as the plain rows at the
/// bottom of the list: never a heading, never repeated in a row's detail. The start path also skips the
/// writer-collision question for it, because two agents answering unrelated questions in the scratch
/// folder are not two agents editing one repository.
const PROJECTLESS_DIRECTORY = "no-project";

/// The scratch folder for conversations without a project, given the extension's global storage path.
export function projectlessRoot(globalStorage: string): string {
  return path.join(globalStorage, PROJECTLESS_DIRECTORY);
}

/// Whether a conversation's folder is the scratch folder (or inside it), which is what "no project" means.
///
/// A null root means the surface has no scratch folder at all (unit tests, or a build without global
/// storage), and then nothing is projectless by this rule.
export function isProjectless(workspace: string, root: string | null): boolean {
  if (root === null || !workspace.trim()) return false;
  return workspaceCovers(root, workspace);
}
