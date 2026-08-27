"""Request the minimum read-only enrollment and print its owner-review identity."""

import asyncio

from runtrol_runtime import AsyncRuntimeClient, Identity


async def main() -> None:
    identity = Identity.generate()
    client = await AsyncRuntimeClient.connect(
        name="Runtrol Python example",
        version="0.1.1",
        identity=identity,
    )
    receipt = await client.request_enrollment(
        client_instance_id="runtrol-python-example",
        manifest_digest=bytes(32),
        requested_scopes=["provider.read", "session.list"],
        requested_roots=[],
    )
    print(receipt["pendingId"])
    print(identity.public_key_base64())
    await client.close()


if __name__ == "__main__":
    asyncio.run(main())
