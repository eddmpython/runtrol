"""Gate: a headless phone survives a network cut and a production Core restart.

The shipped PWA WebCrypto, Noise, record, and Core modules drive an installed real CLI. The journey cuts the phone
watch socket, requires exact bounded replay with no duplicate cursor, aborts and recomposes the production server
over the same durable home, requires an explicit cross-stream gap, and continues through the provider-native resume
surface. The deterministic loopback model discards request bodies and uses no hosted credential.
"""

from __future__ import annotations

import sys

import phoneDrivesPcSmoke as phone


if __name__ == "__main__":
    raise SystemExit(phone.run("resilience", sys.argv[1:], "remoteResilienceFaultInjection"))
