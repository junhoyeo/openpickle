# openpickle

An unofficial client for [Pickle](https://pickle.com), built from the app's own network traffic.

| Path | What it is |
| --- | --- |
| [`ts/`](ts/README.md) | Typed TypeScript client for the Pickle character API, a small CLI, and the script that replays Proxyman captures against the client. The API notes live in its README. |

Captures, exported messages and login session files hold bearer tokens and personal data. They
belong in `.capture/`, which git ignores.
