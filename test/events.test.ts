import { expect, test } from "bun:test";
import { SseParser, toPickleEvent } from "../src/events";

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

test("types known events and keeps unknown ones", () => {
  const events = new SseParser().push(stream).map(toPickleEvent);
  expect(events).toEqual([
    undefined,
    { event: "sync", data: { v: 1, rev: 3390 } },
    { event: "chat", data: { v: 1, kind: "pending", on: true } },
    { event: "unknown", name: "novel", data: { v: 1 } },
  ]);
});
