# openpickle-desktop

A native desktop app for chatting with your Pickle. It is written in Rust and draws with
[GPUI](https://gpui.rs), the GPU-rendered UI framework of the Zed editor, so there is no webview.

```sh
cargo run --release -p openpickle-desktop            # the app
./crates/openpickle-desktop/macos/package.sh         # a DMG for this Mac
./crates/openpickle-desktop/windows/package.ps1      # a Windows x86-64 zip (in PowerShell)
cargo run -p openpickle-desktop --example gallery    # the chat screen with made-up messages
cargo run -p openpickle-desktop --example gallery -- login
cargo run -p openpickle-desktop --example gallery -- app   # the whole window: menu, search
```

## What it does

- **Sign-in** with a phone number and the texted code. The app gets its own login session, so the
  phone stays signed in.
- **The chat with your character**, following the Figma frames of the iOS chat screen: grouped
  bubbles, info cards, request cards, timestamps, the composer and the tab bar. Korean lines break
  between words.
- **Live updates** over the server's event stream: your message, the character's replies and what
  it is doing appear as they happen.
- **Sending**: a message shows at once and is replaced by the server's copy when it arrives.
- **Archive** (the third tab, or Cmd-4, and in the menu): every picture and file of the
  conversation, grouped by day, newest first. Media is a grid of three; a file is a row with its
  preview, name, kind, pages and size. Opening one goes to that message in the chat. Until the
  whole history has been read, an empty page says it is still reading.
- **Explore** (the second tab, or Cmd-2): the offers the server lists, as the app shows them.
  "For you" previews each section, with See all, and lists the rest under "More to explore"; an
  area's chip lists that area's offers by rank. An offer opens a sheet with its steps, and
  Start sends the offer's text to the chat with the offer's id. Offers that stay on screen are
  reported to the server, as the app does.
- **Phone calls**: live/completed calls placed by Pickle, drawn as the phone app draws them. In
  the chat a call is a card with its name, the other side's number, how long it lasted and the
  end of its transcript; a live call's card is read again every four seconds. A click opens the
  transcript, where Cmd-C copies it and a live call can be ended after a confirmation. The
  calls are also listed behind the composer's phone button, in the menu, and with Cmd-Shift-P.
- **My Pickle** (the fourth tab, Cmd-5): account name, daily usage, reset time, full/half reset
  ticket holdings and server-reported connection count. The Usage card opens a scrollable
  detail page. Credits/plan events update the displayed snapshot as they arrive.
  Usage percentages and tickets come from the server, not the reference screenshots.
- **Friends** (in the menu, or Cmd-3): who asked to be friends, and your friends, with a search
  field from ten friends on.
- **Invite** (the header button): your invite link with Copy link, how many invites are left,
  and the people who joined with it or invited you.
- **Searching**: Cmd-F, or the menu button and then the search button. The server searches;
  the results show one line of each message with the match marked. The arrow keys and Enter, or
  a click, open a result: the conversation scrolls to that message and stays there, so what was
  said before and after is a scroll away. The search keeps its text and results for the next
  time it is opened.
- **Selecting and copying**: drag across messages, or right-click for Copy, Copy Message and
  Select All. Cmd-C and Cmd-A work too when the composer has no selection or text of its own. A
  selection is read from the conversation, not from the rows on screen, so Select All copies all
  of it.

Not built: the menu's 3D World and direct microphone voice chat with Pickle; opening a picture full size, saving or sharing a file, the + that
makes documents; website preview cards; reactions; answering a request card. No capture shows the requests for deleting an offer, adding a friend,
answering a friend request or a friend's own page, so those are left out, as are the contacts
list, sharing the invite through other apps and the Korean roulette promotion. The texts of the
new screens are English readings of the app's string keys; the strings were not captured.

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

## Profile data and limits

Profile loads the captured account, session-bootstrap and connection-catalog routes. Only a
small `profile-view.json` snapshot is cached; the bootstrap token and session response are
never persisted in it. Usage continues to update from the existing event stream.

Reset-ticket use/buy, auto-use edits, account edits, payment methods, notification/language/
appearance settings and the mobile widget are visibly unavailable on desktop. No mutation
endpoints for these controls were supplied. Ticket history is also unavailable: its sample
layout is in the UI definitions, but none of the supplied captures includes a history request
or history payload. OpenPickle does not invent a request or turn missing history into an empty
history. Connected counts describe the server catalog and do not include device-only mobile
permissions such as address-book access.


## Phone-call history and live transcripts

The phone button beside an empty composer, the menu's Phone calls row, or Cmd-Shift-P
opens calls placed by Pickle. This is not microphone chat with Pickle and does not dial a
number. A call can be opened from the list or the drawer's recent-call entries.

Only the opened call is refreshed, every four seconds while its server state remains live.
Closing the view or returning to the list stops that refresh and never ends the call.
Ending a live call requires the explicit confirmation dialog. The same action id is reused
when retrying a failed end request; a pending request cannot be submitted twice. All call
responses are matched to their call id before updating the visible view.

API contracts were observed in the supplied capture: GET /api/phone-calls/{id} and
POST /api/actions with kind=end_phone_call. No dialer, microphone transport, full recording
download, or unsupported action endpoint is inferred from these routes.

## Packaging

`macos/package.sh` builds `OpenPickle.app` and a disk image into `target/macos/`. Pass
`aarch64-apple-darwin` or `x86_64-apple-darwin` to choose an architecture; with no argument it
builds for the host. It uses macOS's own tools (`sips`, `iconutil`, `codesign`, `hdiutil`) and
signs the app ad hoc.

`windows/package.ps1` builds the MSVC release binary and writes a zip containing
`OpenPickle.exe` and its install note into `target/windows/`. Windows uses its native title bar
and Segoe UI, with the system font fallback for Korean text. See the
[root README](../../README.md#get-it) for the unsigned-app first-launch steps on both platforms.

## Why it is fast

- The backend thread starts before the window. It sends the conversation from the disk cache to
  the UI before it asks the network for anything, and the UI applies it before its first frame.
- The message list is virtual: only the rows near the viewport are laid out, so a long history
  costs almost nothing.
- A message's text is parsed once, on the backend thread, when it is loaded. Drawing never parses.
- The network and the disk never run on the UI thread.

The `gallery` example prints its launch times. A release build on an Apple Silicon Mac drew its
first frame about 146 ms after `main()` with 8 messages and about 153 ms with 2,000 (median of six
runs each; GPUI's own start-up is about 67 ms of that).

## Where things live

Everything is under the user's application-support directory
(`~/Library/Application Support/openpickle` on macOS, readable by you only, or
`%APPDATA%\openpickle` on Windows, inside your user profile):

- `session.json`: the login session. Its refresh token is single-use and
  replaced on every refresh, so the file is written atomically and only one instance of the app
  may run. Deleting it signs the app out.
- `cache/`: the room history as an append-only log, the character's picture, the last answers
  for Friends and Explore, the pictures of friends' characters and of offers, and a few small
  files. Deleting it only makes the next launch fetch everything again. Signing out clears it.
  The log's format (`room.json`, `room-<id>.jsonl`, `identity.json`) is also read by the
  [`pickle-sessions`](../../skills/pickle-sessions/SKILL.md) skill, so a change to it should change
  the skill too.

## Code map

| File | Role |
| --- | --- |
| `src/backend.rs` | The backend thread: sign-in, cache, catch-up through `history/delta`, the event stream, sending. |
| `src/protocol.rs` | The `Command` and `Update` messages between the UI and the backend. |
| `src/store.rs` | The session file, the cache, the single-instance lock. |
| `src/convert.rs`, `src/content.rs` | History items to messages; message Markdown to bubbles and cards. |
| `src/model.rs` | Messages to list rows: grouping, joined corners, timestamps. |
| `src/app.rs` | The root view: sign-in or chat, applying updates. |
| `src/ui/` | The sections (chat, Explore, Friends), what opens over them (the menu, search, the invite sheet), the sign-in screen, the text field (with IME), row drawing, text selection, line breaking. |
| `src/theme.rs`, `assets/icons/` | Colors and sizes read off the Figma frames; Lucide icons (ISC). |
