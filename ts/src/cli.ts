#!/usr/bin/env bun
// Small CLI over PickleClient. Output is JSON, except `search`, `explore`, `attachments`,
// `thumbnail` and `download`. Credentials come from one of:
//   PICKLE_SESSION_FILE  a JSON file holding {"login_session_token": "ls_…"}. A CLI command
//                        holds an exclusive lock while it can read or rotate this file.
//   PICKLE_TOKEN         an access token (the app's Authorization header), valid for 5 minutes.

import { lstatSync, mkdirSync, readFileSync, realpathSync, rmdirSync } from "node:fs";
import { mkdir, rename, rm, writeFile } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { createInterface } from "node:readline/promises";
import { PickleClient, decodeAccessToken } from "./client.ts";
import type { HistoryItem, LoginSessionRefresh } from "./types.ts";

const USAGE = `usage: PICKLE_SESSION_FILE=<file> pickle <command>
       PICKLE_TOKEN=<jwt> pickle <command>

  login <phone> [carrier]      sign in with a texted code and write PICKLE_SESSION_FILE;
                               <phone> is E.164, e.g. +821012345678
  token                        claims and remaining lifetime of PICKLE_TOKEN
  sync                         rooms with cursors and unread counts
  history <room> [after_seq]   room history after a sequence number (default 0)
  send <room> <text>           send a message; <room> may be "character"
  phone-call <id>              read a phone call's current status and transcript
  handoff <name>               read one task's plan and progress log
  search <query>               messages containing <query>, newest first, one per line:
                               seq, date, label and the snippet with matches in [brackets]
  explore [area]               Explore sections and their offers: title, call to action and
                               how many asked; [area] (e.g. money, travel) keeps that area only
  attachments <room>           photos and files sent in the room, newest first, one per line:
                               date, kind (image or file), file name, size (bytes for
                               files, width x height for images) and the attachment path
  thumbnail <path> <out file>  write the JPEG thumbnail of an attachment path to <out file>
  download <path> <out file>   write the attachment itself, the original file, to <out file>
  export <dir>                 write the full history of every room to <dir>/<room_id>.json
  events                       stream live events, one JSON object per line
  session | register | character | profile | social
  screen <kind> [key=value]... server-driven screen data, e.g. "screen handoffs"
  get <path>                   GET any path, e.g. "get /api/secure/transfers"`;

const [command, ...args] = process.argv.slice(2);
const sessionFile = process.env.PICKLE_SESSION_FILE;
const token = process.env.PICKLE_TOKEN?.replace(/^Bearer\s+/i, "");
if (!command || (!sessionFile && !token)) {
  console.error(USAGE);
  process.exit(2);
}

/** Locks this path before reading it or issuing fresh credentials. Never reclaim another lock. */
function openSessionFile(input: string, createParent: boolean) {
  const parent = dirname(input);
  if (createParent) mkdirSync(parent, { recursive: true });
  let file = join(realpathSync.native(parent), basename(input));
  try {
    const stat = lstatSync(file);
    if (!stat.isFile() || stat.nlink > 1) {
      throw new Error("PICKLE_SESSION_FILE must be a regular file without symlinks or hard links to it");
    }
    file = realpathSync.native(file);
  } catch (error) {
    if (!createParent || (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const lock = `${file}.lock`;
  try {
    mkdirSync(lock, { mode: 0o700 });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    throw new Error(`Session file is locked: ${lock}. Remove the lock only after confirming no CLI command is using the session.`);
  }
  process.once("exit", () => {
    try {
      rmdirSync(lock);
    } catch {
      console.error(`Could not remove the session lock: ${lock}`);
    }
  });

  return {
    file,
    async save(refresh: LoginSessionRefresh) {
      const next = { login_session_token: refresh.login_session_token, expires_at: refresh.expires_at };
      const temporary = join(lock, "next");
      try {
        await writeFile(temporary, JSON.stringify(next), { mode: 0o600, flag: "wx" });
        await rename(temporary, file);
      } catch (error) {
        await rm(temporary, { force: true }).catch(() => {});
        throw error;
      }
    },
  };
}

function readSessionToken(file: string): string {
  const text = readFileSync(file, "utf8");
  let token: unknown;
  try {
    token = JSON.parse(text)?.login_session_token;
  } catch {}
  if (typeof token !== "string" || token.length === 0) {
    throw new Error("PICKLE_SESSION_FILE must contain a nonempty login_session_token");
  }
  return token;
}

const session = sessionFile && command !== "token" ? openSessionFile(sessionFile, command === "login") : undefined;
if (command === "login") {
  const [phone, carrier] = args;
  if (!phone || !session) throw new Error("PICKLE_SESSION_FILE=<file> pickle login <phone> [carrier]");
  const signedOut = new PickleClient();
  const { pending_id, expires_in } = await signedOut.requestPhoneCode(phone, { carrier });
  const prompt = createInterface({ input: process.stdin, output: process.stderr });
  const code = (await prompt.question(`Code texted to ${phone} (valid ${expires_in / 60} min): `)).trim();
  prompt.close();
  const issued = await signedOut.verifyPhoneCode(pending_id, code);
  await session.save(issued);
  console.log(JSON.stringify({ user_id: issued.user_id, expires_at: issued.expires_at, saved_to: sessionFile }, null, 2));
  process.exit(0);
}

const pickle = session
  ? new PickleClient({
      loginSession: {
        token: readSessionToken(session.file),
        onRotate: session.save,
      },
    })
  : new PickleClient({ accessToken: token! });
const print = (value: unknown) => console.log(JSON.stringify(value, null, 2));

async function resolveRoom(room: string): Promise<string> {
  if (room !== "character") return room;
  const { rooms } = await pickle.sync();
  const own = rooms.find((r) => r.kind === "character");
  if (!own) throw new Error("no character room in /api/sync");
  return own.room_id;
}

/**
 * Every message of a room, newest first: the newest window, then history pages before its oldest
 * message, as the app pages back from what it already holds.
 */
async function* newestFirst(roomId: string): AsyncGenerator<HistoryItem> {
  const { messages } = await pickle.getHistory(roomId);
  yield* [...messages].reverse();
  let beforeSeq = messages[0]?.seq;
  while (beforeSeq !== undefined) {
    const page = await pickle.getHistoryPage(roomId, { beforeSeq });
    yield* [...page.messages].reverse();
    if (!page.has_more || page.next_before >= beforeSeq) return;
    beforeSeq = page.next_before;
  }
}

switch (command) {
  case "token": {
    if (!token) throw new Error("token reads PICKLE_TOKEN");
    const claims = decodeAccessToken(token);
    print({ ...claims, expires_in_s: claims.exp - Math.floor(Date.now() / 1000) });
    break;
  }
  case "sync":
    print(await pickle.sync());
    break;
  case "history": {
    const [room, afterSeq = "0"] = args;
    if (!room || !/^\d+$/.test(afterSeq)) throw new Error("history <room> [after_seq]");
    print(await pickle.getHistoryDelta(await resolveRoom(room), { afterSeq: Number(afterSeq) }));
    break;
  }
  case "send": {
    const [room, ...words] = args;
    if (!room || words.length === 0) throw new Error("send <room> <text>");
    print(await pickle.sendMessage(await resolveRoom(room), words.join(" ")));
    break;
  }
  case "phone-call": {
    const [callId, ...rest] = args;
    if (!callId || rest.length !== 0) throw new Error("phone-call <id>");
    print(await pickle.getPhoneCall(callId));
    break;
  }
  case "handoff": {
    if (args.length === 0) throw new Error("handoff <name>");
    print(await pickle.getHandoff(args.join(" ")));
    break;
  }
  case "search": {
    if (args.length === 0) throw new Error("search <query>");
    const { items } = await pickle.searchMessages(args.join(" "));
    for (const item of items) {
      const snippet = item.segments.map((s) => (s.match ? `[${s.text}]` : s.text)).join("");
      console.log(`${item.seq}\t${item.date_label}\t${item.label}\t${snippet}`);
    }
    break;
  }
  case "explore": {
    const [area] = args;
    const { sections } = await pickle.getExploreSections();
    for (const section of sections) {
      const offers = area ? section.items.filter((offer) => offer.areas.includes(area)) : section.items;
      if (offers.length === 0) continue;
      console.log(section.title);
      for (const offer of offers) {
        console.log(`  ${offer.title}\t${offer.cta_label}\t${offer.accepted_users_label}`);
      }
    }
    break;
  }
  case "attachments": {
    const [room] = args;
    if (!room) throw new Error("attachments <room>");
    const seen = new Set<string>();
    for await (const item of newestFirst(await resolveRoom(room))) {
      const paths = [...(item.attachments ?? []), ...Object.keys(item.image_meta ?? {}), ...Object.keys(item.file_meta ?? {})];
      for (const path of paths) {
        if (seen.has(path)) continue;
        seen.add(path);
        const image = item.image_meta?.[path];
        const file = item.file_meta?.[path];
        const date = new Date(item.at * 1000).toISOString().slice(0, 10);
        const name = file?.name ?? path.slice(path.lastIndexOf("/") + 1);
        const size = image ? `${image.w}x${image.h}` : file ? String(file.size) : "-";
        console.log(`${date}\t${image ? "image" : "file"}\t${name}\t${size}\t${path}`);
      }
    }
    break;
  }
  case "thumbnail": {
    const [path, out] = args;
    if (!path || !out) throw new Error("thumbnail <path> <out file>");
    const { bytes, contentType } = await pickle.getAttachmentThumbnail(path);
    await writeFile(out, bytes);
    console.error(`${contentType}, ${bytes.length} bytes written to ${out}`);
    break;
  }
  case "download": {
    const [path, out] = args;
    if (!path || !out) throw new Error("download <path> <out file>");
    const { bytes, contentType } = await pickle.getAttachment(path);
    await writeFile(out, bytes);
    console.error(`${contentType}, ${bytes.length} bytes written to ${out}`);
    break;
  }
  case "export": {
    const [dir] = args;
    if (!dir) throw new Error("export <dir>");
    await mkdir(dir, { recursive: true });
    const sync = await pickle.sync();
    await writeFile(join(dir, "sync.json"), JSON.stringify(sync, null, 2));
    for (const room of sync.rooms) {
      const pages = [];
      for await (const page of pickle.history(room.room_id)) pages.push(page);
      const merged = {
        room,
        items: pages.flatMap((p) => p.items),
        reactions: pages.flatMap((p) => p.reactions),
        card_states: pages.flatMap((p) => p.card_states),
        pages: pages.length,
      };
      await writeFile(join(dir, `${room.room_id}.json`), JSON.stringify(merged, null, 2));
      const who = room.peer?.display_name ?? "(your character)";
      console.error(`${room.kind} ${who}: ${merged.items.length} items, ${pages.length} pages, last_seq ${room.last_seq}`);
    }
    break;
  }
  case "events": {
    const controller = new AbortController();
    const stop = () => controller.abort();
    process.once("SIGINT", stop);
    process.once("SIGTERM", stop);
    try {
      for await (const event of pickle.events({ signal: controller.signal })) console.log(JSON.stringify(event));
    } catch (error) {
      const aborted = error === controller.signal.reason || (error instanceof Error && error.name === "AbortError");
      if (!controller.signal.aborted || !aborted) throw error;
    } finally {
      process.off("SIGINT", stop);
      process.off("SIGTERM", stop);
    }
    break;
  }
  case "session":
    print(await pickle.startSession());
    break;
  case "register":
    print(await pickle.registerAccount());
    break;
  case "character":
    print(await pickle.getCharacter());
    break;
  case "profile":
    print(await pickle.getProfile());
    break;
  case "social":
    print(await pickle.getSocialSnapshot());
    break;
  case "screen": {
    const [kind, ...pairs] = args;
    if (!kind) throw new Error("screen <kind> [key=value]...");
    const query = Object.fromEntries(
      pairs.map((pair) => {
        const at = pair.indexOf("=");
        return at === -1 ? [pair, ""] : [pair.slice(0, at), pair.slice(at + 1)];
      }),
    );
    print(await pickle.getScreen(kind, query));
    break;
  }
  case "get": {
    const [path] = args;
    if (!path) throw new Error("get <path>");
    print(await pickle.request("GET", path.startsWith("/") ? path : `/${path}`));
    break;
  }
  default:
    console.error(USAGE);
    process.exit(2);
}
