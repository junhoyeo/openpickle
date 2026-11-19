import { createWriteStream } from "node:fs";
import { mkdtemp, rename, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

/** Publish only a complete download. The chosen path may replace a file, as before. */
export async function saveAttachmentResponse(response: Response, out: string): Promise<number> {
  const body = response.body;
  if (!body) throw new Error("The attachment response has no file body.");
  if (response.status !== 200 || response.headers.has("content-range")) {
    await body.cancel().catch(() => {});
    throw new Error("The server did not send the full attachment.");
  }
  let bytes = 0;
  const directory = await mkdtemp(join(dirname(out), ".openpickle-download-")).catch(async (error) => {
    await body.cancel().catch(() => {});
    throw error;
  });
  const staging = join(directory, "attachment");
  async function* chunks(source: ReadableStream<Uint8Array>) {
    const reader = source.getReader();
    try {
      for (;;) {
        const chunk = await reader.read();
        if (chunk.done) return;
        bytes += chunk.value.byteLength;
        yield chunk.value;
      }
    } finally {
      await reader.cancel().catch(() => {});
      reader.releaseLock();
    }
  }
  try {
    await pipeline(Readable.from(chunks(body), { objectMode: false }), createWriteStream(staging, { mode: 0o600 }));
    await rename(staging, out);
    return bytes;
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
