import { SseParser, toPickleEvent, type PickleEvent } from "./events";
import type {
  ApiErrorBody,
  Character,
  EnclaveNode,
  HistoryDelta,
  KnownScreens,
  LogNodeSummary,
  LogStoreListings,
  LogStoreListingsRequest,
  PermissionSnapshot,
  Profile,
  RegisterResponse,
  SecureAccount,
  SendMessageResponse,
  SessionRequest,
  SessionResponse,
  SharedLedger,
  SignedStatement,
  SocialScope,
  SocialSnapshot,
  StatusOk,
  SyncResponse,
  UiDefinitions,
} from "./types";

export const DEFAULT_BASE_URL = "https://character-api-prod.picklemem.com";

/** What the app says about itself in request headers. */
export interface ClientProfile {
  /** `X-Client-Version` */
  appVersion: string;
  /** `X-Client-Build` */
  appBuild: string;
  /** `X-Client-Platform` (UI definitions) and `platform` (session). */
  platform: "ios" | "android";
  userAgent: string;
  /** `X-Pickle-Language` */
  language: string;
  /** `X-Pickle-Language-Source` */
  languageSource: string;
  acceptLanguage: string;
  /**
   * `X-Pickle-Storefront-Country`, an App Store storefront code such as `KOR`. When `null`, the
   * header is left out and `X-Pickle-Store-Country-Resolved` is `false`, as the app does before
   * StoreKit answers.
   */
  storefrontCountry: string | null;
  /** `X-Pickle-Device-Region` */
  deviceRegion: string;
  /** `X-Client-Timezone`, sent with session and register calls. */
  timezone: string;
  /** Hash of the UI vocabulary, `X-Pickle-Vocabulary`. */
  vocabulary: string;
}

/** The iOS app the studied traffic came from (version 1.1.19, build 202611071128, Korea). */
export const IOS_PROFILE: ClientProfile = {
  appVersion: "1.1.19",
  appBuild: "202611071128",
  platform: "ios",
  userAgent: "Pickle/202611071128 CFNetwork/3860.700.1 Darwin/25.6.0",
  language: "en",
  languageSource: "device_language",
  acceptLanguage: "en-US,en;q=0.9",
  storefrontCountry: "KOR",
  deviceRegion: "KR",
  timezone: "Asia/Seoul",
  vocabulary: "6d926b057792cf46",
};

export interface PickleClientOptions {
  /**
   * The bearer token (an ES256 JWT from the app's `Authorization` header). Access tokens expire
   * 5 minutes after issue, so long-lived use needs a function that returns a fresh one; it is
   * called once per request.
   */
  accessToken: string | (() => string | Promise<string>);
  /** The device id the app reports (iOS `identifierForVendor`). Defaults to a new random one. */
  deviceId?: string;
  /** `X-Client-Launch-Id`: one id per app launch. Defaults to a new random one. */
  launchId?: string;
  profile?: Partial<ClientProfile>;
  baseUrl?: string;
  fetch?: typeof fetch;
}

export interface RequestOptions {
  query?: Record<string, string | number | undefined>;
  /** Serialized as JSON. */
  body?: unknown;
  headers?: Record<string, string>;
  /** Send the bearer token and the app identity headers. Default `true`. */
  auth?: boolean;
  signal?: AbortSignal;
}

/** A non-2xx response. `detail` is the server's reason code when it sent one. */
export class PickleApiError extends Error {
  constructor(
    readonly method: string,
    readonly path: string,
    readonly status: number,
    readonly body: unknown,
  ) {
    const detail = (body as Partial<ApiErrorBody> | null)?.detail;
    super(`${method} ${path} failed with ${status}${detail ? `: ${detail}` : ""}`);
    this.name = "PickleApiError";
  }

  get detail(): string | undefined {
    const detail = (this.body as Partial<ApiErrorBody> | null)?.detail;
    return typeof detail === "string" ? detail : undefined;
  }
}

const ALL_SOCIAL_SCOPES = ["activity", "blocked", "friends", "invite", "requests"] as const;

export class PickleClient {
  readonly baseUrl: string;
  readonly profile: ClientProfile;
  readonly deviceId: string;
  readonly launchId: string;
  private readonly accessToken: PickleClientOptions["accessToken"];
  private readonly fetchImpl: typeof fetch;

  constructor(options: PickleClientOptions) {
    this.accessToken = options.accessToken;
    this.baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/$/, "");
    this.profile = { ...IOS_PROFILE, ...options.profile };
    this.deviceId = options.deviceId ?? crypto.randomUUID().toUpperCase();
    this.launchId = options.launchId ?? crypto.randomUUID();
    this.fetchImpl = options.fetch ?? fetch;
  }

  // ---- Account and session ----

  /**
   * `POST /api/account/register`. The app calls it on every launch; for an existing account it
   * returns the account state (`registered: false`) and changes nothing visible.
   */
  registerAccount(): Promise<RegisterResponse> {
    return this.request("POST", "/api/account/register", {
      headers: {
        "X-Pickle-Onboarding-Flow": "4",
        "X-Client-Timezone": this.profile.timezone,
        "X-Pickle-Reported-Language": this.profile.language,
      },
    });
  }

  /**
   * `POST /api/session`: account, plan, credit and character state, plus a LiveKit room token.
   * The defaults are what the app sent when it did not need a call room (`room_demand: "none"`).
   */
  startSession(overrides: Partial<SessionRequest> = {}): Promise<SessionResponse> {
    const body: SessionRequest = {
      bgm_library: [],
      app_version: this.profile.appVersion,
      vocabulary: this.profile.vocabulary,
      rig_ready: true,
      client_connect_id: crypto.randomUUID(),
      room_demand: "none",
      platform: this.profile.platform,
      app_build: this.profile.appBuild,
      device_memory_bytes: 12262113280,
      establishment_aware: true,
      world_ready: false,
      ...overrides,
    };
    return this.request("POST", "/api/session", {
      body,
      headers: { "X-Client-Timezone": this.profile.timezone },
    });
  }

  // ---- Rooms and chat ----

  /** `GET /api/sync`: every room with its cursors and unread counts. */
  sync(): Promise<SyncResponse> {
    return this.request("GET", "/api/sync");
  }

  /** `GET /api/rooms/{room}/history/delta`: what happened in a room after `afterSeq`. */
  getHistoryDelta(roomId: string, options: { afterSeq: number; limit?: number }): Promise<HistoryDelta> {
    return this.request("GET", `/api/rooms/${encodeURIComponent(roomId)}/history/delta`, {
      query: { after_seq: options.afterSeq, limit: options.limit ?? 500 },
    });
  }

  /**
   * `POST /api/rooms/{room}/messages`. The reply arrives on `events()`: `chat` events carry
   * `client_msg_id` as the message `id` and as the `target_id` of reactions to it.
   */
  async sendMessage(
    roomId: string,
    text: string,
    options: { clientMsgId?: string; inCall?: boolean } = {},
  ): Promise<SendMessageResponse & { client_msg_id: string }> {
    const clientMsgId = options.clientMsgId ?? crypto.randomUUID();
    const response = await this.request<SendMessageResponse>(
      "POST",
      `/api/rooms/${encodeURIComponent(roomId)}/messages`,
      { body: { text, client_msg_id: clientMsgId, in_call: options.inCall ?? false } },
    );
    return { ...response, client_msg_id: clientMsgId };
  }

  /** `PUT /api/rooms/{room}/read`: moves this device's read cursor. */
  markRead(roomId: string, seq: number): Promise<StatusOk> {
    return this.request("PUT", `/api/rooms/${encodeURIComponent(roomId)}/read`, {
      body: { seq, device_id: this.deviceId },
    });
  }

  /**
   * `GET /api/events`: the live event stream (chat turns, reactions, room cursors, usage, widgets).
   * The server opens with its current state. The generator ends when the server closes the stream;
   * reconnecting is up to the caller.
   */
  async *events(options: { signal?: AbortSignal } = {}): AsyncGenerator<PickleEvent> {
    const response = await this.send("GET", "/api/events", {
      headers: { Accept: "text/event-stream" },
      signal: options.signal,
    });
    if (!response.body) return;
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    const parser = new SseParser();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) return;
        for (const frame of parser.push(value)) {
          const event = toPickleEvent(frame);
          if (event) yield event;
        }
      }
    } finally {
      // Cancelling the reader propagates through the pipe and closes the connection.
      await reader.cancel().catch(() => {});
    }
  }

  // ---- Profile, character and social ----

  /** `GET /api/character`: your character's look, level, energy and jelly. */
  getCharacter(): Promise<Character> {
    return this.request("GET", "/api/character");
  }

  /** `GET /api/profile?region=`: your own profile. */
  getProfile(region = this.profile.deviceRegion): Promise<Profile> {
    return this.request("GET", "/api/profile", { query: { region } });
  }

  /** `GET /api/social/snapshot?scopes=`: friends, requests, blocks, invites and activity. */
  getSocialSnapshot<S extends SocialScope = (typeof ALL_SOCIAL_SCOPES)[number]>(
    scopes: readonly S[] = ALL_SOCIAL_SCOPES as unknown as S[],
  ): Promise<SocialSnapshot<S>> {
    return this.request("GET", "/api/social/snapshot", { query: { scopes: scopes.join(",") } });
  }

  /**
   * `GET /api/screens/{kind}`: data for a server-driven screen. `handoffs` (tasks and phone calls
   * the character ran), `extra-conversations` and `roulette-promo` were present in traffic.
   */
  getScreen<K extends keyof KnownScreens>(kind: K): Promise<KnownScreens[K]>;
  getScreen(kind: string, query?: RequestOptions["query"]): Promise<unknown>;
  getScreen(kind: string, query?: RequestOptions["query"]): Promise<unknown> {
    return this.request("GET", `/api/screens/${encodeURIComponent(kind)}`, { query });
  }

  // ---- Device ----

  /** `PUT /api/device/permissions`: reports which device data sources the app may read. */
  reportPermissions(snapshots: PermissionSnapshot[]): Promise<StatusOk> {
    return this.request("PUT", "/api/device/permissions", {
      body: { device_id: this.deviceId, snapshots },
    });
  }

  /**
   * `POST /api/device/location/token`. Fails with 409 `location_disabled` while location sharing
   * is off; the success shape was not present in traffic.
   */
  requestLocationToken(): Promise<Record<string, unknown>> {
    return this.request("POST", "/api/device/location/token", { body: { device_id: this.deviceId } });
  }

  // ---- UI definitions ----

  /** `GET /api/ui-definitions`: the server-driven UI bundle. Needs no token. */
  getUiDefinitions(): Promise<UiDefinitions> {
    return this.request("GET", "/api/ui-definitions", {
      auth: false,
      headers: { "X-Pickle-Vocabulary": this.profile.vocabulary, "X-Client-Platform": this.profile.platform },
    });
  }

  // ---- Secure (enclave credential protocol, read side) ----
  //
  // The app runs the protocol in `secure_program.source` of the UI definitions: it verifies the
  // Nitro attestation of each node and the signed log heads. These calls return the raw material.

  /** `GET /api/secure/account`: the id of the account's master key. */
  getSecureAccount(): Promise<SecureAccount> {
    return this.request("GET", "/api/secure/account");
  }

  /** `GET /api/secure/nodes?nonce=`: live enclave nodes with attestation documents bound to `nonce`. */
  getEnclaveNodes(nonce = randomNonce()): Promise<{ nodes: EnclaveNode[] }> {
    return this.request("GET", "/api/secure/nodes", { query: { nonce } });
  }

  /** `POST /api/secure/nodes/{node}/status`: the node's signed log head, bound to `nonce`. */
  getNodeStatus(node: string, nonce = randomNonce()): Promise<{ head: SignedStatement }> {
    return this.request("POST", `/api/secure/nodes/${encodeURIComponent(node)}/status`, { body: { nonce } });
  }

  /** `GET /api/secure/log/nodes`: every node that ever held a log for this account. */
  getLogNodes(): Promise<{ nodes: LogNodeSummary[] }> {
    return this.request("GET", "/api/secure/log/nodes");
  }

  /** `POST /api/secure/log/store/listings`: presigned S3 listing URLs for the credential log. */
  getLogStoreListings(request: LogStoreListingsRequest): Promise<LogStoreListings> {
    return this.request("POST", "/api/secure/log/store/listings", { body: request });
  }

  /** `GET /api/secure/ledger`: the node ledger, sealed with a key only devices hold. */
  getSharedLedger(): Promise<SharedLedger> {
    return this.request("GET", "/api/secure/ledger");
  }

  /** `GET /api/secure/transfers`: pending master-key transfers between devices. */
  getTransfers(): Promise<{ transfers: unknown[] }> {
    return this.request("GET", "/api/secure/transfers");
  }

  // ---- Transport ----

  /** Calls any endpoint and parses the JSON response. An empty body yields `{}`. */
  async request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
    const response = await this.send(method, path, options);
    const text = await response.text();
    return (text === "" ? {} : JSON.parse(text)) as T;
  }

  /** Calls any endpoint and returns the raw response. Throws `PickleApiError` unless it is 2xx. */
  async send(method: string, path: string, options: RequestOptions = {}): Promise<Response> {
    const headers: Record<string, string> = {
      Accept: "*/*",
      "Accept-Language": this.profile.acceptLanguage,
      "User-Agent": this.profile.userAgent,
      Pragma: "no-cache",
      "Cache-Control": "no-cache",
    };
    if (options.auth ?? true) Object.assign(headers, await this.identityHeaders());
    if (options.body !== undefined) headers["Content-Type"] = "application/json";
    Object.assign(headers, options.headers);

    const response = await this.fetchImpl(this.baseUrl + path + queryString(options.query), {
      method,
      headers,
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
      signal: options.signal,
    });
    if (!response.ok) {
      const text = await response.text();
      let body: unknown = text;
      try {
        body = JSON.parse(text);
      } catch {}
      throw new PickleApiError(method, path, response.status, body);
    }
    return response;
  }

  private async identityHeaders(): Promise<Record<string, string>> {
    const token = typeof this.accessToken === "function" ? await this.accessToken() : this.accessToken;
    const headers: Record<string, string> = {
      Authorization: `Bearer ${token}`,
      "X-Client-Version": this.profile.appVersion,
      "X-Client-Build": this.profile.appBuild,
      "X-Client-Launch-Id": this.launchId,
      "X-Pickle-Language": this.profile.language,
      "X-Pickle-Language-Source": this.profile.languageSource,
      "X-Pickle-Device-Region": this.profile.deviceRegion,
      "X-Pickle-Store-Country-Resolved": String(this.profile.storefrontCountry !== null),
    };
    if (this.profile.storefrontCountry !== null) {
      headers["X-Pickle-Storefront-Country"] = this.profile.storefrontCountry;
    }
    return headers;
  }
}

/** The app leaves commas unescaped (`scopes=a,b`); everything else is percent-encoded. */
function queryString(query: RequestOptions["query"]): string {
  if (!query) return "";
  const pairs = Object.entries(query)
    .filter((entry): entry is [string, string | number] => entry[1] !== undefined)
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value).replace(/%2C/g, ",")}`);
  return pairs.length ? `?${pairs.join("&")}` : "";
}

/** 32 random bytes in unpadded base64url, the nonce format the app uses. */
export function randomNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Reads the claims of a bearer token without verifying it, e.g. to see when it expires. */
export function decodeAccessToken(token: string): {
  sub: string;
  sid: string;
  iat: number;
  /** Access token expiry, Unix seconds (5 minutes after `iat`). */
  exp: number;
  /** Session expiry, Unix seconds (7 days after `iat`). */
  sexp: number;
  [claim: string]: unknown;
} {
  const payload = token.replace(/^Bearer\s+/i, "").split(".")[1];
  if (!payload) throw new Error("not a JWT");
  return JSON.parse(atob(payload.replace(/-/g, "+").replace(/_/g, "/")));
}
