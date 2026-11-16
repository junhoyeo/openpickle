# picklemem-client

A typed TypeScript client for the Pickle character API (`https://character-api-prod.picklemem.com`),
the backend of the Pickle iOS app. It was built from two Proxyman captures of the iOS app
(version 1.1.19, build 202611071128) taken on 2026-11-16, 26 endpoints in all, and checked live
by exporting a full account history. The client covers 24 endpoints. It has no runtime
dependencies and uses `fetch`.

```ts
import { PickleClient } from "picklemem-client";

const pickle = new PickleClient({ accessToken: process.env.PICKLE_TOKEN! });

const { rooms } = await pickle.sync();
const characterRoom = rooms.find((room) => room.kind === "character")!;
await pickle.sendMessage(characterRoom.room_id, "안녕!");

for await (const event of pickle.events()) {
  if (event.event === "chat") console.log(event.data);
}
```

## Authentication

Every call except `getUiDefinitions()` and the refresh itself sends `Authorization: Bearer <token>`.
The token is an ES256 JWT issued by the API:

| claim | meaning |
| --- | --- |
| `sub` | user id, `user-live-<uuid>` |
| `sid` | login session id |
| `exp` | access token expiry: **5 minutes** after `iat` |
| `sexp` | login session expiry: 7 days after `iat` |

New access tokens come from `POST /api/login-sessions/refresh`. It takes a refresh token
(`{"login_session_token": "ls_…"}`, no bearer) and returns `login_session_jwt` (the access token),
`expires_at` (7 days out) and a new `login_session_token`. The app sends
`X-Pickle-Login-Session-Rotation: 1`: every refresh spends the token it was given.

The client takes either credential:

```ts
// An access token, or a function that returns a fresh one before every request.
new PickleClient({ accessToken: "eyJ…" });

// A refresh token. The client refreshes a minute before each access token expires, one refresh
// at a time, and hands every new refresh token to onRotate. Store it: a lost token ends the
// login session.
new PickleClient({
  loginSession: { token: savedToken, onRotate: (r) => save(r.login_session_token) },
});
```

**The refresh token in a capture belongs to the app on your phone.** Because each refresh spends
the token, the app and this client cannot share one chain: whichever refreshes second holds a
spent token. Using a token taken from the phone's traffic can sign the phone out (or the
phone's next refresh ends the client's chain). How the server treats a spent token was
not tested. A separate sign-in for the client would avoid this, but the sign-in calls (phone
code, Apple, Kakao, LINE) are not in the captures.

The server rejects an old token with `401 {"detail":"token_expired"}` and a missing one with
`401 {"detail":"unauthenticated"}`. Both raise `PickleApiError`; `error.detail` holds the reason.

## Identity headers

The client sends the same app headers the iOS app sends (`X-Client-Version`, `X-Client-Build`,
`X-Client-Launch-Id`, `X-Pickle-Language`, `X-Pickle-Storefront-Country`, `X-Pickle-Device-Region`,
the `User-Agent` and so on). The defaults in `IOS_PROFILE` are the values from the capture: an
English-language iPhone in Korea with the KOR storefront. Override any of them with
`profile: { ... }`. `deviceId` (sent with read cursors and device reports) and `launchId` default to
new random UUIDs; pass your own to keep them stable.

## Endpoints

All of these were in the captures. `scripts/verify-capture.ts` checks each one against them (see
[Verification](#verification)).

| Method | Path | Client method | Notes |
| --- | --- | --- | --- |
| POST | `/api/login-sessions/refresh` | `refreshLoginSession(token)` | Rotates the refresh token. See [Authentication](#authentication). |
| POST | `/api/account/register` | `registerAccount()` | Called on every launch. Returns account state; `registered: false` for an existing account. |
| POST | `/api/session` | `startSession(overrides?)` | Account, plan, credits, character, and a LiveKit room token. About 110 keys; the useful ones are typed. |
| GET | `/api/sync` | `sync()` | All rooms with `last_seq`, `read_seq`, unread counts. |
| GET | `/api/events` | `events()` | Server-sent events. See [Events](#events). |
| GET | `/api/rooms/{room}/history/delta?after_seq=&limit=` | `getHistoryDelta(room, { afterSeq, limit })`, `history(room)` | Messages, reactions and card states after a sequence number, up to 500 per page. `history()` pages from the start. |
| POST | `/api/rooms/{room}/messages` | `sendMessage(room, text, { clientMsgId?, inCall? })` | Returns `{ status, responder, seq: null }`. The reply arrives on `events()`. |
| PUT | `/api/rooms/{room}/read` | `markRead(room, seq)` | Moves this device's read cursor. |
| GET | `/api/character` | `getCharacter()` | Look, level, XP, energy, jelly. |
| GET | `/api/profile?region=` | `getProfile(region?)` | Your profile, including phone and birthday. |
| GET | `/api/social/snapshot?scopes=` | `getSocialSnapshot(scopes?)` | Friends, requests, blocked, invite, activity. Each scope has its own status. |
| GET | `/api/screens/{kind}` | `getScreen(kind, query?)` | Data for server-driven screens. `handoffs`, `extra-conversations` and `roulette-promo` are typed. |
| PUT | `/api/device/permissions` | `reportPermissions(snapshots)` | Which device data sources the app may read. |
| POST | `/api/device/location/token` | `requestLocationToken()` | `409 location_disabled` while location sharing is off. |
| GET | `/api/ui-definitions` | `getUiDefinitions()` | The server-driven UI bundle (about 2 MB). No token needed. |
| GET | `/api/secure/account` | `getSecureAccount()` | Master key id. |
| GET | `/api/secure/nodes?nonce=` | `getEnclaveNodes(nonce?)` | AWS Nitro enclave nodes with attestation documents. |
| POST | `/api/secure/nodes/{node}/status` | `getNodeStatus(node, nonce?)` | A node's signed log head. |
| GET | `/api/secure/log/nodes` | `getLogNodes()` | Every node that held a credential log for the account. |
| POST | `/api/secure/log/store/listings` | `getLogStoreListings(request)` | Presigned S3 listing URLs for the credential log. |
| GET | `/api/secure/ledger` | `getSharedLedger()` | The node ledger, encrypted with a key only your devices hold. |
| GET | `/api/secure/transfers` | `getTransfers()` | Pending master-key transfers between devices. |

`request(method, path, options)` and `send(...)` call any other path with the same headers and
error handling.

### Left out on purpose

- `POST /api/telemetry`: the app's analytics batches. It authenticates with the LiveKit token from
  `startSession()`, not the API token.
- `POST /api/secure/log/verified`: the app reports the result of its own credential-log
  verification. A client that did not run that verification should not report one.

### Not in the captures

- **Sign-in.** Only the refresh of an existing login session was captured.
- **Other screen kinds.** The UI definitions name these data kinds: `activity`, `bubble`,
  `bubbles`, `character-voices`, `character_avatar_history`, `character_colors`,
  `character_settings`, `character_sharing`, `clone_brief`, `connection-catalog`,
  `connection-detail`, `connections`, `explore-sections`, `friend`, `friend-request`,
  `handle_check`, `handoff`, `message-search`, `notification_settings`, `page`, `phone_call`,
  `scheduled-action`, `scheduled-action-run`, `scheduled-actions`, `ticket-history`, `vault`.
  Whether each one is served at `/api/screens/{kind}` is unverified. `profile` is also a data kind,
  and it is served at `/api/profile`, so the pattern does not hold for every kind.
  `getScreen(kind)` returns `unknown` for these; `getUiDefinitions().samples[kind]` shows the
  expected shape.
- **Actions.** Buttons in the server-driven UI run native `system_action`s (`friend_request_send`,
  `request_handoff`, `save_character_settings`, and about 100 more). Their endpoints live in the
  app binary and were not exercised in the capture.
- **Credential writes.** `/api/secure/vault/*`, `/api/secure/connections/*`, `/api/secure/sealing`
  and the rest of the end-to-end credential protocol. The app runs that protocol in a signed
  JavaScript program shipped in `getUiDefinitions().secure_program.source`.

## Events

`events()` opens `GET /api/events` and yields typed events until the server closes the stream.
The server first sends its current state (usage, character status, widgets, sync revision), then
changes as they happen. Reconnecting is up to the caller; the server suggests a delay with
`retry:` (1–2 seconds in the capture).

| `event` | `data.kind` | Payload |
| --- | --- | --- |
| `chat` | `user` | Your message as stored: `id` (= `client_msg_id`), `text`, `seq`, `at` |
| `chat` | `assistant` | A character reply; `text` wraps each chat bubble in `<bubble>…</bubble>` |
| `chat` | `pending` | `on`: whether the character is working on a turn |
| `chat` | `responder` | `state`: `chat_turn_started` / `chat_turn_finished` |
| `chat` | `activity` | `state`: `thinking` (with `typing`) / `tool` (with `label`) |
| `chat` | `reaction` | Emoji reaction with `target_id` = the message's `client_msg_id` |
| `room` | `advanced` | A room's new `last_seq` and its updated summary |
| `sync` | – | New `rev` of the room list; call `sync()` |
| `usage` | `energy`, `credits`, `jelly`, `plan` | Usage counters |
| `status` | – | Character status (level, XP, energy, jelly) |
| `widget` | `bgm_catalog`, `state` | Background music catalog, timer and music widget state |

An event name outside this list arrives as `{ event: "unknown", name, data }`.

## CLI

```sh
export PICKLE_TOKEN='eyJhbGciOiJFUzI1NiIs...'   # from the app's Authorization header
bun src/cli.ts token                # claims and seconds left
bun src/cli.ts sync
bun src/cli.ts send character "안녕"
bun src/cli.ts export ./messages    # full history of every room, one JSON file per room
bun src/cli.ts events               # one JSON object per line
bun src/cli.ts screen handoffs
bun src/cli.ts get /api/secure/transfers
```

## Verification

```sh
bun install
bun run typecheck
bun test
bun run verify:capture ~/Downloads/character-api-prod.picklemem.com_11-16-2026-19-15-36.proxymanlogv2
```

The client was also run against the live API with an access token from the second capture:
`export` read all 14 rooms (1,031 history items, 909 of them in the room with your own
character). The refresh call
was not made live, because it would have spent the phone's refresh token.

`verify:capture` replays a Proxyman capture without touching the network. For every recorded
request it calls the matching client method, with `fetch` replaced by one that answers with the
recorded response. It then checks that the client sent the same method, path, raw query string,
JSON body and app headers, and that it parsed the response or raised `PickleApiError` for the
recorded status. Run it on a new capture after an app update to see what changed; endpoints
without a client method are listed as `skip`.

Captures contain bearer tokens and personal data. `.gitignore` excludes `*.proxymanlogv2` and
`.capture/`; keep them out of the repository.
