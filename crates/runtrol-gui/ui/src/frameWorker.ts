import { frameToItem } from "./frames";

type FrameBatch = {
  session: string;
  frames: string[];
};

type ParsedBatch = {
  session: string;
  frames: ReturnType<typeof frameToItem>[];
};

self.onmessage = ({ data }: MessageEvent<FrameBatch>) => {
  const parsed: ParsedBatch = {
    session: data.session,
    frames: data.frames.map(frameToItem),
  };
  self.postMessage(parsed);
};
