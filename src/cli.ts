#!/usr/bin/env bun
// Small CLI over PickleClient. The token comes from PICKLE_TOKEN (the app's Authorization header,
// with or without "Bearer "). Output is JSON.

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { PickleClient, decodeAccessToken } from "./client";

const USAGE = `usage: PICKLE_TOKEN=<jwt> pickle <command>

  token                        claims and remaining lifetime of PICKLE_TOKEN
  sync                         rooms with cursors and unread counts
  history <room> [after_seq]   room history after a sequence number (default 0)
  send <room> <text>           send a message; <room> may be "character"
  export <dir>                 write the full history of every room to <dir>/<room_id>.json
  events                       stream live events, one JSON object per line
  session | register | character | profile | social
  screen <kind> [key=value]... server-driven screen data, e.g. "screen handoffs"
  get <path>                   GET any path, e.g. "get /api/secure/transfers"`;

const [command, ...args] = process.argv.slice(2);
const token = process.env.PICKLE_TOKEN?.replace(/^Bearer\s+/i, "");
if (!command || !token) {
  console.error(USAGE);
  process.exit(2);
}

const pickle = new PickleClient({ accessToken: token });
const print = (value: unknown) => console.log(JSON.stringify(value, null, 2));

async function resolveRoom(room: string): Promise<string> {
  if (room !== "character") return room;
  const { rooms } = await pickle.sync();
  const own = rooms.find((r) => r.kind === "character");
  if (!own) throw new Error("no character room in /api/sync");
  return own.room_id;
}

switch (command) {
  case "token": {
    const claims = decodeAccessToken(token);
    print({ ...claims, expires_in_s: claims.exp - Math.floor(Date.now() / 1000) });
    break;
  }
  case "sync":
    print(await pickle.sync());
    break;
  case "history": {
    const [room, afterSeq = "0"] = args;
    if (!room) throw new Error("history <room> [after_seq]");
    print(await pickle.getHistoryDelta(await resolveRoom(room), { afterSeq: Number(afterSeq) }));
    break;
  }
  case "send": {
    const [room, ...words] = args;
    if (!room || words.length === 0) throw new Error("send <room> <text>");
    print(await pickle.sendMessage(await resolveRoom(room), words.join(" ")));
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
  case "events":
    for await (const event of pickle.events()) console.log(JSON.stringify(event));
    break;
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
    const query = Object.fromEntries(pairs.map((pair) => pair.split("=", 2) as [string, string]));
    print(await pickle.getScreen(kind, query));
    break;
  }
  case "get": {
    const [path] = args;
    if (!path) throw new Error("get <path>");
    print(await pickle.request("GET", path));
    break;
  }
  default:
    console.error(USAGE);
    process.exit(2);
}
