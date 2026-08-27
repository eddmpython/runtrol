"""Open a faithful provider terminal after enrollment and local grant approval."""

import asyncio
import json
import sys
from pathlib import Path

from runtrol_runtime import AsyncRuntimeClient, Identity, new_mutation_request_id


async def main(provider_id: str, workspace: str, secret_file: str, grant_file: str) -> None:
    identity = Identity.from_secret(Path(secret_file).read_bytes())
    grant = json.loads(Path(grant_file).read_text(encoding="utf-8"))
    client = await AsyncRuntimeClient.connect(
        name="Runtrol Python terminal example",
        version="0.1.1",
        identity=identity,
        grant=grant,
    )
    terminal = await client.open_terminal(
        {
            "requestId": new_mutation_request_id(),
            "providerId": provider_id,
            "workspace": workspace,
            "target": {"kind": "fresh"},
            "geometry": {"columns": 120, "rows": 36},
        }
    )
    sys.stdout.buffer.write(terminal.initial_screen)
    while True:
        event = await terminal.next()
        if event.kind == "exited":
            break
        sys.stdout.buffer.write(event.bytes)
        sys.stdout.buffer.flush()
    await client.close()


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]))
