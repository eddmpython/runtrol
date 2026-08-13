# Runtrol Runtime TypeScript client

This package connects a Node.js application to the shared per-user Runtrol Runtime. It discovers the owner-local
Runtime locator, negotiates the public protocol, signs integration enrollment and reconnect proofs, and exposes typed
provider and session operations.

It does not bundle or start Runtime, inspect provider storage, hold provider credentials, or own provider sessions.
The application owns its integration private key and should persist the PKCS#8 bytes in operating-system secure
storage.

```ts
import { IntegrationIdentity, RuntimeConnector } from "@runtrol/runtime-client";

const identity = IntegrationIdentity.generate();
const runtime = await new RuntimeConnector().connectSystem({
  name: "My local companion",
  version: "1.0.0",
  identity,
});

const receipt = await runtime.integrations().request({
  clientInstanceId: "installed-instance",
  manifestDigest: new Uint8Array(32),
  requestedScopes: ["provider.read", "session.list"],
  requestedRoots: [],
});

console.log(receipt.pendingId);
runtime.close();
```

Provider installation observations also use a dedicated snapshot stream and update only when Runtime observes a
changed verified inventory:

```ts
const providers = await runtime.providers().watch();
console.log(providers.started.snapshot.providers);
const providerUpdate = await providers.next();
if (providerUpdate.kind === "changed") console.log(providerUpdate.changed.snapshot.providers);
```

Irreversible Runtime metadata removal remains locally confirmed. `sessions().forget(params)` first returns
`presenceRequired`; approve the exact request in Runtrol Studio and retry the unchanged mutation request ID. This removes
only the Runtime pointer, never provider-owned conversation state.

Integration key replacement also requires local confirmation. Generate and securely retain the replacement identity,
then call `integrations().rotateKey(requestId, previousKeyGeneration, replacement)`. After Runtrol Studio confirms the
exact integration and replacement fingerprint, retry the same values. The returned credentials carry the incremented
key generation and the previous key can no longer reconnect.

`connectSystemWithRetry` retries only connection establishment with capped exponential backoff, jitter, and a total
deadline. It reads and validates the system locator again for every attempt. Authentication, protocol, enrollment,
and authorization failures return immediately.

Use `watchEventsWithReconnectSystem` for a read-only event stream that survives Runtime endpoint replacement. After
consuming an event, call `accept(event.event.nextExpected)` before reading another. Reconnection uses only that accepted
cursor and returns a `reconnected` item with the full start boundary and any replay gap. The wrapper never acquires
control or retries input, approval, interrupt, or lifecycle mutations. `watchEventsWithReconnect` provides the same
cursor contract for callers that already own a fixed validated locator.

Managed-session changes use a dedicated snapshot stream, so an integration does not poll or infer changes from
provider output:

```ts
const index = await runtime.sessions().watchIndex();
console.log(index.started.snapshot.sessions);
const next = await index.next();
if (next.kind === "changed") console.log(next.changed.snapshot.sessions);
if (next.kind === "ended") console.log(next.ended.reason);
```

After local approval and an authenticated reconnect, start requests keep their UUIDv7 identity so an uncertain retry
cannot silently create another provider session:

```ts
import { newMutationRequestId } from "@runtrol/runtime-client";

const provider = (await runtime.providers().list()).providers.at(0);
if (!provider) throw new Error("No provider is installed");
const capabilities = await runtime.providers().getCapabilities(provider.providerId);
if (capabilities.freshSession.availability !== "available") {
  throw new Error("Provider cannot start a fresh session");
}
const opened = await runtime.sessions().start({
  requestId: newMutationRequestId(),
  providerId: provider.providerId,
  workspace: approvedWorkspace,
  access: "exclusive",
});

console.log(opened.session.sessionId);
const current = await runtime.sessions().get(opened.session.sessionId);
console.log(current.lifecycle);
await runtime.sessions().cool({
  requestId: newMutationRequestId(),
  sessionId: opened.session.sessionId,
  expectedSessionGeneration: opened.session.sessionGeneration,
  leaseId: opened.control.leaseId,
  leaseGeneration: opened.control.leaseGeneration,
});
```

Pending approvals remain bound to the current control lease. Risk comes from the provider request retained by
Runtime, never from caller input:

```ts
const pending = await runtime.approvals().listPending({
  sessionId: opened.session.sessionId,
  leaseId: opened.control.leaseId,
  leaseGeneration: opened.control.leaseGeneration,
});
const approval = pending.approvals.at(0);
const option = approval?.options.find((candidate) => candidate.unavailable == null);
if (approval && option) {
  await runtime.approvals().respond({
    requestId: newMutationRequestId(),
    sessionId: opened.session.sessionId,
    leaseId: opened.control.leaseId,
    leaseGeneration: opened.control.leaseGeneration,
    approvalId: approval.approvalId,
    optionId: option.optionId,
    subjectDigest: approval.subjectDigest,
  });
}
```
