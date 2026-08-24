// The one vocabulary of actions the conversation Webview may ask the Extension Host to perform.
//
// This module is deliberately free of any `vscode` import so the validator can be unit-tested in
// plain Node. Every message that crosses the Webview boundary is hostile until it passes
// `isViewAction`: the page runs remote-rendered conversation content, so an action name that the
// dispatcher does not explicitly handle must be dropped, never defaulted into another action.

import { PUBLIC_LIMITS } from "@runtrol/runtime-client";

import { type DeclaredDiff, isDeclaredDiff } from "./webview/toolDiff";

export type ViewAction =
  | { type: "prompt"; text: string }
  | { type: "answerApproval"; approval: string; option: number; subjectDigest: number[] }
  // `model` and `effort` are what the page says is answering, so the one menu can offer that
  // model's efforts beside the models themselves and mark the current pair (one control, the way
  // the Codex and ChatGPT composers do it).
  | { type: "switchModel"; available: string[]; model: string; effort: string }
  | { type: "switchMode"; available: string[] }
  | { type: "switchEffort"; model: string }
  | { type: "pickProject" }
  | { type: "pickService" }
  | { type: "attach" }
  | { type: "pasteImage"; name: string; mediaType: string; base64Data: string }
  | { type: "removeAttachment"; index: number }
  | { type: "mentionFile" }
  | { type: "openDiff"; diff: DeclaredDiff }
  | { type: "menuChoice"; menu: string; choice: string | null }
  | { type: "interrupt" };

/// A choice offered in the composer's own popover, where the chip was clicked.
///
/// A heading row groups the rows after it (the model menu carries its efforts under one) and is
/// skipped by the keys; `current` marks the row whose value is answering right now; `icon` names
/// the provider whose shipped mark the row shows, resolved to a Webview URI by the view that posts
/// the menu (the page receives the URI in this same field).
export type MenuItem = {
  readonly id: string;
  readonly label: string;
  readonly description?: string;
  readonly detail?: string;
  readonly icon?: string;
  readonly heading?: boolean;
  readonly current?: boolean;
};

/// Which chip a popover hangs from.
export type MenuAnchor = "project" | "service" | "model" | "effort" | "mode";

/// The most choices one popover lists; a catalogue longer than this is the provider's, and the rest stays in
/// the command palette path.
export const MAX_MENU_ITEMS = 64;

/// The most attachments one message carries, which is also the protocol's published image bound.
export const MAX_ATTACHMENTS = 8;

export function isViewAction(value: unknown): value is ViewAction {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const type = (value as { type?: unknown }).type;
  if (type === "prompt") {
    return typeof (value as { text?: unknown }).text === "string";
  }
  if (type === "answerApproval") {
    const candidate = value as {
      approval?: unknown;
      option?: unknown;
      subjectDigest?: unknown;
    };
    return typeof candidate.approval === "string"
      && typeof candidate.option === "number"
      && Array.isArray(candidate.subjectDigest)
      && candidate.subjectDigest.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255);
  }
  if (type === "switchEffort") {
    // The page reports which model is currently answering so the effort attaches to it; empty is
    // allowed (the controller refuses honestly) but bulk is not.
    const model = (value as { model?: unknown }).model;
    return typeof model === "string" && model.length <= 200;
  }
  if (type === "switchModel" || type === "switchMode") {
    const available = (value as { available?: unknown }).available;
    // Bounded like everything else that crosses the webview boundary: the set is display data from the
    // provider's own announcement, and a hostile page must not be able to smuggle bulk through it.
    if (
      !Array.isArray(available)
      || available.length > 64
      || !available.every((id) => typeof id === "string" && id.length > 0 && id.length <= 200)
    ) {
      return false;
    }
    if (type === "switchMode") return true;
    // The answering model and effort, empty when the provider has not announced them. Bounded like
    // switchEffort's model.
    const { model, effort } = value as { model?: unknown; effort?: unknown };
    return typeof model === "string" && model.length <= 200
      && typeof effort === "string" && effort.length <= 200;
  }
  if (type === "removeAttachment") {
    const index = (value as { index?: unknown }).index;
    return Number.isInteger(index) && (index as number) >= 0 && (index as number) < MAX_ATTACHMENTS;
  }
  if (type === "pasteImage") {
    const candidate = value as { name?: unknown; mediaType?: unknown; base64Data?: unknown };
    return typeof candidate.name === "string"
      && candidate.name.length > 0
      && candidate.name.length <= 255
      && typeof candidate.mediaType === "string"
      && IMAGE_MEDIA_TYPES.has(candidate.mediaType)
      && typeof candidate.base64Data === "string"
      && candidate.base64Data.length > 0
      && candidate.base64Data.length <= PUBLIC_LIMITS.maxAttachmentBase64Bytes
      && candidate.base64Data.length % 4 === 0
      && BASE64.test(candidate.base64Data);
  }
  if (type === "openDiff") {
    return isDeclaredDiff((value as { diff?: unknown }).diff);
  }
  if (type === "menuChoice") {
    const candidate = value as { menu?: unknown; choice?: unknown };
    return typeof candidate.menu === "string"
      && candidate.menu.length <= 64
      && (candidate.choice === null || (typeof candidate.choice === "string" && candidate.choice.length <= 64));
  }
  // Exactly the actions the dispatcher handles. "openWorkspace" and "close" once passed here without a
  // dispatcher branch, so they fell into the interrupt fallback: a message the page never sends today,
  // but one hostile byte away from stopping a running agent.
  return type === "pickProject"
    || type === "pickService"
    || type === "attach"
    || type === "mentionFile"
    || type === "interrupt";
}

const IMAGE_MEDIA_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u;
