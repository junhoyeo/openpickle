// `GET /api/events`: the server-sent event stream the app keeps open while in the foreground.

import type { CharacterStatus, CreditUsage, JellyRewards, LevelMilestone, Plan, Room, UserSummary } from "./types.ts";

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
  private afterCarriageReturn = false;

  push(chunk: string): SseFrame[] {
    // A "\r" ends its line without waiting for what follows, because nothing may follow. A "\n"
    // that opens the next chunk is then the second half of the same "\r\n".
    if (this.afterCarriageReturn && chunk.startsWith("\n")) {
      chunk = chunk.slice(1);
      this.afterCarriageReturn = false;
    }
    if (chunk !== "") this.afterCarriageReturn = chunk.endsWith("\r");
    this.buffer += chunk;
    const frames: SseFrame[] = [];
    let newline: number;
    while ((newline = this.buffer.search(/\r\n|\r|\n/)) !== -1) {
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
// Typed payloads carry `v: 1`. The event name and, where present, `kind` select the shape.
// A payload that does not match a typed shape arrives as `{ event: "unknown", name, data }`.

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
  | {
      /** A partial character reply before it is assigned a room sequence number. */
      kind: "assistant_partial";
      role: string;
      id: string;
      text: string;
      at: number;
      channel: string;
      parts: number[];
    }
  | {
      /** A stored subagent result linked to a path. */
      kind: "subagent";
      role: string;
      id: string;
      title: string;
      path: string;
      at: number;
      seq: number;
      prev_seq: number;
      channel: string;
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
type RoomEvent = {
  kind: "advanced";
  room_id: string;
  last_seq: number;
  room: Omit<Room, "log_epoch" | "projection_version">;
};

type PicklePayloadEvent =
  | { event: "chat"; data: V1<ChatEvent> }
  | { event: "usage"; data: V1<UsageEvent> }
  | { event: "status"; data: V1<CharacterStatus> }
  | { event: "widget"; data: V1<WidgetEvent> }
  /** A change to the room list; call `sync()` when `rev` moves past what you hold. */
  | { event: "sync"; data: V1<{ rev: number }> }
  | { event: "room"; data: V1<RoomEvent> }
  | { event: "unknown"; name: string; data: unknown };

export type PickleEvent =
  | PicklePayloadEvent
  /** Suggested reconnect delay in milliseconds, as a non-negative safe integer. */
  | { event: "retry"; delayMs: number };

type Check<T> = (value: unknown) => value is T;
type Fields<T> = { [K in keyof T]-?: Check<T[K]> };
type Kinds<T extends { kind: string }> = {
  [K in T["kind"]]: Fields<Omit<Extract<T, { kind: K }>, "kind">>;
};

const isString = (value: unknown): value is string => typeof value === "string";
const isBoolean = (value: unknown): value is boolean => typeof value === "boolean";
const isNumber = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value);
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const isV1 = (value: unknown): value is { v: 1 } => isRecord(value) && value.v === 1;

const optional = <T>(check: Check<T>): Check<T | undefined> =>
  (value): value is T | undefined => value === undefined || check(value);
const nullable = <T>(check: Check<T>): Check<T | null> =>
  (value): value is T | null => value === null || check(value);
const arrayOf = <T>(check: Check<T>): Check<T[]> =>
  (value): value is T[] => Array.isArray(value) && value.every(check);
const recordOf = <T>(check: Check<T>): Check<Record<string, T>> =>
  (value): value is Record<string, T> => isRecord(value) && Object.values(value).every(check);

function object<T>(checks: Fields<T>): Check<T> {
  const fields: [string, Check<unknown>][] = Object.entries(checks);
  return (value): value is T => isRecord(value) && fields.every(([key, check]) => check(value[key]));
}

function variants<T extends { kind: string }>(checks: Kinds<T>): Check<T> {
  const entries: [string, Fields<Record<string, unknown>>][] = Object.entries(checks);
  const byKind = new Map(entries.map(([kind, fields]) => [kind, object(fields)] as const));
  return (value): value is T => isRecord(value) && isString(value.kind) && byKind.get(value.kind)?.(value) === true;
}

const chatMessage = { role: isString, id: isString, at: isNumber, channel: isString };
const roomSequence = { seq: isNumber, prev_seq: isNumber };
const isChat = variants<ChatEvent>({
  user: { ...chatMessage, ...roomSequence, text: isString, prev_seq: optional(isNumber) },
  assistant: { ...chatMessage, ...roomSequence, text: isString, parts: arrayOf(isNumber) },
  assistant_partial: { ...chatMessage, text: isString, parts: arrayOf(isNumber) },
  subagent: { ...chatMessage, ...roomSequence, title: isString, path: isString },
  pending: { on: isBoolean },
  responder: { state: isString },
  activity: { state: isString, label: optional(isString), typing: optional(isBoolean) },
  reaction: {
    ...roomSequence,
    id: isString,
    target_id: isString,
    part: isNumber,
    actor: isString,
    emoji: isString,
    at: isNumber,
  },
});

const creditFields = {
  allowance: isNumber,
  used: isNumber,
  remaining: isNumber,
  bonus: isNumber,
  exhausted: isBoolean,
  reset_at: isString,
  ticket_credits: isNumber,
  tickets: isNumber,
  tickets_by_kind: recordOf(isNumber),
  ticket_auto_use: isBoolean,
  period: isString,
  balance: isNumber,
  credit_auto_use: isBoolean,
  auto_use_order: isString,
  ticket_order: isString,
  credit_spending: isBoolean,
} satisfies Fields<CreditUsage>;
const isLevelMilestone = object<LevelMilestone>({ level: isNumber, amount: isNumber, full_size: isBoolean });
const levelRewardFields = {
  level_milestones: arrayOf(isLevelMilestone),
  next_level_reward_level: isNumber,
  next_level_reward_amount: isNumber,
};
const isUsage = variants<UsageEvent>({
  energy: { pct: isNumber, state: isString },
  credits: creditFields,
  jelly: {
    count: isNumber,
    rewards: object<JellyRewards>({
      ...levelRewardFields,
      day: isString,
      checkin_available: isBoolean,
      checkin_amount: isNumber,
      talk_seconds: isNumber,
      talk_milestones: Array.isArray,
      talk_claimable: isNumber,
      friends_reward_day: isString,
      notices: Array.isArray,
      purchase_granted: isBoolean,
    }),
    batch: object({ accepted: isNumber, eating: isBoolean, rev: isNumber }),
  },
  plan: { plan: isString, plan_rank: isNumber, expires_at: nullable(isString), credits: object<CreditUsage>(creditFields) },
});
const isStatus = object<CharacterStatus>({
  ...levelRewardFields,
  color_combo_id: isString,
  level: isNumber,
  xp: isNumber,
  xp_next: isNumber,
  satiety: isNumber,
  portrait_redraw_due: isBoolean,
  body_color: isString,
  line_id: isString,
  body_hex: isString,
  line_hex: isString,
  energy_pct: isNumber,
  energy_exhausted: isBoolean,
  jelly_count: isNumber,
  jelly_checkin_available: isBoolean,
  jelly_checkin_amount: isNumber,
  jelly_notice_kind: isString,
  jelly_notice_amount: isNumber,
});
const isWidget = variants<WidgetEvent>({
  bgm_catalog: {
    widget: isString,
    tracks: arrayOf(object({
      name: isString,
      title: isString,
      emoji: isString,
      sha256: isString,
      bytes: isNumber,
      group: optional(isString),
    })),
  },
  state: { widget: isString, revision: isNumber, data: isRecord },
});
const isSync = object<{ rev: number }>({ rev: isNumber });
const isRoom = variants<RoomEvent>({
  advanced: {
    room_id: isString,
    last_seq: isNumber,
    room: object<RoomEvent["room"]>({
      room_id: isString,
      kind: isString,
      title: isString,
      peer: nullable(object<UserSummary>({
        user_id: isString,
        handle: isString,
        display_name: isString,
        avatar_url: isString,
        character_avatar_url: isString,
        character_name: isString,
      })),
      member_count: isNumber,
      last_seq: isNumber,
      read_seq: isNumber,
      role: isString,
      last_event_at: isString,
      observer: isBoolean,
      unread: isNumber,
      muted: isBoolean,
      pinned_at: nullable(isString),
      peer_read_seq: nullable(isNumber),
      peer_read_at: nullable(isString),
      peer_joined_at: nullable(isString),
      request_id: isString,
      request_role: isString,
      extra_conversation_unread: isBoolean,
    }),
  },
});

/** Decodes non-empty payloads, not retry hints. Unknown events keep JSON or raw text. */
export function toPickleEvent({ event: name, data: raw }: SseFrame): PicklePayloadEvent | undefined {
  if (raw === "") return undefined;
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return { event: "unknown", name, data: raw };
  }
  if (isV1(data)) {
    switch (name) {
      case "chat":
        if (isChat(data)) return { event: name, data };
        break;
      case "usage":
        if (isUsage(data)) return { event: name, data };
        break;
      case "status":
        if (isStatus(data)) return { event: name, data };
        break;
      case "widget":
        if (isWidget(data)) return { event: name, data };
        break;
      case "sync":
        if (isSync(data)) return { event: name, data };
        break;
      case "room":
        if (isRoom(data)) return { event: name, data };
        break;
    }
  }
  return { event: "unknown", name, data };
}
