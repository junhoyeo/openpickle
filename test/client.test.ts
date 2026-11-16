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
