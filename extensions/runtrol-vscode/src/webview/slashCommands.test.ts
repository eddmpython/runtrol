import assert from "node:assert/strict";
import test from "node:test";

import {
  asksForCommands,
  completed,
  matchingCommands,
  movedHighlight,
  slashCommandsOf,
} from "./slashCommands";

test("a described command list is read as the service wrote it", () => {
  const commands = slashCommandsOf({
    payload: {
      availableCommands: [
        { name: "model", description: "Change the model" },
        { name: "compact", description: "Compact the context" },
      ],
    },
  });
  assert.deepEqual(commands, [
    { name: "model", description: "Change the model" },
    { name: "compact", description: "Compact the context" },
  ]);
});

test("a bare list of names is the same fact as a described one", () => {
  // Claude Code announces `slash_commands: ["compact"]` inside its startup frame. Same information, less of it.
  const commands = slashCommandsOf({ payload: { slash_commands: ["compact", "model"] } });
  assert.deepEqual(commands.map((command) => command.name), ["compact", "model"]);
  assert.equal(commands[0]?.description, "");
});

test("nothing but the name and the description is read out of the payload", () => {
  // A command's argument schema is the service's business. Interpreting it would make Runtrol decide what a
  // command means, and passing the command through untouched is the whole point.
  const commands = slashCommandsOf({
    payload: {
      availableCommands: [
        { name: "model", description: "Change the model", input: { hint: "SECRET" }, meta: "SECRET" },
      ],
    },
  });
  assert.equal(JSON.stringify(commands).includes("SECRET"), false);
});

test("a service that announced nothing offers nothing", () => {
  assert.deepEqual(slashCommandsOf({}), []);
  assert.deepEqual(slashCommandsOf({ payload: {} }), []);
  assert.deepEqual(slashCommandsOf({ payload: { availableCommands: "not a list" } }), []);
});

test("a duplicate name is one command, not two rows that look identical", () => {
  const commands = slashCommandsOf({
    payload: { availableCommands: [{ name: "model" }, { name: "model" }] },
  });
  assert.equal(commands.length, 1);
});

test("only a slash that opens the message asks for the menu", () => {
  // A slash later in a sentence is part of a path or a fraction. Popping a menu there interrupts prose.
  assert.equal(asksForCommands("/"), true);
  assert.equal(asksForCommands("/mod"), true);
  assert.equal(asksForCommands("look at src/main.ts"), false);
  assert.equal(asksForCommands("/model gpt"), false, "once an argument is typed the choice is made");
  assert.equal(asksForCommands(""), false);
});

test("what was typed ranks above a match anywhere else", () => {
  const commands = [
    { name: "compact", description: "" },
    { name: "model", description: "" },
    { name: "remodel", description: "" },
  ];
  const matched = matchingCommands(commands, "/mod");
  assert.deepEqual(matched.map((command) => command.name), ["model", "remodel"]);
});

test("a bare slash offers everything, bounded", () => {
  const many = Array.from({ length: 40 }, (_unused, index) => ({ name: `c${index}`, description: "" }));
  assert.equal(matchingCommands(many, "/").length, 8);
});

test("choosing a command leaves room for its argument", () => {
  // Every one of these either takes an argument or ignores one, so the space is never wrong and its absence
  // costs a keystroke to the person who wanted one.
  assert.equal(completed({ name: "model", description: "" }), "/model ");
});

test("the highlight wraps at both ends", () => {
  // A menu of at most eight items is a ring, not a page. Reaching the end and pressing on should return to the
  // start rather than do nothing.
  assert.equal(movedHighlight(0, 3, -1), 2);
  assert.equal(movedHighlight(2, 3, 1), 0);
  assert.equal(movedHighlight(0, 0, 1), 0, "an empty menu has nowhere to move");
});
