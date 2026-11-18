import { expect, test } from "bun:test";
import { PickleApiError, PickleClient, decodeAccessToken } from "../src/client.ts";
import type {
  ExploreSectionsScreen,
  HandoffScreen,
  HistoryPage,
  MessageSearchScreen,
  Offer,
  PhoneCallTranscript,
} from "../src/types.ts";

function recorder(status: number, body: string) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(body, { status, headers: { "content-type": "application/json" } });
  }) as unknown as typeof globalThis.fetch;
  return { calls, fetch };
}

test("asks the token function for a token on every request", async () => {
  const { calls, fetch } = recorder(200, '{"rooms":[],"rev":1}');
  let n = 0;
  const pickle = new PickleClient({ accessToken: async () => `t${++n}`, fetch });
  await pickle.sync();
  await pickle.sync();
  const auth = calls.map((c) => (c.init.headers as Record<string, string>).Authorization);
  expect(auth).toEqual(["Bearer t1", "Bearer t2"]);
});

test("turns an error response into PickleApiError with the server's reason", async () => {
  const { fetch } = recorder(401, '{"detail":"token_expired"}');
  const pickle = new PickleClient({ accessToken: "expired", fetch });
  const error = await pickle.getCharacter().catch((e: unknown) => e);
  expect(error).toBeInstanceOf(PickleApiError);
  expect((error as PickleApiError).status).toBe(401);
  expect((error as PickleApiError).detail).toBe("token_expired");
});

test("refreshes a login session once for concurrent requests and hands over the next token", async () => {
  const claims = (exp: number) => btoa(JSON.stringify({ exp })).replace(/=+$/, "");
  const paths: string[] = [];
  const fetch = (async (url: string, init: RequestInit) => {
    const { pathname } = new URL(url);
    paths.push(pathname);
    const body =
      pathname === "/api/login-sessions/refresh"
        ? { login_session_jwt: `h.${claims(Date.now() / 1000 + 300)}.s`, expires_at: "", login_session_token: "ls_2" }
        : { rooms: [], rev: 1, auth: (init.headers as Record<string, string>).Authorization };
    return new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
  }) as unknown as typeof globalThis.fetch;
  const rotated: string[] = [];
  const pickle = new PickleClient({
    loginSession: { token: "ls_1", onRotate: (r) => void rotated.push(r.login_session_token) },
    fetch,
  });
  await Promise.all([pickle.sync(), pickle.sync(), pickle.sync()]);
  await pickle.sync();
  expect(paths.filter((p) => p === "/api/login-sessions/refresh")).toHaveLength(1);
  expect(rotated).toEqual(["ls_2"]);
});

test("a header override replaces the generated header whatever its case", async () => {
  const { calls, fetch } = recorder(200, "{}");
  const pickle = new PickleClient({ accessToken: "generated", fetch });
  await pickle.request("GET", "/api/sync", { headers: { authorization: "Bearer override" } });
  const sent = Object.entries(calls[0]!.init.headers as Record<string, string>);
  expect(sent.filter(([name]) => name.toLowerCase() === "authorization")).toEqual([["authorization", "Bearer override"]]);
});

test("query options join a path that already has a query", async () => {
  const { calls, fetch } = recorder(200, "{}");
  const pickle = new PickleClient({ accessToken: "t", fetch });
  await pickle.request("GET", "/api/profile?region=KR", { query: { scopes: "a,b" } });
  await pickle.request("GET", "/api/profile", { query: { region: "KR" } });
  await pickle.request("GET", "/api/profile", { query: {} });
  expect(calls.map((call) => call.url.replace(pickle.baseUrl, ""))).toEqual([
    "/api/profile?region=KR&scopes=a,b",
    "/api/profile?region=KR",
    "/api/profile",
  ]);
});

test("token claims are read as UTF-8", () => {
  const claims = { sub: "피클", sid: "s", iat: 1, exp: 2, sexp: 3 };
  const bytes = new TextEncoder().encode(JSON.stringify(claims));
  const payload = btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  expect(decodeAccessToken(`Bearer h.${payload}.s`)).toEqual(claims);
});

test("searchMessages percent-encodes the query and returns the snippet runs", async () => {
  const body: MessageSearchScreen = {
    kind: "message-search",
    items: [
      {
        id: "chat:42",
        kind: "chat",
        segments: [
          { text: "…내일 ", match: false },
          { text: "점심 메뉴", match: true },
          { text: " 정하자…", match: false },
        ],
        label: "Chat",
        date_label: "Nov 3",
        room_id: "",
        call_id: "",
        seq: 42,
        line_id: "",
      },
    ],
  };
  const { calls, fetch } = recorder(200, JSON.stringify(body));
  const pickle = new PickleClient({ accessToken: "t", fetch });
  const result = await pickle.searchMessages("점심 메뉴");
  await pickle.searchMessages("a&b=c,d");
  expect(calls.map((call) => call.url.replace(pickle.baseUrl, ""))).toEqual([
    "/api/screens/message-search?query=%EC%A0%90%EC%8B%AC%20%EB%A9%94%EB%89%B4",
    "/api/screens/message-search?query=a%26b%3Dc,d",
  ]);
  expect(calls[0]!.init.method).toBe("GET");
  expect(result).toEqual(body);
  expect(result.items[0]!.segments.filter((s) => s.match).map((s) => s.text)).toEqual(["점심 메뉴"]);
});

const offer: Offer = {
  offer_id: "offer-1",
  title: "Find a refund",
  detail: "Check recent orders for a refund you are owed.",
  cta_label: "Find refunds I am owed",
  icon_image: "https://example.com/icons/money.png",
  icon_symbol: "banknote",
  area: "money",
  accepted_users_label: "1,234 people asked for this",
  steps: ["Read order emails", "List refunds"],
  areas: ["money"],
};

test("getExploreSections reads the Explore screen", async () => {
  const { steps: _steps, ...ranked } = offer;
  const body: ExploreSectionsScreen = {
    kind: "explore-sections",
    areas: ["money", "fun"],
    popular_ids: ["offer-1"],
    sections: [{ section_id: "popular", title: "Popular", items: [offer], preview_count: 1, expand_count: 0, areas: ["money"] }],
    more_items: [],
    ranked_items: [{ ...ranked, section_id: "popular" }],
  };
  const { calls, fetch } = recorder(200, JSON.stringify(body));
  const pickle = new PickleClient({ accessToken: "t", fetch });
  expect(await pickle.getExploreSections()).toEqual(body);
  expect(calls[0]!.url).toBe(`${pickle.baseUrl}/api/screens/explore-sections`);
  expect(calls[0]!.init.method).toBe("GET");
  expect(calls[0]!.init.body).toBeUndefined();
});

test("reportOfferImpressions sends the shown offer ids", async () => {
  const { calls, fetch } = recorder(200, "{}");
  const pickle = new PickleClient({ accessToken: "t", fetch });
  expect(await pickle.reportOfferImpressions(["offer-1", "offer-2"])).toEqual({});
  expect(calls[0]!.url).toBe(`${pickle.baseUrl}/api/offers/impressions`);
  expect(calls[0]!.init.method).toBe("POST");
  expect(calls[0]!.init.body).toBe('{"offer_ids":["offer-1","offer-2"]}');
});

test("sendMessage sends a context only when given one", async () => {
  const { calls, fetch } = recorder(200, '{"status":"ok","responder":"chat","seq":null}');
  const pickle = new PickleClient({ accessToken: "t", fetch });
  await pickle.sendMessage("room-1", "hi", { clientMsgId: "m1" });
  await pickle.sendMessage("room-1", offer.cta_label, { clientMsgId: "m2", context: { id: offer.offer_id, kind: "offer" } });
  expect(calls.map((call) => call.init.body)).toEqual([
    '{"text":"hi","client_msg_id":"m1","in_call":false}',
    '{"text":"Find refunds I am owed","client_msg_id":"m2","in_call":false,"context":{"id":"offer-1","kind":"offer"}}',
  ]);
});

test("getHistoryPage asks for messages before a sequence number, limit first", async () => {
  const body: HistoryPage = {
    v: 1,
    messages: [
      { role: "user", id: "m-7", text: "older", at: 1700000000, seq: 7, attachments: ["attachments/2026-01-02/scan 1.pdf"] },
      { role: "assistant", id: "m-8", text: "<bubble>reply</bubble>", at: 1700000060, seq: 8 },
    ],
    reactions: [{ seq: 9, target_id: "m-7", part: 0, actor: "assistant", emoji: "👍", at: 1700000070 }],
    card_states: [],
    has_more: true,
    next_before: 7,
  };
  const { calls, fetch } = recorder(200, JSON.stringify(body));
  const pickle = new PickleClient({ accessToken: "t", fetch });
  expect(await pickle.getHistoryPage("room-1", { beforeSeq: 12, limit: 2 })).toEqual(body);
  await pickle.getHistoryPage("room-1", { beforeSeq: 7 });
  expect(calls.map((call) => call.url.replace(pickle.baseUrl, ""))).toEqual([
    "/api/rooms/room-1/history/page?limit=2&before_seq=12",
    "/api/rooms/room-1/history/page?limit=250&before_seq=7",
  ]);
  expect(calls[0]!.init.method).toBe("GET");
});

test("getHandoff reads progress with open node kinds and encodes the name", async () => {
  const body: HandoffScreen = {
    kind: "handoff",
    state: "running",
    title: "🧭 Plan a day trip",
    requested_at_epoch_ms: 1_700_000_000_000,
    latest_log_text: "Comparing routes",
    latest_log_kind: "web",
    handoff_estimate_pct: 50,
    plan_blocks: [{ type: "prose", markdown: "## Plan\nCompare two routes." }],
    log_entries: [],
    log_nodes: [
      { kind: "future_kind", text: "Checking an option" },
      { kind: "report", text: "A useful result" },
    ],
  };
  const { calls, fetch } = recorder(200, JSON.stringify(body));
  const pickle = new PickleClient({ accessToken: "synthetic-token", fetch });

  expect(await pickle.getHandoff("day trip/서울?")).toEqual(body);
  expect(calls[0]!.url).toBe(
    `${pickle.baseUrl}/api/screens/handoff?name=day%20trip%2F%EC%84%9C%EC%9A%B8%3F`,
  );
  expect(calls[0]!.init.method).toBe("GET");
  expect(calls[0]!.init.body).toBeUndefined();
});

test("getPhoneCall reads live, ending, and completed transcript shapes and encodes the id as one segment", async () => {
  const live: PhoneCallTranscript = {
    kind: "phone_call",
    call_id: "synthetic-live",
    title: "Active call",
    subtitle_lead: "Speaking with",
    counterpart_label: "Example contact",
    subtitle_tail: "",
    started_label: "Now",
    live: true,
    hangup_requested: false,
    footer: "Call in progress",
    lines: [
      {
        id: "line-1",
        line_id: "line-1",
        speaker: "me",
        text: "Synthetic transcript line",
        keys: "12#",
        joins_previous: false,
        joins_next: false,
      },
    ],
    started_at_epoch_ms: 1_700_000_000_000,
  };
  const ending: PhoneCallTranscript = {
    ...live,
    call_id: "synthetic-ending",
    live: true,
    hangup_requested: true,
    footer: "Ending call",
    answered_at_epoch_ms: 1_700_000_001_000,
  };
  const completed: PhoneCallTranscript = {
    ...ending,
    call_id: "synthetic-completed",
    live: false,
    hangup_requested: false,
    footer: "Call ended",
    ended_at_epoch_ms: 1_700_000_061_000,
    duration_s: 60,
    outcome: "answered",
  };
  const replies = [live, ending, completed];
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(JSON.stringify(replies[calls.length - 1]), {
      headers: { "content-type": "application/json" },
    });
  }) as unknown as typeof globalThis.fetch;
  const pickle = new PickleClient({ accessToken: "synthetic-token", fetch });

  expect(await pickle.getPhoneCall("live/id ?")).toEqual(live);
  expect(await pickle.getPhoneCall("ending-id")).toEqual(ending);
  expect(await pickle.getPhoneCall("completed-id")).toEqual(completed);
  expect(calls[0]!.url).toBe(`${pickle.baseUrl}/api/phone-calls/live%2Fid%20%3F`);
  expect(calls.every((call) => call.init.method === "GET" && call.init.body === undefined)).toBe(true);
});

test("endPhoneCall sends the captured action and preserves a retry action id", async () => {
  const { calls, fetch } = recorder(200, '{"status":"ok"}');
  const pickle = new PickleClient({ accessToken: "synthetic-token", fetch });

  expect(await pickle.endPhoneCall("synthetic-call", { actionId: "stable-action", inCall: false })).toEqual({
    status: "ok",
  });
  expect(calls[0]!.url).toBe(`${pickle.baseUrl}/api/actions`);
  expect(calls[0]!.init.method).toBe("POST");
  expect(JSON.parse(String(calls[0]!.init.body))).toEqual({
    action_id: "stable-action",
    in_call: false,
    payload: { call_id: "synthetic-call" },
    kind: "end_phone_call",
  });
});

test("getAttachmentThumbnail encodes each path segment and returns the JPEG", async () => {
  const jpeg = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]);
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(jpeg, { headers: { "content-type": "image/jpeg" } });
  }) as unknown as typeof globalThis.fetch;
  const pickle = new PickleClient({ accessToken: "t", fetch });
  const thumbnail = await pickle.getAttachmentThumbnail("attachments/2026-01-02/여행 계획 v2.pdf");
  expect(calls[0]!.url).toBe(
    `${pickle.baseUrl}/api/attachments/attachments/2026-01-02/%EC%97%AC%ED%96%89%20%EA%B3%84%ED%9A%8D%20v2.pdf/thumbnail`,
  );
  expect(Object.keys(calls[0]!.init.headers as Record<string, string>)).not.toContain("X-Client-Version");
  expect((calls[0]!.init.headers as Record<string, string>).Authorization).toBe("Bearer t");
  expect(thumbnail).toEqual({ bytes: jpeg, contentType: "image/jpeg" });
});

test("getAttachment asks for the file itself, by the same path without a suffix", async () => {
  const pdf = new TextEncoder().encode("%PDF-1.7 made-up bytes");
  const calls: { url: string; init: RequestInit }[] = [];
  const fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(pdf, { headers: { "content-type": "application/pdf" } });
  }) as unknown as typeof globalThis.fetch;
  const pickle = new PickleClient({ accessToken: "t", fetch });
  const file = await pickle.getAttachment("attachments/2026-01-02/여행 계획 v2.pdf");
  expect(calls[0]!.url).toBe(
    `${pickle.baseUrl}/api/attachments/attachments/2026-01-02/%EC%97%AC%ED%96%89%20%EA%B3%84%ED%9A%8D%20v2.pdf`,
  );
  expect(Object.keys(calls[0]!.init.headers as Record<string, string>)).not.toContain("X-Client-Version");
  expect((calls[0]!.init.headers as Record<string, string>).Authorization).toBe("Bearer t");
  expect(file).toEqual({ bytes: pdf, contentType: "application/pdf" });
});
