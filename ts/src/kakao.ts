/** Public native-client identifiers supplied from a verified OAuth flow; not credentials. */
export const KAKAO_NATIVE_APP_KEY = "41aae57a320049f89503212f8452b399";
export const KAKAO_REDIRECT_URI = `kakao${KAKAO_NATIVE_APP_KEY}://oauth`;
export const KAKAO_IOS_BUNDLE_ID = "com.pickle.client";
export const KAKAO_KA = "sdk/2.24.4 sdk_type/swift os/ios-18.1 lang/ko-KR res/1179x2556 device/iPhone origin/com.pickle.client appPkg/com.pickle.client";
export interface KakaoConfig { clientId: string; redirectUri: string; iosBundleId: string; ka: string }
export const DEFAULT_KAKAO_CONFIG: KakaoConfig = {
  clientId: KAKAO_NATIVE_APP_KEY, redirectUri: KAKAO_REDIRECT_URI,
  iosBundleId: KAKAO_IOS_BUNDLE_ID, ka: KAKAO_KA,
};
export class KakaoError extends Error {
  readonly kind: "usage" | "auth" | "api" | "timeout" | "network";
  constructor(kind: KakaoError["kind"], message: string) {
    super(message); this.name = "KakaoError"; this.kind = kind;
  }
}
function base64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
export async function pkceChallenge(verifier: string): Promise<string> {
  return base64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))));
}
export async function kakaoAuthorize(config: KakaoConfig = DEFAULT_KAKAO_CONFIG) {
  const verifier = base64url(crypto.getRandomValues(new Uint8Array(32)));
  const url = new URL("https://kauth.kakao.com/oauth/authorize");
  url.search = new URLSearchParams({ response_type: "code", client_id: config.clientId,
    redirect_uri: config.redirectUri, code_challenge: await pkceChallenge(verifier),
    code_challenge_method: "S256", ka: config.ka }).toString();
  return { url: url.href, verifier, config, createdAt: Date.now() };
}
export function parseKakaoCode(input: string, redirectUri = KAKAO_REDIRECT_URI): string {
  const value = input.trim();
  const invalid = () => new KakaoError("usage", "Expected a Kakao redirect URL or a nonempty authorization code");
  if (!value || /\s/.test(value)) throw invalid();
  if (!value.includes(":")) {
    if (/[?&#/]/.test(value)) throw invalid();
    return value;
  }
  let url: URL;
  try { url = new URL(value); } catch { throw invalid(); }
  if (`${url.protocol}//${url.host}${url.pathname}` !== redirectUri || url.username || url.password || url.hash) throw invalid();
  if (url.searchParams.has("error")) throw new KakaoError("auth", "Kakao authorization was denied; start Kakao login again");
  const codes = url.searchParams.getAll("code");
  if (codes.length !== 1 || !codes[0] || /\s/.test(codes[0])) throw invalid();
  return codes[0];
}
export async function exchangeKakaoCode(
  fetchImpl: (url: string, init: RequestInit) => Promise<Response>,
  code: string, verifier: string, config: KakaoConfig = DEFAULT_KAKAO_CONFIG,
): Promise<string> {
  try {
    const response = await fetchImpl("https://kauth.kakao.com/oauth/token", {
      method: "POST", redirect: "error", signal: AbortSignal.timeout(30_000),
      headers: { "Content-Type": "application/x-www-form-urlencoded", KA: config.ka },
      body: new URLSearchParams({ grant_type: "authorization_code", client_id: config.clientId,
        redirect_uri: config.redirectUri, code, code_verifier: verifier, ios_bundle_id: config.iosBundleId }).toString(),
    });
    // Never surface OAuth bodies: they can contain credentials or reflect the submitted code.
    if (!response.ok) throw new KakaoError([400, 401, 403].includes(response.status) ? "auth" : "api", `Kakao token exchange failed (${response.status})`);
    const body = await response.json().catch(() => null) as { id_token?: unknown } | null;
    if (typeof body?.id_token !== "string" || !body.id_token) throw new KakaoError("api", "Kakao response has no id_token");
    return body.id_token;
  } catch (error) {
    if (error instanceof KakaoError) throw error;
    if (error instanceof Error && ["TimeoutError", "AbortError"].includes(error.name)) throw new KakaoError("timeout", "Kakao token exchange timed out");
    throw new KakaoError("network", "Kakao token endpoint could not be reached");
  }
}
