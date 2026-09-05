import { lstat, readFile, readdir } from "node:fs/promises";
import path from "node:path";

// Pass unique ASCII sentinel segments for new journeys. Older callers can pass the whole body, including
// Unicode and newlines. Never derive a shorter marker: a body prefix may also name an unrelated fixture pipe.
export async function assertNoBodyResidue(directory, markers) {
  const bodies = typeof markers === "string" ? [markers] : markers;
  if (!Array.isArray(bodies) || bodies.length === 0
    || bodies.some((body) => typeof body !== "string" || body.length === 0)) {
    throw new TypeError("body residue scanning requires nonempty markers");
  }
  const needles = new Set();
  for (const body of bodies) {
    needles.add(body);
    needles.add(JSON.stringify(body).slice(1, -1));
  }
  const encodings = ["utf8", "utf16le"];
  const encoded = [...needles].flatMap((needle) => encodings.map((encoding) => Buffer.from(needle, encoding)));
  const residue = new Error("opaque body retained in a scanned file");
  const link = new Error("body residue scanning refuses symbolic links");
  try {
    if ((await lstat(directory)).isSymbolicLink()) throw link;
    await visit(directory);
  } catch (error) {
    if (error === residue || error === link) throw error;
    // Filesystem errors include paths, which may themselves contain a body. Do not retain the original cause.
    throw new Error("body residue scanning could not read an audit entry");
  }

  function containsMarker(bytes) {
    return encoded.some((needle) => bytes.includes(needle))
      || encodings.some((encoding) => {
        // A JSON writer may escape any character, including only some of the ASCII sentinel. Decode those
        // escapes without parsing or printing the surrounding file, which need not itself be valid JSON.
        const text = bytes.toString(encoding).replace(/\\u([\da-f]{4})/giu,
          (_escape, digits) => String.fromCharCode(Number.parseInt(digits, 16)));
        return [...needles].some((needle) => text.includes(needle));
      });
  }

  async function visit(current) {
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const target = path.join(current, entry.name);
      if (containsMarker(Buffer.from(path.relative(directory, target)))) throw residue;
      if (entry.isSymbolicLink()) throw link;
      if (entry.isDirectory()) await visit(target);
      else if (entry.isFile()) {
        if (containsMarker(await readFile(target))) throw residue;
      }
    }
  }
}
