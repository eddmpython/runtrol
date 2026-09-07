"""The Python convenience methods preserve owner receipts and never retry unknown input."""

import asyncio
import json
import sys
from pathlib import Path
from types import ModuleType
from unittest.mock import patch


def loadClient():
    native = ModuleType("runtrol_runtime._native")
    native.PyIdentity = object
    native.NativeError = RuntimeError
    source = str(Path(__file__).resolve().parents[1] / "python")
    with patch.dict(sys.modules, {"runtrol_runtime._native": native}), patch.object(sys, "path", [source, *sys.path]):
        from runtrol_runtime import client, errors
        return client, errors


class NativeInput:
    started_json = '{"subscriptionId":"owner","maxPendingOffers":2}'

    def __init__(self):
        self.calls = []
        self.closed = False

    async def call(self, operation, params):
        self.calls.append((operation, json.loads(params)))
        if operation == "next":
            return '{"kind":"offered","offered":{"subscriptionId":"owner","sequence":1,"binding":{}}}'
        if operation == "claimInput":
            return json.dumps({"text": "한글\r"})
        return "{}"

    def close(self):
        self.closed = True


class NativeTerminal:
    opened_json = "{}"

    def __init__(self):
        self.calls = []
        self.unknown = False

    def initial_screen(self):
        return b""

    async def call(self, operation, params):
        value = json.loads(params)
        self.calls.append((operation, value))
        if self.unknown:
            raise RuntimeError(json.dumps({"code": "outcomeUnknown", "message": "receipt lost", "retryable": False}))
        return json.dumps({"requestId": value["requestId"], "deliverySequence": 1,
                           "ownerRegistrationGeneration": 2, "outcome": "ownerExtensionAccepted"})


def test_owner_input_async_methods_preserve_serial_wire_values():
    client, _errors = loadClient()
    native = NativeInput()

    async def scenario():
        receiver = client.AsyncWindowInputSubscription(native)
        assert receiver.started["maxPendingOffers"] == 2
        offered = await receiver.next()
        assert offered.kind == "offered"
        assert offered.offered["sequence"] == 1
        assert await receiver.claimInput(1) == {"text": "한글\r"}
        await receiver.inputReceipt({"subscriptionId": "owner", "sequence": 1, "outcome": "ownerExtensionAccepted"})
        await receiver.close()

    asyncio.run(scenario())
    assert [operation for operation, _params in native.calls] == ["next", "claimInput", "inputReceipt"]
    assert native.closed


def test_text_receipt_and_unknown_are_identical_in_sync_and_async_surfaces():
    client, errors = loadClient()
    native = NativeTerminal()
    params = {"requestId": "mutation", "terminalId": "terminal", "leaseId": "lease", "leaseGeneration": 1, "text": "한글\r"}
    asynchronous = client.AsyncTerminalView(native)
    assert asyncio.run(asynchronous.sendText(params))["outcome"] == "ownerExtensionAccepted"
    runner = client._LoopRunner()
    try:
        synchronous = client.TerminalView(runner, asynchronous)
        native.unknown = True
        try:
            synchronous.sendText(params)
            raise AssertionError("a lost receipt must remain unknown")
        except errors.OutcomeUnknownError as error:
            assert error.code == "outcomeUnknown"
        assert native.calls == [("sendText", params), ("sendText", params)]
    finally:
        runner.close()


def test_sync_owner_receiver_uses_the_same_claim_and_explicit_close():
    client, _errors = loadClient()
    native = NativeInput()
    runner = client._LoopRunner()
    try:
        receiver = client.WindowInputSubscription(runner, client.AsyncWindowInputSubscription(native))
        assert receiver.next().offered["sequence"] == 1
        assert receiver.claimInput(1)["text"] == "한글\r"
        receiver.inputReceipt({"subscriptionId": "owner", "sequence": 1, "outcome": "ownerExtensionAccepted"})
        receiver.close()
        assert native.closed
    finally:
        runner.close()
