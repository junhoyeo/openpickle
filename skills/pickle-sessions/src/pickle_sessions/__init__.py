"""Search and read the local copy of the Pickle conversation kept by the openpickle desktop app.

Entrypoint: ``run`` (exposed as ``await pickle_sessions(...)`` in the Prime Agent kernel).
Structured helpers: ``load``, ``search``, ``around``, ``info``.
"""

from __future__ import annotations

import json
import os
import re
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path

__all__ = ["run", "load", "search", "around", "info", "Message", "Hit", "Info", "Conversation", "CacheError"]

BUBBLE = re.compile(r"</?bubble>")
SNIPPET = 60


class CacheError(RuntimeError):
    pass


@dataclass
class Message:
    seq: int
    at: float
    role: str
    text: str
    attachments: int = 0

    @property
    def time(self) -> str:
        return datetime.fromtimestamp(self.at).strftime("%Y-%m-%d %H:%M")


@dataclass
class Conversation:
    log: Path
    written: float
    character: str | None
    messages: list[Message] = field(default_factory=list)


@dataclass
class Hit:
    seq: int
    time: str
    role: str
    snippet: str


@dataclass
class Info:
    log: str
    character: str | None
    messages: int
    roles: dict[str, int]
    first: str | None
    last: str | None
    written: str
    age: str


def cache_dir() -> Path:
    """Find the app's data directory unless an explicit cache path was supplied."""
    if override := os.environ.get("PICKLE_SESSIONS_CACHE"):
        return Path(override)
    if sys.platform == "darwin":
        data = Path.home() / "Library" / "Application Support"
    elif sys.platform == "win32":
        data = Path(os.environ.get("APPDATA", ""))
        if not data.is_absolute():
            raise CacheError(
                "Cannot locate the OpenPickle cache: APPDATA must be an absolute path on Windows. "
                "Set PICKLE_SESSIONS_CACHE to the app's cache directory."
            )
    else:
        xdg = Path(os.environ.get("XDG_DATA_HOME", ""))
        data = xdg if xdg.is_absolute() else Path.home() / ".local" / "share"
    return data / "openpickle" / "cache"


def _optional_text(value: object) -> str:
    if value is None:
        return ""
    if not isinstance(value, str):
        raise ValueError("expected text")
    return value


def _body(item: dict, role: str) -> str:
    text = _optional_text(item.get("text"))
    if role == "user":
        return text
    if role == "agent_request" and (request := item.get("request")) is not None:
        if not isinstance(request, dict):
            raise ValueError("invalid request")
        question = _optional_text(request.get("question"))
        lines = [question] if question else []
        options = request.get("options")
        if options is not None:
            if not isinstance(options, list):
                raise ValueError("invalid request options")
            for option in options:
                if not isinstance(option, dict) or not isinstance(option.get("label"), str):
                    raise ValueError("invalid request option")
                lines.append(f"- {option['label']}")
        if lines:
            return "\n".join(lines)
    if role == "subagent" and (title := _optional_text(item.get("title"))):
        return title
    parts = [part.strip() for part in BUBBLE.split(text)]
    return "\n\n".join(part for part in parts if part)


def _message(item: dict) -> tuple[str, Message]:
    item_id = item.get("id")
    seq = item.get("seq")
    role = item.get("role")
    seconds = item.get("at")
    if not isinstance(item_id, str) or not isinstance(role, str):
        raise ValueError("invalid message id or role")
    if type(seq) is not int or not -(1 << 63) <= seq < (1 << 63):
        raise ValueError("invalid message sequence")
    if type(seconds) not in (int, float):
        raise ValueError("invalid message timestamp")
    at = float(seconds)
    datetime.fromtimestamp(at)  # Reject timestamps the local platform cannot render.
    attachments = item.get("attachments")
    if attachments is not None and (
        not isinstance(attachments, list) or not all(isinstance(path, str) for path in attachments)
    ):
        raise ValueError("invalid attachments")
    text = _body(item, role)
    # Python's JSON decoder accepts lone surrogate escapes, which UTF-8 output cannot render.
    item_id.encode()
    role.encode()
    text.encode()
    return item_id, Message(
        seq=seq, at=at, role=role, text=text, attachments=len(attachments) if attachments else 0,
    )


def _character(cache: Path) -> str | None:
    try:
        identity = json.loads((cache / "identity.json").read_bytes())
        name = identity.get("name") if isinstance(identity, dict) else None
        if isinstance(name, str) and name:
            name.encode()
            return name
    except (OSError, ValueError, RecursionError):
        pass
    return None


def load() -> Conversation:
    """Read the primary room log, retaining the last valid record for each message id."""
    cache = cache_dir()
    items: dict[str, Message] = {}
    try:
        room = json.loads((cache / "room.json").read_bytes())
        if not isinstance(room, str) or not room or any(c in room for c in "/\\:\0"):
            raise ValueError("invalid room id")
        log = cache / f"room-{room}.jsonl"
        with log.open("rb") as stream:
            written = os.fstat(stream.fileno()).st_mtime
            for line in stream:
                try:
                    record = json.loads(line.decode("utf-8"))
                    if not isinstance(record, dict) or record.get("t") != "item":
                        continue
                    item = record.get("v")
                    if not isinstance(item, dict):
                        continue
                    item_id, message = _message(item)
                except (ValueError, OverflowError, OSError, RecursionError):
                    continue
                items[item_id] = message
    except (OSError, ValueError, RecursionError) as error:
        raise CacheError(
            f"No Pickle conversation cached in {cache} ({error.__class__.__name__}). "
            "Run the openpickle desktop app and sign in once; it writes the conversation there."
        ) from None
    messages = sorted(items.values(), key=lambda message: message.seq)
    return Conversation(log=log, written=written, character=_character(cache), messages=messages)


def _bound(value: str | None, end: bool) -> float | None:
    if not value:
        return None
    moment = datetime.fromisoformat(value)
    if end and len(value) == 10:
        moment += timedelta(days=1)
    return moment.timestamp()


def search(
    query: str,
    *,
    regex: bool = False,
    role: str | None = None,
    since: str | None = None,
    until: str | None = None,
) -> list[Hit]:
    """Every matching message, newest first. Case-insensitive; ``since``/``until`` are local ISO dates or times, ``until`` inclusive of its day."""
    pattern = re.compile(query if regex else re.escape(query), re.IGNORECASE)
    roles = {name.strip() for name in role.split(",")} if role else None
    start, end = _bound(since, False), _bound(until, True)
    hits = []
    for message in reversed(load().messages):
        if roles and message.role not in roles:
            continue
        if (start is not None and message.at < start) or (end is not None and message.at >= end):
            continue
        flat = " ".join(message.text.split())
        found = pattern.search(flat)
        if found:
            left, right = max(0, found.start() - SNIPPET), found.end() + SNIPPET
            snippet = ("…" if left else "") + flat[left:right] + ("…" if right < len(flat) else "")
            hits.append(Hit(message.seq, message.time, message.role, snippet))
    return hits


def around(seq: int, context: int = 5) -> list[Message]:
    """The message with this seq and ``context`` messages before and after it."""
    messages = load().messages
    index = next((i for i, message in enumerate(messages) if message.seq == seq), None)
    if index is None:
        raise CacheError(f"No message with seq {seq} (seqs run {messages[0].seq}-{messages[-1].seq})" if messages else "The conversation is empty")
    return messages[max(0, index - context) : index + context + 1]


def _age(seconds: float) -> str:
    minutes = int(seconds // 60)
    if minutes < 60:
        return f"{minutes}m ago"
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h {minutes}m ago" if hours < 48 else f"{hours // 24}d ago"


def info() -> Info:
    conversation = load()
    messages = conversation.messages
    roles: dict[str, int] = {}
    for message in messages:
        roles[message.role] = roles.get(message.role, 0) + 1
    return Info(
        log=str(conversation.log),
        character=conversation.character,
        messages=len(messages),
        roles=roles,
        first=messages[0].time if messages else None,
        last=messages[-1].time if messages else None,
        written=datetime.fromtimestamp(conversation.written).strftime("%Y-%m-%d %H:%M"),
        age=_age(time.time() - conversation.written),
    )


def _render_info(data: Info) -> str:
    roles = ", ".join(f"{role} {count}" for role, count in sorted(data.roles.items(), key=lambda pair: -pair[1]))
    return "\n".join(
        [
            f"log        {data.log}",
            f"character  {data.character or '(unknown)'}",
            f"messages   {data.messages} ({roles})",
            f"span       {data.first} .. {data.last}",
            f"written    {data.written} ({data.age}; the cache is only as fresh as the app's last run)",
        ]
    )


def _render_message(message: Message, mark: bool) -> str:
    extra = f"  [{message.attachments} attachment{'s' if message.attachments != 1 else ''}]" if message.attachments else ""
    head = f"{'>' if mark else ' '} #{message.seq}  {message.time}  {message.role}{extra}"
    return head + "\n" + "\n".join("    " + line if line else "" for line in message.text.split("\n"))


def run(
    action: str = "info",
    query: str | None = None,
    seq: int | None = None,
    regex: bool = False,
    role: str | None = None,
    since: str | None = None,
    until: str | None = None,
    limit: int = 20,
    context: int = 5,
) -> str:
    """Search and read the Pickle conversation cached by the openpickle desktop app.

    Actions
      info    - cache path, message count, first/last message, how old the cache is, the character's name.
      search  - messages containing ``query`` (case-insensitive substring; ``regex=True`` for a regex),
                newest first, as ``#seq  time  role  snippet``. Filters: ``role`` (comma list:
                user, assistant, agent_request, subagent), ``since``/``until`` (local ISO date or
                date-time; ``until`` includes its whole day). ``limit`` caps the hits shown.
      show    - the message with ``seq`` and ``context`` messages before and after it, full text.
    """
    verb = action.strip().lower()
    if verb == "info":
        return _render_info(info())
    if verb == "search":
        if not query:
            raise CacheError("search needs a query")
        hits = search(query, regex=regex, role=role, since=since, until=until)
        shown = hits[:limit] if limit > 0 else hits
        lines = [f"#{hit.seq}  {hit.time}  {hit.role:<9}  {hit.snippet}" for hit in shown]
        lines.append(f"{len(hits)} match{'es' if len(hits) != 1 else ''}" + (f", showing the newest {len(shown)}" if len(shown) < len(hits) else ""))
        return "\n".join(lines)
    if verb == "show":
        if seq is None:
            raise CacheError("show needs a seq (the #number search prints)")
        return "\n\n".join(_render_message(message, message.seq == seq) for message in around(seq, context))
    raise CacheError(f"Unknown action {action!r}: use info, search or show")
