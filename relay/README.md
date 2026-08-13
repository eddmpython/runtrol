# runtrol relay

This package is the independently deployable, untrusted ciphertext relay for the phone connection ladder. It is a
Cloudflare Worker with one SQLite-backed Durable Object per 256-bit route identifier. It does not own a session,
provider process, model credential, or transcript.

## Security boundary

The Rust daemon and phone establish Noise over the relay. TLS protects delivery to the edge, but it is not the data
security boundary. The relay can observe route presence, connection timing, frame sizes, and a random 256-bit peer
identifier. It cannot decrypt the Noise records it forwards.

The Durable Object stores only:

- a SHA-256 digest of the 256-bit route credential
- SHA-256 digests of single-use connection tickets, their role, and their expiry

It never stores a relayed frame. Tickets expire after 30 seconds and are deleted on the first connection attempt.
The route accepts one PC connection and at most eight phone connections. Text frames and oversized binary frames
are closed.

## Wire shape

The public API is deliberately small:

- `PUT /v1/routes/{route}` registers the route credential once.
- `POST /v1/routes/{route}/tickets` exchanges that credential for a short-lived `pc` or `phone` ticket.
- `GET /v1/routes/{route}/connect` upgrades with the `runtrol.relay.v1` protocol and a ticket protocol value.

A phone receives its 32-byte peer identifier as the first binary message. Later phone messages are raw Noise
records. The PC receives `peer_id || noise_record` and replies with the same envelope. An exact 32-byte PC message
means that peer disconnected. Both clients bind the relay origin and peer identifier into the Noise prologue.

Routing role and peer identifier live in serialized WebSocket attachments, so the object can use Cloudflare's
[WebSocket Hibernation API](https://developers.cloudflare.com/durable-objects/best-practices/websockets/). Durable
state uses the recommended
[SQLite storage backend](https://developers.cloudflare.com/durable-objects/best-practices/rules-of-durable-objects/).

## Local verification

```text
npm ci
npm run check
npm run build
```

`check` regenerates no files. It verifies the checked Wrangler binding declarations, type-checks the Worker, and
runs the Durable Object tests in the Workers runtime. `build` performs a Wrangler dry run and does not deploy or
change an account.
