<h1 align="center">openpickle</h1>

<p align="center">
  An unofficial, native desktop app for <a href="https://pickle.com">Pickle</a>.<br />
  Written in Rust, drawn on the GPU, and built from the phone app's own network traffic.
</p>

<p align="center">
  <img src="docs/screenshots/chat.png" alt="The chat with your Pickle: a question and its answer in bubbles" height="540" />
  <img src="docs/screenshots/activity.png" alt="A reply in progress: the character says it is searching the web" height="540" />
  <img src="docs/screenshots/select.png" alt="Text selected across paragraphs, with the Copy menu open" height="540" />
</p>

openpickle is not made by, or affiliated with, the company behind Pickle. It talks to the same
servers the iOS app does, with your own account.

## What it does

- **Chat with your Pickle from your Mac.** Bubbles, cards, timestamps and the composer follow the
  iOS chat screen. Korean lines break between words, and the text field supports Korean input.
- **Live.** Your message, the character's replies and what it is doing right now ("Searching
  Web…") arrive over the server's event stream as they happen.
- **Archive.** The third tab holds every picture and file of the conversation, grouped by day,
  with documents listed by name, kind and size.
- **Explore.** The second tab lists what your Pickle offers to do for you, in sections and by
  area. An offer shows how it would go about it, and Start asks for it in the chat.
- **Phone calls.** A call Pickle placed shows in the chat as a card with the end of its
  transcript. Open it to read the whole call as it goes, and request an end only after
  confirmation. Opening a call or the call list does not dial a number.
- **My Pickle.** The fourth tab shows account, usage, reset-ticket holdings and connected-service
  information. The usage card opens the detailed, read-only Usage screen.
- **Friends and invites.** The menu leads to your friends, and Invite to your invite link, how
  many invites are left and who joined with it.
- **Search.** Cmd-F, or the search button in the menu, searches your messages on Pickle's
  server. Opening a result takes you to that message, with the conversation around it to scroll
  through.
- **Select and copy.** Drag across messages to select text, or right-click for Copy, Copy Message
  and Select All. Select All takes the whole conversation, not only what is on screen.
- **Opens at once.** The app keeps the conversation on disk and draws it before it asks the
  network for anything. See [the numbers](#speed).
- **Its own sign-in.** You sign in with your phone number and a texted code. The app gets a login
  session of its own, so your phone stays signed in.

<p align="center">
  <img src="docs/screenshots/phone-card.png" alt="A call Pickle placed for you: its card in the chat, over the end of the transcript" height="540" />
  <img src="docs/screenshots/phone-call.png" alt="The transcript of a call in progress, with the keys Pickle pressed and the End call button" height="540" />
</p>

<p align="center">
  <img src="docs/screenshots/explore.png" alt="The Explore tab: what your Pickle offers to do" height="380" />
  <img src="docs/screenshots/search.png" alt="Search results with the matching text in blue" height="380" />
  <img src="docs/screenshots/sign-in.png" alt="Sign-in with a phone number" height="380" />
</p>

Not built yet: editing account/settings, spending or buying tickets, direct microphone voice chat with Pickle, opening a picture full size, saving or sharing a
file, link previews,
reactions, answering a request card, and
everything that changes your friends: adding one, answering a request, a friend's own page.
The [desktop README](crates/openpickle-desktop/README.md) has the full list.

## Searching your chat from coding agents

[`skills/pickle-sessions`](skills/pickle-sessions/SKILL.md) is an agent skill that searches and
reads the conversation the desktop app keeps on disk (the history log in
`~/Library/Application Support/openpickle/cache/`). It finds messages by text, role and date, shows
the messages around a hit, and says how old the cache is. It only reads, needs nothing but the
Python standard library, and never opens the login session or the network:

```sh
python3 skills/pickle-sessions/scripts/pickle-sessions search "dinner" --since 2026-11-01
```

## Get it

```sh
# A disk image with OpenPickle.app (Apple Silicon):
./crates/openpickle-desktop/macos/package.sh      # writes target/macos/OpenPickle-<version>-arm64.dmg

# Or run it from source:
cargo run --release -p openpickle-desktop
```

The app is signed ad hoc, which is enough for the Mac that built it. Another Mac refuses it until
it is signed with a Developer ID and notarized. Building needs a recent stable Rust (1.98 was
used) and Xcode.

## Speed

The message list only lays out the rows near the viewport, and a message's text is parsed once,
off the UI thread. So a long history costs almost nothing at launch:

| Sample conversation | First frame after `main()` |
| --- | --- |
| 8 messages | about 146 ms |
| 2,000 messages | about 153 ms |

Release build on an Apple Silicon Mac, median of six runs of the `gallery` example, which draws
the chat screen with made-up messages. GPUI's own start-up is about 67 ms of that.

## How it was built

The API is not documented. It was read off seven [Proxyman](https://proxyman.com) captures of the
iOS app. Each client is checked by replaying the captured requests it covers and comparing
what it sends with what the app sent: method, path, raw query string, JSON body and headers.
The verifier reports uncovered routes separately instead of treating them as passing.

```sh
cargo run -p openpickle-api --example verify_capture -- <capture.proxymanlogv2>
cd ts && bun run verify:capture <capture.proxymanlogv2>
```

The notes on the API (endpoints, phone sign-in, login sessions, the event stream) are in
[`ts/README.md`](ts/README.md).

## What is in the repository

| Path | What it is |
| --- | --- |
| [`crates/openpickle-desktop`](crates/openpickle-desktop/README.md) | The desktop app: GPUI views, a backend thread for the network and the disk, the macOS packaging. |
| `crates/openpickle-api` | The Rust client for the captured Pickle API: phone sign-in, login session refresh, typed event stream, message search, attachments and phone-call transcripts/actions. |
| [`ts/`](ts/README.md) | The TypeScript client the Rust one was ported from, a small CLI, and the API notes. |

## Chat links and attachment previews

- Room cards and `pickle://chat?room_id=…` links open a conversation listed by the
  signed-in account's `sync()` response. The menu's **Other chats** lists those rooms;
  **Chat** returns to your own character. Observer rooms have no send composer.
- Room history, pending messages, drafts, read cursors and live updates stay scoped to
  their room. Unscoped character events are not inserted into a peer conversation.
- Generated file links are resolved through the message's attachment map. Clicking a
  file card or its linked name opens a **thumbnail preview**. For a PDF this is its
  **first page**, not the original document. Loading failures show a retry action.
- **Original-file opening, downloading and sharing are not implemented.** The captures
  show only the thumbnail endpoint, not an original-file download route. Open the
  original in the Pickle phone app until that route has been captured.

## Your data

- The app stores a login session and a copy of your conversation under
  `~/Library/Application Support/openpickle`, readable by you only. Signing out removes both.
- The login session holds a single-use refresh token that changes on every refresh. Only one
  copy of the app may run, and the session file should not be copied to another tool.
- Captures, exported messages and session files hold bearer tokens and personal data. In this
  repository they belong in `.capture/`, which git ignores.
