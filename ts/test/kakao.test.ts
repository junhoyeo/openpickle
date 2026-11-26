import { expect, test } from "bun:test";
import { PickleClient } from "../src/client.ts";
import { DEFAULT_KAKAO_CONFIG, kakaoAuthorize, parseKakaoCode, pkceChallenge } from "../src/kakao.ts";
import type { KakaoLoginOutcome } from "../src/types.ts";

const issued = { status: "ok", user_id: "fixture-user", login_session_token: "fixture-session", login_session_jwt: "fixture-jwt", expires_at: "2030-01-01" };
test("PKCE uses RFC 7636 S256 vector and fresh independent random verifiers", async () => {
  expect(await pkceChallenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")).toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  const a = await kakaoAuthorize();
  expect(a.verifier).toMatch(/^[A-Za-z0-9_-]{43}$/);
  expect(a.verifier).not.toBe((await kakaoAuthorize()).verifier);
  const url = new URL(a.url);
  expect(url.origin + url.pathname).toBe("https://kauth.kakao.com/oauth/authorize");
  expect(Object.fromEntries(url.searchParams)).toEqual({ response_type: "code", client_id: DEFAULT_KAKAO_CONFIG.clientId,
    redirect_uri: DEFAULT_KAKAO_CONFIG.redirectUri, code_challenge: await pkceChallenge(a.verifier), code_challenge_method: "S256", ka: DEFAULT_KAKAO_CONFIG.ka });
});
test("redirect parsing rejects foreign, ambiguous, denied and empty callbacks", () => {
  const uri = DEFAULT_KAKAO_CONFIG.redirectUri;
  expect(parseKakaoCode(" bare-code\n")).toBe("bare-code");
  expect(parseKakaoCode(`${uri}?code=a%2Bb`)).toBe("a+b");
  for (const value of ["", "a b", "https://evil.test?code=x", `${uri}?code=`, `${uri}?code=x&code=y`, `${uri}?error=access_denied`, `${uri}?code=x#fragment`]) {
    expect(() => parseKakaoCode(value)).toThrow();
  }
});
test("Kakao and SMS request contracts; no credentials cross hosts", async () => {
  const calls: { url: string; init: RequestInit }[] = [];
  const client = new PickleClient({ accessToken: "never-forward", fetch: async (url, init) => {
    calls.push({ url, init }); return Response.json(url.includes("kauth") ? { id_token: "fixture-id" } : issued);
  } });
  const config = { ...DEFAULT_KAKAO_CONFIG, ka: "custom/SDK" };
  expect(await client.exchangeKakaoCode("a+&", "verifier", config)).toBe("fixture-id");
  const token = calls[0]!;
  expect(token.url).toBe("https://kauth.kakao.com/oauth/token");
  expect(token.init.method).toBe("POST");
  expect(token.init.redirect).toBe("error");
  expect(new Headers(token.init.headers).get("KA")).toBe(config.ka);
  expect(new Headers(token.init.headers).get("content-type")).toBe("application/x-www-form-urlencoded");
  expect(Object.fromEntries(new URLSearchParams(String(token.init.body)))).toEqual({ grant_type: "authorization_code", client_id: config.clientId,
    redirect_uri: config.redirectUri, code: "a+&", code_verifier: "verifier", ios_bundle_id: config.iosBundleId });
  expect(await client.loginWithKakaoIdToken("fixture-id")).toEqual(issued);
  expect(JSON.parse(String(calls[1]!.init.body))).toEqual({ provider: "kakao", kind: "id_token", id_token: "fixture-id" });
  await client.verifyPhoneCode("pending", "123456");
  expect(JSON.parse(String(calls[2]!.init.body))).toEqual({ pending_id: "pending", code: "123456", kind: "phone_code" });
  for (const call of calls) expect(new Headers(call.init.headers).has("authorization")).toBe(false);
});

test("Kakao login can require phone verification without issuing a session", async () => {
  const required = {
    status: "phone_verification_required",
    identity_ticket: "fixture-ticket",
  } satisfies KakaoLoginOutcome;
  const client = new PickleClient({ fetch: async () => Response.json(required) });
  const outcome = await client.loginWithKakaoIdToken("fixture-id");
  if (!("identity_ticket" in outcome)) throw new Error("Expected a phone verification ticket");
  expect(outcome.identity_ticket).toBe("fixture-ticket");
  expect(outcome).toEqual(required);
});
