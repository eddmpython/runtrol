import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine } from "./runtimeTypes";
import { firstOffer, offersFor, troubleOf, troubleSentence } from "./serviceHelp";

function provider(overrides: Partial<ProviderLine> = {}): ProviderLine {
  return {
    providerId: "claude",
    displayName: "Claude Code",
    installation: { state: "usable", version: "2.1.0" },
    help: {
      signIn: "claude auth login",
      diagnose: "claude doctor",
      install: "npm install --global @anthropic-ai/claude-code",
    },
    ...overrides,
  };
}

test("a private local action means the coding service wants signing in", () => {
  // The one error category that carries real information about the cause.
  assert.equal(troubleOf("presenceRequired", provider()), "needsSigningIn");
});

test("signing in is what gets offered first when that is the trouble", () => {
  const offer = firstOffer(provider(), "needsSigningIn");
  assert.equal(offer?.command, "claude auth login");
});

test("a missing executable outranks whatever the error category said", () => {
  // Discovery already knows the CLI is absent. Offering a sign-in there sends somebody to authenticate
  // against a program that is not on the machine.
  const absent = provider({ installation: { state: "missing" } });
  assert.equal(troubleOf("presenceRequired", absent), "needsSigningIn");
  assert.equal(troubleOf("providerUnavailable", absent), "notInstalled");
  assert.equal(firstOffer(absent, "notInstalled")?.command, "npm install --global @anthropic-ai/claude-code");
});

test("an installed service that will not run leads with its own diagnosis", () => {
  // The CLI knows its installation, configuration and login better than any check Runtrol could write, and
  // it stays correct when the vendor changes any of them.
  const trouble = troubleOf("providerUnavailable", provider());
  assert.equal(trouble, "misbehaving");
  assert.equal(firstOffer(provider(), trouble)?.command, "claude doctor");
});

test("an unknown failure leads with diagnosis rather than a guess", () => {
  // Running a diagnosis is never the wrong thing to have done; guessing at a sign-in is.
  assert.equal(troubleOf(undefined, provider()), "unknown");
  assert.equal(firstOffer(provider(), "unknown")?.command, "claude doctor");
});

test("a service that declares nothing is offered nothing rather than a dead end", () => {
  const bare = provider({ help: undefined });
  assert.deepEqual(offersFor(bare, "needsSigningIn"), []);
  assert.equal(firstOffer(bare, "needsSigningIn"), null);
});

test("only the commands a service actually declared are offered", () => {
  // OpenCode declares no self-diagnosis, because its `debug` is a group of inspection subcommands rather
  // than a single check. An offer that only prints resolved configuration would send a stuck person
  // somewhere that cannot help them.
  const partial = provider({
    displayName: "OpenCode",
    providerId: "acp-fixture",
    help: { signIn: "acp-fixture auth", install: "npm install --global acp-fixture" },
  });
  const commands = offersFor(partial, "unknown").map((offer) => offer.command);
  assert.deepEqual(commands, ["acp-fixture auth", "npm install --global acp-fixture"]);
});

test("every offer names the service so two services never read identically", () => {
  const claude = offersFor(provider(), "needsSigningIn");
  const codex = offersFor(
    provider({ providerId: "codex", displayName: "Codex", help: { signIn: "codex login" } }),
    "needsSigningIn",
  );
  assert.ok(claude[0]?.label.includes("Claude Code"));
  assert.ok(codex[0]?.label.includes("Codex"));
  assert.notEqual(claude[0]?.label, codex[0]?.label);
});

test("what the person reads is about the coding service, not the transport", () => {
  // The failure this replaced said "the session or native pointer changed after the caller observed it",
  // which is protocol vocabulary in front of somebody trying to get work done.
  for (const trouble of ["needsSigningIn", "notInstalled", "misbehaving", "unknown"] as const) {
    const sentence = troubleSentence(provider(), trouble);
    assert.ok(sentence.includes("Claude Code"), `${trouble} does not name the service`);
    assert.ok(!sentence.includes("pointer"), `${trouble} mentions the transport`);
    assert.ok(!sentence.includes("session"), `${trouble} mentions protocol vocabulary`);
  }
});

test("no offered command carries anything a shell reads as a second command", () => {
  // Defence in depth. The manifest boundary already refuses these characters, and this asserts that
  // nothing between there and the terminal reintroduces one.
  const forbidden = [";", "&", "|", "$", "`", "\n", ">", "<", "(", ")"];
  for (const trouble of ["needsSigningIn", "notInstalled", "misbehaving", "unknown"] as const) {
    for (const offer of offersFor(provider(), trouble)) {
      for (const character of forbidden) {
        assert.ok(
          !offer.command.includes(character),
          `${offer.command} contains ${JSON.stringify(character)}`,
        );
      }
    }
  }
});
