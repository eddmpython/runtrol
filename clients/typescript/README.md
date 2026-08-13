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

After local approval and an authenticated reconnect, start requests keep their UUIDv7 identity so an uncertain retry
cannot silently create another provider session:

```ts
import { newMutationRequestId } from "@runtrol/runtime-client";

const provider = (await runtime.providers().list()).providers.at(0);
if (!provider) throw new Error("No provider is installed");
const opened = await runtime.sessions().start({
  requestId: newMutationRequestId(),
  providerId: provider.providerId,
  workspace: approvedWorkspace,
  access: "exclusive",
});

console.log(opened.session.sessionId);
```
