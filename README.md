# openpickle

An unofficial client for [Pickle](https://pickle.com), built from the app's own network traffic.

| Path | What it is |
| --- | --- |
| [`crates/openpickle-desktop`](crates/openpickle-desktop/README.md) | A native desktop app for chatting with your Pickle, written in Rust with GPUI. |
| `crates/openpickle-api` | The Rust client for the Pickle character API that the desktop app uses. |
| [`ts/`](ts/README.md) | The TypeScript client the Rust one was ported from, a small CLI, and the notes on the API: endpoints, sign-in, login sessions, events. |

Both clients are checked against Proxyman captures of the iOS app: every recorded request is
replayed through the client and compared with what the app sent.

```sh
cargo run --release -p openpickle-desktop                               # the app
cargo run -p openpickle-api --example verify_capture -- <capture.proxymanlogv2>
cd ts && bun run verify:capture <capture.proxymanlogv2>
```

Captures, exported messages and login session files hold bearer tokens and personal data. They
belong in `.capture/`, which git ignores.
