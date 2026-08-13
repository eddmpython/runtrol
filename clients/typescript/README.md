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
