// Live mirror observation retains payload only when the caller explicitly selects a synthetic fixture.
export async function captureMirrorView(view, milliseconds, syntheticFixture = false) {
  const terminal = view.opened.terminal;
  const result = { origin: terminal.origin, ownerWindowSessionId: terminal.ownerWindowSessionId,
    ownerTerminalKey: terminal.ownerTerminalKey, checkpointBytes: view.initialScreen.byteLength,
    chunks: 0, lagged: 0, liveBytes: 0, exited: null };
  const fixture = [];
  let expired = false;
  const timer = setTimeout(() => { expired = true; view.close(); }, milliseconds);
  try {
    for (;;) {
      let event;
      try { event = await view.next(); }
      catch (error) { if (expired) break; throw error; }
      if (event.kind === "output") {
        result.chunks += 1;
        result.liveBytes += event.bytes.byteLength;
        if (syntheticFixture) {
          if (result.liveBytes > 256 * 1024) throw new Error("the synthetic mirror exceeded its capture bound");
          fixture.push(Buffer.from(event.bytes));
        }
      } else if (event.kind === "lagged") {
        result.lagged += 1;
      } else if (event.kind === "exited") {
        result.exited = event.exitCode;
        break;
      } else {
        throw new Error("the mirror watch ended without an exit receipt");
      }
    }
    return syntheticFixture ? { ...result, fixtureHex: Buffer.concat(fixture).toString("hex") } : result;
  } finally {
    clearTimeout(timer);
    view.close();
  }
}
