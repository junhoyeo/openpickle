// `GET /api/events`: the server-sent event stream the app keeps open while in the foreground.

import type { CharacterStatus, CreditUsage, JellyRewards, Plan, Room } from "./types.ts";

/** One parsed SSE frame. `data` is the raw text, lines joined with `\n`. */
export interface SseFrame {
  event: string;
  data: string;
  id?: string;
  retry?: number;
}

/** Incremental SSE parser (WHATWG event-stream format). Feed it text chunks in any split. */
export class SseParser {
  private buffer = "";
  private event = "";
  private data: string[] = [];
  private id: string | undefined;
  private retry: number | undefined;

  push(chunk: string): SseFrame[] {
    this.buffer += chunk;
    const frames: SseFrame[] = [];
    let newline: number;
    while ((newline = this.buffer.search(/\r\n|\r|\n/)) !== -1) {
      // A lone trailing "\r" may be the first half of "\r\n"; wait for more input.
      if (this.buffer[newline] === "\r" && newline === this.buffer.length - 1) break;
      const line = this.buffer.slice(0, newline);
      const width = this.buffer.startsWith("\r\n", newline) ? 2 : 1;
      this.buffer = this.buffer.slice(newline + width);
      const frame = this.line(line);
      if (frame) frames.push(frame);
    }
    return frames;
  }

  private line(line: string): SseFrame | undefined {
    if (line === "") return this.dispatch();
    if (line.startsWith(":")) return;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") this.event = value;
    else if (field === "data") this.data.push(value);
    else if (field === "id") this.id = value;
    else if (field === "retry" && /^\d+$/.test(value)) this.retry = Number(value);
  }

  private dispatch(): SseFrame | undefined {
    const frame: SseFrame = { event: this.event || "message", data: this.data.join("\n") };
    if (this.id !== undefined) frame.id = this.id;
    if (this.retry !== undefined) frame.retry = this.retry;
    const empty = this.data.length === 0 && this.retry === undefined;
    this.event = "";
    this.data = [];
    this.retry = undefined;
    return empty ? undefined : frame;
  }
}

// ---- Typed events ----
//
// Every payload carries `v: 1`. The event name plus `kind` select the shape. The names and kinds
// below were present in traffic. Others may exist, so a payload can carry a `kind` this union does
// not list. An event name outside this list arrives as `{ event: "unknown", name, data }`.

export type ChatEvent =
  | {
      kind: "user";
      role: string;
      /** The `client_msg_id` the message was sent with. */
      id: string;
      text: string;
      /** Unix seconds */
      at: number;
      seq: number;
      channel: string;
      prev_seq?: number;
    }
  | {
      /** A character reply. `text` wraps each chat bubble in `<bubble>…</bubble>`. */
      kind: "assistant";
      role: string;
      id: string;
      text: string;
      at: number;
      seq: number;
      channel: string;
      parts: number[];
      prev_seq: number;
    }
  | { kind: "pending"; on: boolean }
  | { kind: "responder"; state: "chat_turn_started" | "chat_turn_finished" | (string & {}) }
  | { kind: "activity"; state: "thinking" | "tool" | (string & {}); label?: string; typing?: boolean }
  | {
      kind: "reaction";
      id: string;
      seq: number;
      target_id: string;
      part: number;
      actor: string;
      emoji: string;
      at: number;
      prev_seq: number;
    };

export type UsageEvent =
  | { kind: "energy"; pct: number; state: string }
  | ({ kind: "credits" } & CreditUsage)
  | {
      kind: "jelly";
      count: number;
      rewards: JellyRewards;
      batch: { accepted: number; eating: boolean; rev: number };
    }
  | {
      kind: "plan";
      plan: Plan["id"];
      plan_rank: number;
      /** `null` in every observed event (free plan). */
      expires_at: string | null;
      credits: CreditUsage;
    };

export type WidgetEvent =
  | {
      kind: "bgm_catalog";
      widget: string;
      tracks: { name: string; title: string; emoji: string; sha256: string; bytes: number; group?: string }[];
    }
  | { kind: "state"; widget: string; revision: number; data: Record<string, unknown> };

type V1<T> = T & { v: 1 };

export type PickleEvent =
  | { event: "chat"; data: V1<ChatEvent> }
  | { event: "usage"; data: V1<UsageEvent> }
  | { event: "status"; data: V1<CharacterStatus> }
  | { event: "widget"; data: V1<WidgetEvent> }
  /** A change to the room list; call `sync()` when `rev` moves past what you hold. */
  | { event: "sync"; data: V1<{ rev: number }> }
  | {
      event: "room";
      data: V1<{
        kind: "advanced";
        room_id: string;
        last_seq: number;
        room: Omit<Room, "log_epoch" | "projection_version">;
      }>;
    }
  | { event: "unknown"; name: string; data: unknown };

const KNOWN_EVENTS = new Set(["chat", "usage", "status", "widget", "sync", "room"]);

/** Parses a frame's JSON payload. Frames without data (the leading `retry:` frame) yield nothing. */
export function toPickleEvent(frame: SseFrame): PickleEvent | undefined {
  if (frame.data === "") return undefined;
  const data: unknown = JSON.parse(frame.data);
  if (!KNOWN_EVENTS.has(frame.event)) return { event: "unknown", name: frame.event, data };
  return { event: frame.event, data } as PickleEvent;
}
