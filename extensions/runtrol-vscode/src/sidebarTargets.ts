/// What a sidebar command is handed when it is invoked from a row.
///
/// The sidebar is a page Studio draws itself (`sidebarPage.ts`), so a row cannot pass a tree item. It passes one of
/// these instead: a plain object naming the project group, the conversation, or the service the person pressed.
/// Every command keeps its `instanceof` guard, which is what makes an invocation with the wrong thing refuse rather
/// than act on something surprising.

import type { Conversation, ProjectGroup } from "./conversationList";

export class ConversationItem {
  constructor(readonly conversation: Conversation) {}
}

export class ProjectItem {
  constructor(readonly group: ProjectGroup) {}
}

export class ServiceChoiceItem {
  constructor(readonly providerId: string, readonly workspace: string) {}
}
