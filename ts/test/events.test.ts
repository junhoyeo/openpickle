import { expect, test } from "bun:test";
import { PickleClient } from "../src/client.ts";
import { SseParser, toPickleEvent, type PickleEvent } from "../src/events.ts";

const stream =
  "retry: 2200\n\n" +
  'event: sync\ndata: {"v": 1, "rev": 3390}\n\n' +
  ": keep-alive\r\n" +
  'event: chat\r\ndata: {"v":1,"kind":"pending",\r\ndata: "on":true}\r\n\r\n' +
  'event: novel\ndata: {"v": 1}\n\n';

test("parses the same frames however the stream is split", () => {
  const whole = new SseParser().push(stream);
  expect(whole).toEqual([
    { event: "message", data: "", retry: 2200 },
    { event: "sync", data: '{"v": 1, "rev": 3390}' },
    { event: "chat", data: '{"v":1,"kind":"pending",\n"on":true}' },
    { event: "novel", data: '{"v": 1}' },
  ]);
  for (let cut = 1; cut < stream.length; cut++) {
    const parser = new SseParser();
    expect([...parser.push(stream.slice(0, cut)), ...parser.push(stream.slice(cut))]).toEqual(whole);
  }
});

test("a stream that ends lines with a bare carriage return loses no event", () => {
  const bare = 'event: sync\rdata: {"v":1,"rev":1}\r\r';
  const expected = [{ event: "sync", data: '{"v":1,"rev":1}' }];
  // The last event is complete even though no further chunk arrives.
  expect(new SseParser().push(bare)).toEqual(expected);
  for (let cut = 1; cut < bare.length; cut++) {
    const parser = new SseParser();
    expect([...parser.push(bare.slice(0, cut)), ...parser.push(bare.slice(cut))]).toEqual(expected);
  }
  // "\r" then "\n" in the next chunk is one line ending; a second "\n" is a blank line.
  const parser = new SseParser();
  expect([...parser.push("data: a\r"), ...parser.push("\n"), ...parser.push("\n")]).toEqual([
    { event: "message", data: "a" },
  ]);
});

test("types known events and keeps unknown ones", () => {
  const events = new SseParser().push(stream).map(toPickleEvent);
  expect(events).toEqual([
    undefined,
    { event: "sync", data: { v: 1, rev: 3390 } },
    { event: "chat", data: { v: 1, kind: "pending", on: true } },
    { event: "unknown", name: "novel", data: { v: 1 } },
  ]);
});

test("unknown frames do not interrupt the client event iterator", async () => {
  const futureChat = { v: 1, kind: "future" };
  const futureUsage = { v: 1, kind: "weather", temp: 3 };
  const valid = {
    v: 1 as const,
    kind: "assistant_partial" as const,
    role: "assistant",
    id: "reply-1",
    at: 1.5,
    channel: "chat",
    text: "hello",
    parts: [0, 1],
    new_field: true,
  };
  const malformedChat = { ...valid, parts: [0, "1"] };
  const raw = "plain words\nsecond line";
  const body =
    "retry: 1250\n\n" +
    "event: notice\ndata: plain words\ndata: second line\n\n" +
    `event: chat\ndata: ${JSON.stringify(futureChat)}\n\n` +
    `event: usage\ndata: ${JSON.stringify(futureUsage)}\n\n` +
    `event: chat\ndata: ${JSON.stringify(malformedChat)}\n\n` +
    `event: chat\ndata: ${JSON.stringify(valid)}\n\n`;
  const fetch = (async () =>
    new Response(body, { headers: { "content-type": "text/event-stream" } })) as unknown as typeof globalThis.fetch;
  const client = new PickleClient({ accessToken: "synthetic-token", fetch });
  const events: PickleEvent[] = [];
  for await (const event of client.events()) events.push(event);
  expect(events).toEqual([
    { event: "unknown", name: "notice", data: raw },
    { event: "unknown", name: "chat", data: futureChat },
    { event: "unknown", name: "usage", data: futureUsage },
    { event: "unknown", name: "chat", data: malformedChat },
    { event: "chat", data: valid },
  ]);
});

test("typed events require the version and the declared field shapes", () => {
  const badPayloads = [
    { event: "chat", data: null },
    { event: "chat", data: { v: 1, kind: "pending" } },
    { event: "chat", data: { v: 1, kind: "activity", state: "tool", label: null } },
    { event: "chat", data: { v: 2, kind: "pending", on: true } },
    { event: "status", data: { v: 1 } },
    { event: "sync", data: { v: 1, rev: "3" } },
    { event: "room", data: { v: 1, kind: "advanced", room_id: "room-1", last_seq: 1, room: {} } },
    { event: "widget", data: { v: 1, kind: "state", widget: "timer", revision: 2, data: [] } },
  ];
  for (const { event, data } of badPayloads) {
    expect(toPickleEvent({ event, data: JSON.stringify(data) })).toEqual({ event: "unknown", name: event, data });
  }
});
