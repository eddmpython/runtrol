import { frameToItem } from "./frames";
import type { WatchCursor } from "./domain";

type QueuedFrame = {
  frame: string;
  nextExpected: WatchCursor;
};

type FrameBatch = {
  session: string;
  view: number;
  frames: QueuedFrame[];
};

type ParsedBatch = {
  session: string;
  view: number;
  frames: Array<{
    pending: ReturnType<typeof frameToItem>;
    nextExpected: WatchCursor;
  }>;
};

self.onmessage = ({ data }: MessageEvent<FrameBatch>) => {
  const parsed: ParsedBatch = {
    session: data.session,
    view: data.view,
    frames: data.frames.map(({ frame, nextExpected }) => ({
      pending: frameToItem(frame),
      nextExpected,
    })),
  };
  self.postMessage(parsed);
};
