import assert from "node:assert/strict";
import test from "node:test";

import { insertMention, mentionTriggered } from "./mention";

test("only a word-starting @ asks for the picker", () => {
  assert.ok(mentionTriggered("@", 1));
  assert.ok(mentionTriggered("look at @", 9));
  assert.equal(mentionTriggered("user@", 5), false, "an email @ is content");
  assert.equal(mentionTriggered("@x", 2), false, "only the keystroke that typed the @ triggers");
  assert.equal(mentionTriggered("", 0), false);
});

test("the chosen path replaces the mention token where it was typed", () => {
  assert.deepEqual(insertMention("see @", 5, "src/main.rs "), { value: "see src/main.rs ", caret: 16 });
  assert.deepEqual(
    insertMention("see @ma and more", 7, "src/main.rs "),
    { value: "see src/main.rs  and more", caret: 16 },
    "typing continued while the picker was open still replaces the whole token",
  );
});

test("without a token the text lands at the caret instead of guessing", () => {
  assert.deepEqual(insertMention("plain text", 5, "X"), { value: "plainX text", caret: 6 });
  assert.deepEqual(
    insertMention("a @ b", 5, "X"),
    { value: "a @ bX", caret: 6 },
    "an @ followed by whitespace is no longer a token",
  );
});
