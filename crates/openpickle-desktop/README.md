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

- **Sign-in** with a phone number and the texted code, or Kakao in your browser on macOS. The app gets its own login session, so the
  phone stays signed in.
- **The chat with your character**, following the Figma frames of the iOS chat screen: grouped
  bubbles, info cards, request cards, timestamps, the composer and the tab bar. Korean lines break
  between words.
- **Live updates** over the server's event stream: your message, the character's replies and what
  it is doing appear as they happen.
- **Sending**: a message shows at once and is replaced by the server's copy when it arrives.
- **Archive** (the third tab, or Cmd-4, and in the menu): pictures and files from loaded
  conversation history, grouped by day, newest first. Media is a grid of three; a file is a row
  with its preview, name, kind, pages and size. Files can open directly in a preview panel or
  download from Archive; **Show in chat** still jumps to the source message. Backfill pauses
  when you leave Archive and resumes when you reopen it.
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
  the results show one line of each message with the match marked. Select a result with the
  arrow keys and Enter, or click it. An uncached result loads its nearby history before the
  conversation scrolls to it. Search keeps its text and results when reopened.
- **Selecting and copying**: drag across messages, or right-click for Copy, Copy Message and
  Select All. Cmd-C and Cmd-A work too when the composer has no selection or text of its own. A
  selection is read from the loaded conversation, not just from the rows on screen.

The 3D World menu item is disabled. There is no direct microphone chat with Pickle, upload or
sharing action, or website preview card. Existing reactions and quoted replies are displayed
when available, but the desktop does not send reactions or answer request cards. Friends and invites show lists and let you copy an invite link; there is no
friend-request management, friend detail page, contact import, or sharing through other apps.
The desktop controls are in English; there is no full localization.

## Chat links and attachment previews

- Room cards and `pickle://chat?room_id=…` links open a conversation listed by the
  signed-in account's `sync()` response. The menu's **Other chats** lists those rooms;
  **Chat** returns to your own character. Observer rooms have no send composer.
- Room history, pending messages, drafts, read cursors and live updates stay scoped to
  their room. Unscoped character events are not inserted into a peer conversation.
- Generated file links are resolved through the message's attachment map. Clicking a
  file card or its linked name opens a **thumbnail preview**. For a PDF this is its
  **first page**, not the original document. Loading failures show a retry action.
- **Download** saves the original to Downloads, even if there is no thumbnail. The app chooses
  a free filename (such as `report (1).pdf`) rather than overwriting an existing file. Use
  **Show in Finder** or **Show in folder** to find the saved file. Transfers show progress and
  can be canceled. Open saved images inside the panel; selected safe document types can be
  opened explicitly in the system application. There is no Save As or sharing action.

## Profile data and limits

Profile loads the captured account, session-bootstrap and connection-catalog routes. Only a
small `profile-view.json` snapshot is cached; the bootstrap token and session response are
never persisted in it. Usage continues to update from the existing event stream.

Desktop does not implement spending or buying reset tickets, editing auto-use, account changes,
payment methods, notification/language/appearance settings, or the mobile widget. Ticket
history has a sample layout but no implemented data fetch. The app does not present missing
history as an empty history. The connection count comes from the server catalog and excludes
device-only permissions such as address-book access.

## Phone-call history and live transcripts

The phone button beside an empty composer, the menu's Phone calls row, or Cmd-Shift-P
opens calls placed by Pickle. This is not microphone chat with Pickle and does not dial a
number. A call can be opened from the list or the drawer's recent-call entries.

The opened transcript and live call cards in chat are polled about every four seconds, while
the server reports those calls as live. Closing the transcript or returning to the call list
stops that view's refresh; live chat cards can continue to update. Closing the view does not
end the call. Ending a live call requires the explicit confirmation dialog. The same action id
is reused when retrying a failed end request; a pending request cannot be submitted twice. All
call responses are matched to their call id before updating the visible view.

The implemented routes are `GET /api/phone-calls/{id}` and `POST /api/actions` with
`kind=end_phone_call`. These routes do not establish contracts for a dialer, microphone chat
or recording downloads. The release audit checked End call offline, not on a live call.

## Packaging

Use the [published releases](https://github.com/junhoyeo/openpickle/releases) for Apple Silicon,
Intel Mac and Windows x86-64 builds. The [root README](../../README.md#get-it) lists the available
downloads, first-launch steps and platform verification limits.

`macos/package.sh` builds `OpenPickle.app` and a disk image into `target/macos/`. Pass
`aarch64-apple-darwin` or `x86_64-apple-darwin` to choose an architecture; with no argument it
builds for the host. It uses macOS's own tools (`sips`, `iconutil`, `codesign`, `hdiutil`) and
signs the app ad hoc.

`windows/package.ps1` builds the MSVC release binary and writes a zip containing
`OpenPickle.exe` and its install note into `target/windows/`. Windows uses its native title bar
and Segoe UI, with the system font fallback for Korean text.

## Startup and rendering

The backend handles network requests and chat history on a separate thread. For a returning
session it can send cached messages to the UI before refreshing from the network. The list
only lays out rows near the viewport, but that does not eliminate cache reads, message
conversion or building the list of rows.

The [`gallery` benchmark](../../README.md#speed) measures time from a timer set after the
sample messages are prepared to the first chat frame. It does not measure installed-app
startup or the cost of reading and converting real cached history.

## Where things live

Everything is under the user's application-support directory
(`~/Library/Application Support/openpickle` on macOS, readable by you only, or
`%APPDATA%\openpickle` on Windows, inside your user profile):

- `session.json`: the login session. Its refresh token is single-use and
  replaced on every refresh, so the file is written atomically and only one instance of the app
  may use the data directory at a time. Removing this file alone leaves the cache on disk.
- `cache/`: the room history as an append-only log, the character's picture, the last answers
  for Friends and Explore, the pictures of friends' characters and of offers, and a few small
  files. See [Your data](../../README.md#your-data) for the published app's session cleanup
  behavior. The log's format (`room.json`, `room-<id>.jsonl`, `identity.json`) is also read by the
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

## Kakao sign-in on macOS

1. Quit any running OpenPickle instance, including `cargo run`.
2. Run `./crates/openpickle-desktop/macos/package.sh` from the repository root.
3. Open the generated DMG in `target/macos/`, drag OpenPickle to Applications, and launch
   `/Applications/OpenPickle.app`. Replace an older installed copy if necessary.
4. On the signed-out phone form, click **Continue with Kakao**. If already signed in, use
   the app's Sign out control first (this clears the local session and cache).
5. Finish Kakao sign-in in the system browser and allow it to open OpenPickle. The app
   exchanges the callback and enters the same signed-in chat flow as SMS login.

Each Kakao attempt registers OpenPickle as the default handler of the callback scheme before
opening the browser. This needs an installed `.app` and macOS 12 or later; it should take over
from KakaoCatcher without removing KakaoCatcher.app. Registration failure is shown in the login
form. `cargo run` has no application bundle and cannot reliably receive the callback.

The browser's **Continue** may appear stuck while macOS delivers the scheme to the app, just
as with KakaoCatcher. Check the OpenPickle window. **Cancel** returns to the phone form and
discards the pending attempt; retry from the app if needed. PKCE stays in memory only, so
quitting/restarting the app invalidates an unfinished login. Codes, tokens, and verifiers are
not logged. `--kakao-login` remains a manual terminal fallback for developers.

For local checks without the Metal shader build tools:
`cargo check --locked -p openpickle-desktop --all-targets --features gpui/runtime_shaders`.
The package script uses the normal release build and requires the macOS build tools.
