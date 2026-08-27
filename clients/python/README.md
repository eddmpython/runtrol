# Runtrol Runtime Python client

`runtrol-runtime-client` connects Python 3.11 or newer to one shared per-user Runtrol Runtime. The wheel uses the
CPython stable ABI. It contains neither Runtime nor a provider CLI, starts no daemon, downloads nothing, and never
holds provider model credentials.

The consuming application owns its integration signing identity. Persist `identity.secret_bytes()` in operating
system secure storage, request the minimum scopes and project roots, and show the pending ID and fingerprint to the
user. The owner approves it with `runtrol integrations review <pending-id>`.

```python
from runtrol_runtime import AsyncRuntimeClient, Identity

identity = Identity.generate()
client = await AsyncRuntimeClient.connect(
    name="My local application",
    version="1.0.0",
    identity=identity,
)
receipt = await client.request_enrollment(
    client_instance_id="installed-instance",
    manifest_digest=bytes(32),
    requested_scopes=["provider.read", "session.list"],
    requested_roots=[],
)
print(receipt["pendingId"])
await client.close()
```

After approval, reconnect with the same identity and returned grant. `AsyncRuntimeClient.call()` and
`RuntimeClient.call()` accept the same closed provider-neutral operation names and schema-derived dictionaries.
Dedicated provider, session, event, and terminal-index subscriptions use `subscribe()`. Both clients expose
`open_terminal()` and `attach_terminal()`, and both terminal views expose the same control methods. Terminal output
remains bytes and is never converted into chat messages.

The checked schema and generated `TypedDict` declarations ship in the wheel. Runtime absence raises
`RuntimeNotInstalledError`; importing the package and calling `connect()` never installs or starts it.
