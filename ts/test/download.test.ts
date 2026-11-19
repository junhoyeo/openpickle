import { expect, test } from "bun:test";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { saveAttachmentResponse } from "../src/download.ts";

async function withDestination(use: (out: string, dir: string) => Promise<void>) {
  const dir = await mkdtemp(join(tmpdir(), "openpickle-download-test-"));
  try {
    await use(join(dir, "sample.md"), dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test("publishes the attachment only once the byte stream completes", async () => {
  await withDestination(async (out, dir) => {
    const parts = [new Uint8Array([65, 66]), new Uint8Array([67])];
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        const part = parts.shift();
        if (part) controller.enqueue(part);
        else controller.close();
      },
    });
    expect(await saveAttachmentResponse(new Response(stream), out)).toBe(3);
    expect(await readFile(out, "utf8")).toBe("ABC");
    expect(await readdir(dir)).toEqual(["sample.md"]);
  });
});

test("a failed transfer removes staging data and leaves the previous file intact", async () => {
  await withDestination(async (out, dir) => {
    await writeFile(out, "keep this");
    let sent = false;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (sent) controller.error(new Error("connection lost"));
        else {
          sent = true;
          controller.enqueue(new Uint8Array([65, 66]));
        }
      },
    });
    await expect(saveAttachmentResponse(new Response(stream), out)).rejects.toThrow("connection lost");
    expect(await readFile(out, "utf8")).toBe("keep this");
    expect(await readdir(dir)).toEqual(["sample.md"]);
  });
});
