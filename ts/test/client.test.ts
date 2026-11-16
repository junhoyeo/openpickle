import { expect, test } from "bun:test";
import { PickleApiError, PickleClient, decodeAccessToken } from "../src/client.ts";
import type { MessageSearchScreen } from "../src/types.ts";

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
