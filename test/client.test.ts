import { expect, test } from "bun:test";
import { PickleApiError, PickleClient } from "../src/client";

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
