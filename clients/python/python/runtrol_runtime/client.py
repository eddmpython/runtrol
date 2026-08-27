"""Sync and async clients over the actor-backed native public Runtime binding."""

from __future__ import annotations

import asyncio
import json
import threading
from collections.abc import Callable, Coroutine, Sequence
from dataclasses import dataclass
from typing import Any, TypeVar

from . import _native
from .errors import translate
from .generated import JsonObject

T = TypeVar("T")
Identity = _native.PyIdentity


def new_mutation_request_id() -> str:
    """Mint one canonical UUIDv7 for a mutation and retain it across uncertain retries."""

    return _native.new_mutation_request_id()


async def _native_await(awaitable: Any) -> Any:
    try:
        return await awaitable
    except _native.NativeError as error:
        translate(error)


def _object(encoded: str) -> JsonObject:
    value = json.loads(encoded)
    if not isinstance(value, dict):
        raise TypeError("Runtime result is not an object")
    return value


def _objects(encoded: str) -> list[JsonObject]:
    value = json.loads(encoded)
    if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
        raise TypeError("Runtime result is not an object list")
    return value


@dataclass(frozen=True, slots=True)
class TerminalEvent:
    """Exact provider terminal bytes or one explicit view lifecycle boundary."""

    kind: str
    bytes: bytes
    sequence: int | None = None
    lost_chunks: int | None = None
    next_sequence: int | None = None
    exit_code: int | None = None


class AsyncSubscription:
    """One dedicated bounded read-only Runtime stream."""

    def __init__(self, native: _native.PySubscription) -> None:
        self._native = native
        self.started = _object(native.started_json)

    async def next(self) -> JsonObject:
        return _object(await _native_await(self._native.next()))

    async def close(self) -> None:
        await _native_await(self._native.close())


class AsyncTerminalView:
    """One provider-faithful terminal view with an independent Runtime connection."""

    def __init__(self, native: _native.PyTerminalView) -> None:
        self._native = native
        self.opened = _object(native.opened_json)
        self.initial_screen = bytes(native.initial_screen())

    async def next(self) -> TerminalEvent:
        event = await _native_await(self._native.next())
        return TerminalEvent(
            kind=event.kind,
            bytes=bytes(event.bytes()),
            sequence=event.sequence,
            lost_chunks=event.lost_chunks,
            next_sequence=event.next_sequence,
            exit_code=event.exit_code,
        )

    async def call(self, operation: str, params: JsonObject) -> JsonObject:
        return _object(
            await _native_await(
                self._native.call(operation, json.dumps(params, separators=(",", ":")))
            )
        )

    async def acquire_control(self, params: JsonObject) -> JsonObject:
        return await self.call("acquireControl", params)

    async def renew_control(self, params: JsonObject) -> JsonObject:
        return await self.call("renewControl", params)

    async def release_control(self, params: JsonObject) -> None:
        await self.call("releaseControl", params)

    async def write(self, params: JsonObject) -> None:
        await self.call("write", params)

    async def resize(self, params: JsonObject) -> None:
        await self.call("resize", params)

    async def stop(self, params: JsonObject) -> None:
        await self.call("stop", params)

    async def detach(self, params: JsonObject) -> None:
        await self.call("detach", params)


class AsyncRuntimeClient:
    """Async typed client for one explicitly installed shared Runtime."""

    def __init__(self, native: _native.PyRuntimeClient) -> None:
        self._native = native
        self.initialization = _object(native.initialization_json)

    @classmethod
    async def connect(
        cls,
        *,
        name: str,
        version: str,
        identity: Identity | None = None,
        grant: JsonObject | None = None,
    ) -> AsyncRuntimeClient:
        native = await _native_await(
            _native.connect(
                name,
                version,
                identity,
                None if grant is None else json.dumps(grant, separators=(",", ":")),
            )
        )
        return cls(native)

    async def call(self, operation: str, params: JsonObject | None = None) -> JsonObject:
        """Call one closed provider-neutral operation from the generated schema."""

        encoded = json.dumps(params or {}, separators=(",", ":"))
        return _object(await _native_await(self._native.call(operation, encoded)))

    async def request_enrollment(
        self,
        *,
        client_instance_id: str,
        manifest_digest: bytes,
        requested_scopes: Sequence[str],
        requested_roots: Sequence[str],
    ) -> JsonObject:
        return await self.call(
            "integrations.request",
            {
                "clientInstanceId": client_instance_id,
                "manifestDigest": list(manifest_digest),
                "requestedScopes": list(requested_scopes),
                "requestedRoots": list(requested_roots),
            },
        )

    async def watch_enrollment(self, pending_id: str) -> JsonObject:
        return await self.call("integrations.watch", {"pendingId": pending_id})

    async def integration_grant(self) -> JsonObject:
        return await self.call("integrations.grant")

    async def rotate_key(
        self,
        *,
        request_id: str,
        expected_key_generation: int,
        replacement: Identity,
    ) -> JsonObject:
        return await self.call(
            "integrations.rotateKey",
            {
                "requestId": request_id,
                "expectedKeyGeneration": expected_key_generation,
                "replacementSecret": list(replacement.secret_bytes()),
            },
        )

    async def providers(self) -> JsonObject:
        return await self.call("providers.list")

    async def usage(self) -> JsonObject:
        return await self.call("providers.usage")

    async def sessions(self) -> JsonObject:
        return await self.call("sessions.list")

    async def terminals(self) -> JsonObject:
        return await self.call("terminals.list")

    async def terminal_generations(self) -> list[JsonObject]:
        """List exact current and draining generation outcomes for terminal routing."""

        return _objects(await _native_await(self._native.terminal_generations()))

    async def subscribe(self, kind: str, params: JsonObject | None = None) -> AsyncSubscription:
        native = await _native_await(
            self._native.subscribe(kind, json.dumps(params or {}, separators=(",", ":")))
        )
        return AsyncSubscription(native)

    async def terminal(
        self,
        kind: str,
        params: JsonObject,
        runtime_generation: str | None = None,
    ) -> AsyncTerminalView:
        native = await _native_await(
            self._native.terminal(
                kind,
                json.dumps(params, separators=(",", ":")),
                runtime_generation,
            )
        )
        return AsyncTerminalView(native)

    async def open_terminal(self, params: JsonObject) -> AsyncTerminalView:
        return await self.terminal("open", params)

    async def attach_terminal(
        self,
        params: JsonObject,
        *,
        runtime_generation: str | None = None,
    ) -> AsyncTerminalView:
        return await self.terminal("attach", params, runtime_generation)

    async def panic_stop(self) -> None:
        await self.call("panicStop")

    async def close(self) -> None:
        await _native_await(self._native.close())


class _LoopRunner:
    def __init__(self) -> None:
        self._ready = threading.Event()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread = threading.Thread(target=self._run, name="runtrol-python-client", daemon=True)
        self._thread.start()
        self._ready.wait()

    def _run(self) -> None:
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        self._loop = loop
        self._ready.set()
        loop.run_forever()
        loop.close()

    def call(self, factory: Callable[[], Coroutine[Any, Any, T]]) -> T:
        async def invoke() -> T:
            return await factory()

        if self._loop is None:
            raise RuntimeError("runtimeUnavailable", "the synchronous client loop is closed")
        return asyncio.run_coroutine_threadsafe(invoke(), self._loop).result()

    def close(self) -> None:
        if self._loop is not None:
            self._loop.call_soon_threadsafe(self._loop.stop)
            self._thread.join(timeout=5)
            self._loop = None


class Subscription:
    """Synchronous view of one dedicated Runtime subscription."""

    def __init__(self, runner: _LoopRunner, asynchronous: AsyncSubscription) -> None:
        self._runner = runner
        self._asynchronous = asynchronous
        self.started = asynchronous.started

    def next(self) -> JsonObject:
        return self._runner.call(self._asynchronous.next)

    def close(self) -> None:
        self._runner.call(self._asynchronous.close)


class TerminalView:
    """Synchronous view of one provider-faithful terminal."""

    def __init__(self, runner: _LoopRunner, asynchronous: AsyncTerminalView) -> None:
        self._runner = runner
        self._asynchronous = asynchronous
        self.opened = asynchronous.opened
        self.initial_screen = asynchronous.initial_screen

    def next(self) -> TerminalEvent:
        return self._runner.call(self._asynchronous.next)

    def call(self, operation: str, params: JsonObject) -> JsonObject:
        return self._runner.call(lambda: self._asynchronous.call(operation, params))

    def acquire_control(self, params: JsonObject) -> JsonObject:
        return self.call("acquireControl", params)

    def renew_control(self, params: JsonObject) -> JsonObject:
        return self.call("renewControl", params)

    def release_control(self, params: JsonObject) -> None:
        self.call("releaseControl", params)

    def write(self, params: JsonObject) -> None:
        self.call("write", params)

    def resize(self, params: JsonObject) -> None:
        self.call("resize", params)

    def stop(self, params: JsonObject) -> None:
        self.call("stop", params)

    def detach(self, params: JsonObject) -> None:
        self.call("detach", params)


class RuntimeClient:
    """Synchronous client with the same closed operation, stream, and terminal surfaces."""

    def __init__(self, runner: _LoopRunner, asynchronous: AsyncRuntimeClient) -> None:
        self._runner = runner
        self._asynchronous = asynchronous
        self.initialization = asynchronous.initialization

    @classmethod
    def connect(
        cls,
        *,
        name: str,
        version: str,
        identity: Identity | None = None,
        grant: JsonObject | None = None,
    ) -> RuntimeClient:
        runner = _LoopRunner()
        try:
            asynchronous = runner.call(
                lambda: AsyncRuntimeClient.connect(
                    name=name,
                    version=version,
                    identity=identity,
                    grant=grant,
                )
            )
        except BaseException:
            runner.close()
            raise
        return cls(runner, asynchronous)

    def call(self, operation: str, params: JsonObject | None = None) -> JsonObject:
        return self._runner.call(lambda: self._asynchronous.call(operation, params))

    def request_enrollment(
        self,
        *,
        client_instance_id: str,
        manifest_digest: bytes,
        requested_scopes: Sequence[str],
        requested_roots: Sequence[str],
    ) -> JsonObject:
        return self._runner.call(
            lambda: self._asynchronous.request_enrollment(
                client_instance_id=client_instance_id,
                manifest_digest=manifest_digest,
                requested_scopes=requested_scopes,
                requested_roots=requested_roots,
            )
        )

    def watch_enrollment(self, pending_id: str) -> JsonObject:
        return self._runner.call(lambda: self._asynchronous.watch_enrollment(pending_id))

    def integration_grant(self) -> JsonObject:
        return self._runner.call(self._asynchronous.integration_grant)

    def rotate_key(
        self,
        *,
        request_id: str,
        expected_key_generation: int,
        replacement: Identity,
    ) -> JsonObject:
        return self._runner.call(
            lambda: self._asynchronous.rotate_key(
                request_id=request_id,
                expected_key_generation=expected_key_generation,
                replacement=replacement,
            )
        )

    def providers(self) -> JsonObject:
        return self._runner.call(self._asynchronous.providers)

    def usage(self) -> JsonObject:
        return self._runner.call(self._asynchronous.usage)

    def sessions(self) -> JsonObject:
        return self._runner.call(self._asynchronous.sessions)

    def terminals(self) -> JsonObject:
        return self._runner.call(self._asynchronous.terminals)

    def terminal_generations(self) -> list[JsonObject]:
        return self._runner.call(self._asynchronous.terminal_generations)

    def subscribe(self, kind: str, params: JsonObject | None = None) -> Subscription:
        subscription = self._runner.call(lambda: self._asynchronous.subscribe(kind, params))
        return Subscription(self._runner, subscription)

    def terminal(
        self,
        kind: str,
        params: JsonObject,
        runtime_generation: str | None = None,
    ) -> TerminalView:
        terminal = self._runner.call(
            lambda: self._asynchronous.terminal(kind, params, runtime_generation)
        )
        return TerminalView(self._runner, terminal)

    def open_terminal(self, params: JsonObject) -> TerminalView:
        return self.terminal("open", params)

    def attach_terminal(
        self,
        params: JsonObject,
        *,
        runtime_generation: str | None = None,
    ) -> TerminalView:
        return self.terminal("attach", params, runtime_generation)

    def panic_stop(self) -> None:
        self._runner.call(self._asynchronous.panic_stop)

    def close(self) -> None:
        try:
            self._runner.call(self._asynchronous.close)
        finally:
            self._runner.close()
