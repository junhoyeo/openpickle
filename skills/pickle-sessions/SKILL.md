---
name: pickle-sessions
description: Search and read the owner's Pickle conversation (the chat with their Pickle character) from the local cache the openpickle desktop app keeps on this Mac - case-insensitive or regex search with role and date filters, the messages around a hit, and how old the cache is. Read-only; works while the app runs. Use when the user asks what they or their Pickle said, to find something from the Pickle chat (피클 대화, 피클이 뭐라고 했는지), or to quote or summarise part of that conversation.
---

# Pickle Sessions

Reads `~/Library/Application Support/openpickle/cache/`, which the
[openpickle](https://github.com/junhoyeo/openpickle) desktop app writes: `room.json` (the room id),
`room-<id>.jsonl` (the append-only history log) and `identity.json` (the character's name). It only
reads; it never touches `session.json`, the network, or the app's lock, so it is safe while the app
runs. The cache is only as fresh as the app's last run: `info` says when the log was last written.

The conversation is private. Quote only what the task needs.

## Shell (any agent)

Python 3.9+ standard library, nothing to install:

```sh
S=~/.agents/skills/pickle-sessions/scripts/pickle-sessions
python3 $S info                                   # path, count, first/last message, cache age, character name
python3 $S search "dinner"                        # newest first: #seq  time  role  snippet
python3 $S search "din+er|lunch" --regex --role assistant --since 2026-11-01 --until 2026-11-07 --limit 50
python3 $S show 812 --context 3                   # the messages around #812, full text
```

A missing cache exits 1 with `pickle-sessions: No Pickle conversation cached in …`; the app has to
be run and signed in once.

## Prime Agent kernel

```python
print(await pickle_sessions("info"))
print(await pickle_sessions("search", "dinner", role="user", since="2026-11-01", limit=10))
print(await pickle_sessions("show", seq=812, context=3))
```

Structured access, for counting or filtering in Python:

```python
from pickle_sessions import load, search, around, info
hits = search("dinner", regex=False, role="assistant", since="2026-11-01", until=None)  # list[Hit(seq, time, role, snippet)]
messages = around(812, context=3)                         # list[Message(seq, at, role, text, attachments)]
conversation = load()                                     # Conversation(log, written, character, messages)
```

## What the fields mean

- `seq` is the server's order of the room; `show` takes the number `search` prints after `#`.
- Times are local. `--since`/`--until` take an ISO date (`2026-11-01`) or date-time
  (`2026-11-01T18:30`); `--until` with a bare date includes that whole day.
- Roles: `user` (the owner), `assistant` (the character), `agent_request` (a question card from the
  character; its text is the question and options), `subagent` (a background task; its text is
  the task title). `--role` takes a comma list.
- The character's text wraps each chat bubble in `<bubble>`; search matches the text without the
  tags, and `show` prints bubbles as paragraphs. Markdown and fenced `card` blocks stay as written.
- Pictures and files are not in the log; `show` says how many attachments a message had.
- A later record of a message replaces an earlier one; lines cut short by a crash are skipped.
