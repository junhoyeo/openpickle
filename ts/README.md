# @openpickle/client

A typed TypeScript client for the Pickle character API (`https://character-api-prod.picklemem.com`),
the backend of the Pickle iOS app. It was built from seven Proxyman captures of the iOS app
(version 1.1.19, build 202611071128) taken on 2026-11-16 and 2026-11-17, 39 endpoints in all. A
full history export and login session refreshes were run against the live API; phone sign-in and
phone-call methods have only been replayed against captures. The client covers 34 endpoints. It has no runtime dependencies and
uses `fetch`. It runs on Bun, and on Node versions that load TypeScript directly (tested on
Node 26); older Node needs a bundler.

```ts
import { PickleClient } from "@openpickle/client";

const pickle = new PickleClient({ accessToken: process.env.PICKLE_TOKEN! });

const { rooms } = await pickle.sync();
const characterRoom = rooms.find((room) => room.kind === "character")!;
await pickle.sendMessage(characterRoom.room_id, "안녕!");

for await (const event of pickle.events()) {
  if (event.event === "chat") console.log(event.data);
}
```

## Authentication

Every call except sign-in, the refresh and `getUiDefinitions()` sends `Authorization: Bearer <token>`.
The token is an ES256 JWT issued by the API:

| claim | meaning |
| --- | --- |
| `sub` | user id, `user-live-<uuid>` |
| `sid` | login session id |
| `exp` | access token expiry: **5 minutes** after `iat` |
| `sexp` | login session expiry: 7 days after `iat` |

### Signing in with a phone number

```ts
const signedOut = new PickleClient();
const { pending_id } = await signedOut.requestPhoneCode("+821012345678"); // texts a 6-digit code
const issued = await signedOut.verifyPhoneCode(pending_id, "123456");

const pickle = new PickleClient({
  loginSession: { token: issued.login_session_token, onRotate: (r) => save(r.login_session_token) },
});
```

- `POST /api/login-sessions/phone-code` takes `{phone, region, carrier}` and returns
  `{pending_id, expires_in: 600}`. The app sends the SIM's carrier (`lgu` was seen). Whether the
  server needs it is untested, so the client sends `carrier` only when you pass it.
- `POST /api/login-sessions` takes `{pending_id, code, kind: "phone_code"}` and returns
  `{status: "issued", user_id, login_session_token, login_session_jwt, expires_at}`.

A new sign-in starts its own login session and leaves the others alone: after the phone signed in
again, an older session kept working. So the client and the phone app can both stay signed in.
The app also offers Apple, Kakao and LINE sign-in; those calls are not in the captures.

### Login sessions

A login session is a chain of refresh tokens (`ls_…`). `POST /api/login-sessions/refresh` takes
`{"login_session_token": "ls_…"}` and returns `login_session_jwt` (the access token), `expires_at`
and the next `login_session_token`. Every refresh spends the token it was given. A spent token
gets `401 {"detail":"session_not_found"}`; the chain itself keeps working for whoever holds the
current token. `expires_at` is 7 days after the refresh, so a session lasts as long as it is
refreshed at least once a week.

A client built with `loginSession` refreshes a minute before each access token expires, one
refresh at a time, and hands every new refresh token to `onRotate`. If persistence fails, the
client retries saving that same replacement before making another refresh. An internally
managed bearer rejected with `401 token_expired` is retried at most once. Store the refresh
token: losing it ends the login session. Two programs must not share one chain, because whichever refreshes second
holds a spent token. That is also why a refresh token copied out of the phone's traffic signs the
phone out.

A client can also be built with `accessToken` (a token, or a function that returns a fresh one
before every request) for one-off calls with a token copied from a capture.

Any non-2xx response raises `PickleApiError`, with `error.detail` set when the body has a reason.
A live probe with an expired access token got `401 {"detail":"token_expired"}`, and one without a
token got `401 {"detail":"unauthenticated"}`.

## Identity headers

The client sends the same app headers the iOS app sends (`X-Client-Version`, `X-Client-Build`,
`X-Client-Launch-Id`, `X-Pickle-Language`, `X-Pickle-Storefront-Country`, `X-Pickle-Device-Region`,
the `User-Agent` and so on). The defaults in `IOS_PROFILE` are the values from the capture: an
English-language iPhone in Korea with the KOR storefront. Override any of them with
`profile: { ... }`. `deviceId` (sent with read cursors and device reports) and `launchId` default to
new random UUIDs; pass your own to keep them stable. Transport headers are left to the runtime's
`fetch`; under Node it also adds `sec-fetch-mode` and its own `accept-encoding`.

## Endpoints

All of these were in the captures. `scripts/verify-capture.ts` checks each one against them (see
[Verification](#verification)).

| Method | Path | Client method | Notes |
| --- | --- | --- | --- |
| POST | `/api/login-sessions/phone-code` | `requestPhoneCode(phone, { region?, carrier? })` | Texts a sign-in code. No token needed. |
| POST | `/api/login-sessions` | `verifyPhoneCode(pendingId, code)` | Trades the code for a login session. No token needed. |
| POST | `/api/login-sessions/refresh` | `refreshLoginSession(token)` | Rotates the refresh token. See [Authentication](#authentication). |
| POST | `/api/account/register` | `registerAccount()` | Called on every launch. Returns account state; `registered: false` for an existing account. |
| POST | `/api/session` | `startSession(overrides?)` | Account, plan, credits, character, and a LiveKit room token. About 100 keys; the useful ones are typed. |
| GET | `/api/sync` | `sync()` | All rooms with `last_seq`, `read_seq`, unread counts. |
| GET | `/api/events` | `events()` | Server-sent events. See [Events](#events). |
| GET | `/api/rooms/{room}/history` | `getHistory(room)` | The newest messages of a room (up to 200), reactions inline. The app calls it after a fresh sign-in. |
| GET | `/api/rooms/{room}/history/delta?after_seq=&limit=` | `getHistoryDelta(room, { afterSeq, limit })`, `history(room)` | Messages, reactions and card states after a sequence number, up to 500 per page. `history()` pages from the start. |
| GET | `/api/rooms/{room}/history/page?limit=&before_seq=` | `getHistoryPage(room, { beforeSeq, limit })` | Up to 250 messages before a sequence number, with reactions and card states. See [History pages and attachments](#history-pages-and-attachments). |
| GET | `/api/attachments/{path}/thumbnail` | `getAttachmentThumbnail(path)` | A JPEG preview of a photo or PDF in a message. Returns `{ bytes, contentType }`. |
| GET | `/api/attachments/{path}` | `getAttachment(path)` | The attachment itself, the original file. Returns `{ bytes, contentType }`. |
| POST | `/api/rooms/{room}/messages` | `sendMessage(room, text, { clientMsgId?, inCall?, context? })` | Returns `{ status, responder, seq: null }`. The reply arrives on `events()`. `context` marks a message sent from an [Explore](#explore) offer. |
| PUT | `/api/rooms/{room}/read` | `markRead(room, seq)` | Moves this device's read cursor. |
| GET | `/api/phone-calls/{call_id}` | `getPhoneCall(callId)` | Current call state and transcript. The id is encoded as one path segment. |
| POST | `/api/actions` | `endPhoneCall(callId, { actionId?, inCall? })` | Sends only the captured `end_phone_call` action. Reuse `actionId` for a retry. |
| GET | `/api/character` | `getCharacter()` | Look, level, XP, energy, jelly. |
| GET | `/api/profile?region=` | `getProfile(region?)` | Your profile, including phone and birthday. |
| GET | `/api/social/snapshot?scopes=` | `getSocialSnapshot(scopes?)` | Friends, requests, blocked, invite, activity. Each scope has its own status. |
| GET | `/api/screens/{kind}` | `getScreen(kind, query?)` | Data for server-driven screens. `handoffs`, `extra-conversations`, `roulette-promo`, `message-search` and `explore-sections` are typed. |
| GET | `/api/screens/message-search?query=` | `searchMessages(query)` | Messages containing the text, newest first. See [Message search](#message-search). |
| GET | `/api/screens/explore-sections` | `getExploreSections()` | Things the character offers to do, in sections. See [Explore](#explore). |
| POST | `/api/offers/impressions` | `reportOfferImpressions(offerIds)` | Reports which offers were shown. Answered with `{}`. |
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

## Phone calls

`getPhoneCall(callId)` returns the current `phone_call` state and the transcript collected so far.
A line's `speaker` is `me` or `them`; `keys` appears on keypad lines. Start, answer, end, duration,
and outcome fields are optional because the server adds them as the call moves through its
lifecycle. A live response may later set `hangup_requested` while the end action is being handled.

Ending is always explicit. `endPhoneCall()` sends `{ action_id, in_call, payload: { call_id },
kind: "end_phone_call" }`. It creates a UUID when `actionId` is omitted. Pass the same `actionId`
when retrying the same user action. The CLI exposes only the read-only `phone-call <id>` command;
it does not expose call-ending mutation. Native callers must own confirmation, fresh-call
verification, and stable retry state before using the library method.

No phone call was created, read, or ended live while implementing this support. The capture was
replayed only against local in-memory clients and localhost mock servers.

### Left out on purpose

- `POST /api/telemetry`: the app's analytics batches. It authenticates with the LiveKit token from
  `startSession()`, not the API token.
- `POST /api/push-token`: registers the device's Apple push token. A client without push
  notifications has nothing to register.
- `POST /api/secure/log/verified`: the app reports the result of its own credential-log
  verification. A client that did not run that verification should not report one.
- `GET /api/screens/connection-catalog`: the catalog of services the character can connect to,
  for the app's connections screen. `getScreen("connection-catalog")` returns it untyped.

### Not in the captures

- **Apple, Kakao and LINE sign-in.** Only phone sign-in was captured.
- **Other screen kinds.** The UI definitions name these data kinds: `activity`, `bubble`,
  `bubbles`, `character-voices`, `character_avatar_history`, `character_colors`,
  `character_settings`, `character_sharing`, `clone_brief`, `connection-detail`, `connections`,
  `friend`, `friend-request`, `handle_check`, `handoff`, `notification_settings`, `page`,
  `phone_call`, `scheduled-action`, `scheduled-action-run`, `scheduled-actions`, `ticket-history`,
  `vault`.
  Whether each one is served at `/api/screens/{kind}` is unverified. `profile` is also a data kind,
  and it is served at `/api/profile`, so the pattern does not hold for every kind.
  `getScreen(kind)` returns `unknown` for these; `getUiDefinitions().samples[kind]` shows the
  expected shape.
- **Other actions.** The seventh capture exercised only `kind: "end_phone_call"`. The client does
  not expose a generic action method and does not guess contracts for other native actions.
- **Phone dialing and audio.** No direct dial endpoint or live audio transport was captured.
  `getPhoneCall()` reads status/transcript data, and `endPhoneCall()` ends an existing call; these
  methods are not a dialer or a microphone/audio client.
- **Credential writes.** `/api/secure/vault/*`, `/api/secure/connections/*`, `/api/secure/sealing`
  and the rest of the end-to-end credential protocol. The app runs that protocol in a signed
  JavaScript program shipped in `getUiDefinitions().secure_program.source`.

## Message search

`searchMessages(query)` calls `GET /api/screens/message-search?query=<text>`, the app's message
search. The query is the only parameter; the client percent-encodes it as the app does (UTF-8,
`%20` for a space). The response:

```json
{
  "kind": "message-search",
  "items": [
    {
      "id": "chat:42",
      "kind": "chat",
      "segments": [
        { "text": "…see you at ", "match": false },
        { "text": "lunch", "match": true },
        { "text": " tomorrow…", "match": false }
      ],
      "label": "Chat",
      "date_label": "Nov 3",
      "room_id": "",
      "call_id": "",
      "seq": 42,
      "line_id": ""
    }
  ]
}
```

`segments` is a snippet of the message around the match, split into runs; `match` marks the runs
that matched. Joined, the runs give the snippet, usually cut short with `…` at either end. `seq`
is the message's sequence number in its room, and `id` is `chat:<seq>`.

What the capture showed (5 searches, 115 results, all with Korean queries):

- Every result had every field above. All were `kind: "chat"`, `label: "Chat"`, messages in the
  chat with your own character, with `room_id`, `call_id` and `line_id` empty.
- Results came newest first, at most 30 per search. No paging parameter was sent or returned.
- A result can have several matching runs, and two matching runs can be adjacent.

What it did not show: results of another `kind`. `call_id` and `line_id` suggest the search also
covers phone call transcripts, and a non-empty `room_id` would point to another room, but none
were seen, so `kind` is typed as an open string. Queries with spaces or ASCII text were not
captured either; the client encodes them the same way.

## Explore

The app's Explore screen lists things the character offers to do for you ("offers"), such as
finding money you are owed or watching flight prices. `getExploreSections()` calls
`GET /api/screens/explore-sections`, which takes no parameters. The response, shortened:

```json
{
  "kind": "explore-sections",
  "areas": ["money", "shopping", "health", "people", "work", "travel", "home", "fun"],
  "popular_ids": ["3f2a…"],
  "sections": [
    {
      "section_id": "popular",
      "title": "Popular lately",
      "items": [
        {
          "offer_id": "3f2a…",
          "title": "Find a refund",
          "detail": "Check recent orders for a refund you are owed.",
          "cta_label": "Find refunds I am owed",
          "icon_image": "https://….cloudfront.net/….png",
          "icon_symbol": "banknote",
          "area": "money",
          "accepted_users_label": "1,234 people asked for this",
          "steps": ["Read order emails", "List refunds"],
          "areas": ["money"]
        }
      ],
      "preview_count": 1,
      "expand_count": 0,
      "areas": ["money"]
    }
  ],
  "more_items": [{ "offer_id": "…", "section_id": "money_back", "…": "every offer field" }],
  "ranked_items": [{ "offer_id": "3f2a…", "section_id": "popular", "…": "every offer field but steps" }]
}
```

- `sections[].items` holds every offer of a section. The app shows the first `preview_count`
  (3, or fewer when the section is smaller) and the rest after the section is expanded.
- `more_items` holds each offer that is in no section's preview, once, with the `section_id` it
  belongs to. A section's `expand_count` is the number of its offers in `more_items`.
- `ranked_items` holds every offer once, with its `section_id` and without `steps`, in the server's
  order.
- `popular_ids` are the offer ids of the `popular` section, in order. An offer can be in two
  sections, e.g. `popular` and its own area's section.
- `icon_symbol` is an SF Symbol name; every offer of an area had the same one.

`reportOfferImpressions(offerIds)` calls `POST /api/offers/impressions` with
`{"offer_ids": [...]}`, and the server answers `{}`. The app sent it once, after Explore loaded,
with the preview offers of the first sections.

Tapping an offer sends a message to your character: its text is the offer's `cta_label`, and the
body carries the offer as context. Pass the same to `sendMessage`:

```ts
await pickle.sendMessage(room, offer.cta_label, { context: { id: offer.offer_id, kind: "offer" } });
// {"text":"…","client_msg_id":"…","in_call":false,"context":{"id":"<offer_id>","kind":"offer"}}
```

Without `context`, the body has no `context` key, as before.

What the capture showed (one Explore session): three identical Explore responses, each with
8 areas, 14 sections, 176 section items (173 distinct offers), 134 `more_items` and 173
`ranked_items`. Every offer had every field; one had an empty `title`, one an empty
`accepted_users_label`, many had no `steps`, and one had two `areas`. One impressions report with
9 ids, and one message sent from an offer.

What it did not show: a section id or area outside these, a `context.kind` other than `offer`
(so both are open strings), an Explore response that changed between loads, and whether the
server needs the impressions report before an offer message is accepted.

## History pages and attachments

`getHistoryPage(room, { beforeSeq, limit })` calls
`GET /api/rooms/{room}/history/page?limit=250&before_seq=<seq>`, the request the app uses to
scroll back through a room. The parameters go in that order, `limit` first. The response:

```json
{
  "v": 1,
  "messages": [{ "role": "user", "id": "m-7", "text": "older", "at": 1700000000, "seq": 7 }],
  "reactions": [{ "seq": 9, "target_id": "m-7", "part": 0, "actor": "assistant", "emoji": "👍", "at": 1700000070 }],
  "card_states": [{ "seq": 12, "id": "req-1", "state": "answered", "option_id": "yes" }],
  "has_more": true,
  "next_before": 7
}
```

`messages` are the messages before `before_seq`, oldest first, and have no inline `reactions`.
While `has_more` is true, `next_before` is the oldest `seq` of the page: pass it as the next
`beforeSeq`. The `attachments` CLI command starts from `getHistory()` and pages back from its
oldest message the same way.

Messages refer to photos and files by attachment path, `attachments/<date>/<file name>`:

- `attachments`: the paths a message of yours carries.
- `image_meta`: path to `{ w, h, blurhash }`, for photos of yours and images the character sent.
- `file_meta`: path to `{ name, size, pages? }` (`size` in bytes, `pages` for PDFs), for files
  the character sent.
- `inline_images` and `inline_files`: a path the character wrote in its reply (such as
  `downloads/report.pdf`) to the attachment path it was stored under.

`getAttachmentThumbnail(path)` calls `GET /api/attachments/{path}/thumbnail` and returns
`{ bytes, contentType }`, a JPEG. Each path segment is percent-encoded on its own, so
`attachments/2026-01-02/여행 계획.pdf` becomes
`/api/attachments/attachments/2026-01-02/%EC%97%AC%ED%96%89%20%EA%B3%84%ED%9A%8D.pdf/thumbnail`.
The app sends this request with the bearer token and the common headers, but without its
`X-Client-*` and `X-Pickle-*` headers, and so does the client.

What the capture showed (one session): 4 history pages of the character room, back to its first
message (44, 250, 250 and 165 messages, the last with `has_more: false`). The app started below
the room's `last_seq`, at messages it did not hold yet, and did not call `getHistory()` first.
On the last page `next_before` was 1, not the oldest `seq`. Card states were `answered`,
`expired` or `superseded`. 12 thumbnails, all `200 image/jpeg`: 9 of photos and 3 of PDFs with
Korean file names, two of them with spaces. Across the pages, 7 messages had `attachments`, 12 had `image_meta`, and one had
`file_meta` and `inline_files` with PDF, spreadsheet and text files.

What it did not show: a thumbnail of a spreadsheet or text file, a thumbnail request that failed,
a `limit` other than 250, and paging in a room other than your character's.

`getAttachment(path)` calls `GET /api/attachments/{path}`, the same path without `/thumbnail`
and with the same headers, and returns the file itself. A later capture showed it once, when a
PDF was opened in the app: `200 application/pdf` with `Content-Length`, `Accept-Ranges: bytes`
and `Content-Disposition: attachment; filename="…"`. Not seen: a file of another type, a range
request, and a file large enough to need one.

## Events

`events()` opens `GET /api/events` and yields typed events until the server closes the stream.
The server first sends its current state (usage, character status, widgets, sync revision), then
changes as they happen. Reconnecting is up to the caller. The server suggests a delay with
`retry:` (1.25 s and 2.2 s in the captures), but `events()` does not pass that value on.

| `event` | `data.kind` | Payload |
| --- | --- | --- |
| `chat` | `user` | Your message as stored: `id` (= `client_msg_id`), `text`, `seq`, `at` |
| `chat` | `assistant` | A stored character reply; `text` wraps each chat bubble in `<bubble>…</bubble>` |
| `chat` | `assistant_partial` | A partial reply before the server assigns its room sequence number |
| `chat` | `subagent` | A stored subagent result with its `title` and `path` |
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
# A login session file: {"login_session_token": "ls_…"}. The CLI locks the file's session
# for the full command, including a running events stream.
export PICKLE_SESSION_FILE=~/.config/pickle/login-session.json
bun src/cli.ts login +821012345678  # texts a code, asks for it, writes the session file
# Or an access token from the app's Authorization header, valid for 5 minutes:
# export PICKLE_TOKEN='eyJhbGciOiJFUzI1NiIs...'

bun src/cli.ts sync
bun src/cli.ts send character "안녕"
bun src/cli.ts phone-call synthetic-call-id       # read status and transcript only
bun src/cli.ts search 점심         # one result per line: seq, date, label, snippet with [matches]
bun src/cli.ts explore travel       # Explore offers by section: title, call to action, how many asked
bun src/cli.ts attachments character  # photos and files, newest first: date, kind, name, size, path
bun src/cli.ts thumbnail "attachments/2026-01-02/scan 1.pdf" scan.jpg  # JPEG thumbnail of one
bun src/cli.ts download "attachments/2026-01-02/scan 1.pdf" scan.pdf   # the file itself
bun src/cli.ts export ./messages    # full history of every room, one JSON file per room
bun src/cli.ts events               # one JSON object per line
bun src/cli.ts screen handoffs
bun src/cli.ts get /api/secure/transfers
bun src/cli.ts token                # claims and seconds left of PICKLE_TOKEN
```

Never point `PICKLE_SESSION_FILE` at the desktop app's own login-session file. Give this CLI
its own file. The CLI refuses to run when its sibling `.lock` directory already exists; if a
crash left it behind, remove it only after confirming no other CLI is using that session.
`download` streams into a temporary file and only replaces the chosen output after success.

## Verification

```sh
bun install
bun run typecheck
bun test
bun run verify:capture ~/Downloads/character-api-prod.picklemem.com_11-17-2026-02-42-21.proxymanlogv2
```

The client was also run against the live API. With an access token from the second capture,
`export` read all 14 rooms (1,031 history items, 909 of them in the room with your own
character). Then, with the owner's consent, the client took over the refresh token from that
capture: refreshes in a row each returned a working access token and the next refresh token, all
within the same login session (`sid`), and `export` ran again through `PICKLE_SESSION_FILE`. The
third capture shows the other side of that: the phone's next refresh, with its spent token, got
`401 session_not_found`, and the client's chain still refreshed afterwards. Phone sign-in has not
been run live; it needs a code texted to the owner's phone.

`verify:capture` replays a Proxyman capture without touching the network. For every recorded
request it calls the matching client method, with `fetch` replaced by one that answers with the
recorded response. It then checks that the client sent the same method, path, raw query string,
JSON body and app headers, and that it parsed the response or raised `PickleApiError` for the
recorded status. Run it on a new capture after an app update to see what changed; endpoints
without a client method are listed as `skip`.

Captures contain bearer tokens and personal data. `.gitignore` excludes `*.proxymanlogv2` and
`.capture/`; keep them out of the repository.

### Kakao login (library)

`PickleClient` exposes `requestKakaoAuthorizeUrl`, `exchangeKakaoCode`, and
`loginWithKakaoIdToken`, plus `parseKakaoCode` from `src/kakao.ts`. The flow is
Kakao authorize with PKCE, a `kakao<client id>://oauth?code=…` redirect, a token
exchange at `kauth.kakao.com` with no client secret or App Attest, and
`POST /api/login-sessions` with `{provider: "kakao", kind: "id_token"}`. Public
configuration defaults are named constants in `src/kakao.ts`; they are native-client
identifiers, not secrets. SMS login is unchanged.

### Rust / desktop terminal entry point

Quit the desktop app, then run from the repository root:

```sh
cargo run -p openpickle-desktop --bin openpickle -- --kakao-login
```

Open the printed URL, capture its redirect, and paste the redirect URL or code into
that same terminal. The verifier stays in memory. This writes through the desktop
`Store::start_session` path, clearing old account caches and preserving the normal
rotation behavior; restart the desktop app afterwards. It uses the same public
`PICKLE_KAKAO_*` overrides. Do not point the TS CLI at the desktop session file.

The Rust API exposes `request_kakao_authorize_url`, `exchange_kakao_code`, and
`login_with_kakao_id_token`, plus `KakaoConfig::parse_code`. The minimal desktop
entry point is terminal-based: GPUI Kakao buttons, macOS `CFBundleURLTypes`
registration, and incoming-URL event routing are still pending. The bundle does not
claim the Kakao URL scheme until a working in-app receiver is wired up.
