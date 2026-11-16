import { expect, test } from "bun:test";
import { SseParser, toPickleEvent } from "../src/events.ts";

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
