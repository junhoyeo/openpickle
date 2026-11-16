# openpickle-desktop

A native desktop app for chatting with your Pickle. It is written in Rust and draws with
[GPUI](https://gpui.rs), the GPU-rendered UI framework of the Zed editor, so there is no webview.

```sh
cargo run --release -p openpickle-desktop            # the app
./crates/openpickle-desktop/macos/package.sh         # OpenPickle.app and a disk image
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
- **Explore** (the second tab, or Cmd-2): the offers the server lists, as the app shows them.
  "For you" previews each section, with See all, and lists the rest under "More to explore"; an
  area's chip lists that area's offers by rank. An offer opens a sheet with its steps, and
  Start sends the offer's text to the chat with the offer's id. Offers that stay on screen are
  reported to the server, as the app does.
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

Not built: the Archive and Profile tabs, the menu's Archive, 3D World and My Pickle, the
microphone and voice calls (all drawn, none active); pictures and files (a message only says
how many it has); link previews; reactions; answering a request card; other rooms than the one
with your character. No capture shows the requests for deleting an offer, adding a friend,
answering a friend request or a friend's own page, so those are left out, as are the contacts
list, sharing the invite through other apps and the Korean roulette promotion. The texts of the
new screens are English readings of the app's string keys; the strings were not captured.

## Packaging

`macos/package.sh` builds `OpenPickle.app` and a disk image into `target/macos/`. It uses only
macOS's own tools (`sips`, `iconutil`, `codesign`, `hdiutil`). The app is signed ad hoc; see the
[root README](../../README.md#get-it) for what that means.

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
(`~/Library/Application Support/openpickle` on macOS), readable by you only:

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
