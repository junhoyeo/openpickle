"""Tests against a made-up cache; the real one is never read."""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import datetime
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import pickle_sessions  # noqa: E402
from pickle_sessions import CacheError, around, info, load, run, search  # noqa: E402
from pickle_sessions.__main__ import main  # noqa: E402

ROOM = "room-test"


def at(day: str) -> float:
    return datetime.fromisoformat(day).timestamp()


def item(seq: int, role: str, text: str | None, day: str = "2026-01-10T12:00", **extra) -> bytes:
    value = {"role": role, "id": f"m{seq}", "at": at(day), "seq": seq, **extra}
    if text is not None:
        value["text"] = text
    return json.dumps({"t": "item", "v": value}, ensure_ascii=False).encode()


class CacheTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.cache = Path(directory.name)
        patcher = patch.dict(os.environ, {"PICKLE_SESSIONS_CACHE": str(self.cache)})
        patcher.start()
        self.addCleanup(patcher.stop)

    def write(self, *lines: bytes, name: str | None = "Mango") -> None:
        (self.cache / "room.json").write_text(json.dumps(ROOM))
        (self.cache / f"room-{ROOM}.jsonl").write_bytes(b"\n".join(lines) + b"\n")
        identity = self.cache / "identity.json"
        if name:
            identity.write_text(json.dumps({"name": name, "avatar": ""}))
        else:
            identity.unlink(missing_ok=True)

    def sample(self) -> None:
        self.write(
            item(1, "user", "Where should we eat tonight?", "2026-01-10T18:00"),
            item(2, "assistant", "<bubble>The noodle place</bubble><bubble>or the **taco** truck</bubble>", "2026-01-10T18:01"),
            json.dumps({"t": "card", "v": {"id": "m2", "state": "done"}}).encode(),
            json.dumps({"t": "synced", "v": 2}).encode(),
            item(3, "user", "Tacos it is", "2026-01-12T09:00", attachments=["attachments/a.png"]),
            item(4, "agent_request", None, "2026-01-12T09:01", request={"kind": "choice", "question": "Which truck?", "options": [{"id": "a", "label": "Red truck"}]}),
            item(5, "subagent", None, "2026-01-12T09:02", title="Looking up taco trucks"),
            item(6, "assistant", "<bubble>Red truck opens at 6</bubble>", "2026-01-13T10:00"),
        )


class LoadTest(CacheTest):
    def test_a_later_item_replaces_an_earlier_one(self) -> None:
        self.write(item(1, "user", "first draft"), item(2, "assistant", "<bubble>hi</bubble>"), item(1, "user", "final words"))
        messages = load().messages
        self.assertEqual([m.seq for m in messages], [1, 2])
        self.assertEqual(messages[0].text, "final words")
        self.assertEqual(search("draft"), [])

    def test_torn_lines_and_bad_bytes_cost_only_themselves(self) -> None:
        self.write(item(1, "user", "before"), b"\xff\xfe{\"cut", item(2, "user", "after the bad bytes"), b'{"t":"item","v":{"role"')
        self.assertEqual([m.text for m in load().messages], ["before", "after the bad bytes"])

    def test_bubbles_become_paragraphs(self) -> None:
        self.sample()
        self.assertEqual(around(2, 0)[0].text, "The noodle place\n\nor the **taco** truck")

    def test_requests_and_subagents_read_as_their_question_and_title(self) -> None:
        self.sample()
        texts = {m.seq: m.text for m in load().messages}
        self.assertEqual(texts[4], "Which truck?\n- Red truck")
        self.assertEqual(texts[5], "Looking up taco trucks")

    def test_missing_cache_says_to_run_the_app(self) -> None:
        with self.assertRaisesRegex(CacheError, "Run the openpickle desktop app"):
            load()

    def test_missing_log_is_the_same_error(self) -> None:
        (self.cache / "room.json").write_text(json.dumps(ROOM))
        with self.assertRaises(CacheError):
            info()


class SearchTest(CacheTest):
    def setUp(self) -> None:
        super().setUp()
        self.sample()

    def test_case_insensitive_newest_first_without_bubble_tags(self) -> None:
        hits = search("TACO")
        self.assertEqual([hit.seq for hit in hits], [5, 3, 2])
        self.assertNotIn("bubble", hits[-1].snippet)
        self.assertIn("The noodle place or the **taco** truck", hits[-1].snippet)

    def test_substring_is_literal_unless_regex(self) -> None:
        self.assertEqual(search("opens at ."), [])
        self.assertEqual([hit.seq for hit in search(r"opens at \d", regex=True)], [6])

    def test_role_filter(self) -> None:
        self.assertEqual([hit.seq for hit in search("truck", role="assistant")], [6, 2])
        self.assertEqual([hit.seq for hit in search("truck", role="user,agent_request")], [4])

    def test_date_range_includes_the_whole_until_day(self) -> None:
        self.assertEqual([hit.seq for hit in search("t", since="2026-01-12", until="2026-01-12")], [5, 4, 3])
        self.assertEqual([hit.seq for hit in search("t", until="2026-01-10")], [2, 1])
        self.assertEqual([hit.seq for hit in search("t", since="2026-01-12T09:01")], [6, 5, 4])

    def test_long_text_gets_a_window(self) -> None:
        self.write(item(1, "user", "x" * 200 + " needle " + "y" * 200))
        snippet = search("needle")[0].snippet
        self.assertTrue(snippet.startswith("…") and snippet.endswith("…"))
        self.assertLess(len(snippet), 140)

    def test_rendered_search_respects_limit(self) -> None:
        out = run("search", "truck", limit=1)
        self.assertTrue(out.startswith("#6  "))
        self.assertTrue(out.endswith("4 matches, showing the newest 1"))


class ShowAndInfoTest(CacheTest):
    def setUp(self) -> None:
        super().setUp()
        self.sample()

    def test_show_marks_the_seq_and_keeps_context(self) -> None:
        out = run("show", seq=3, context=1)
        self.assertIn("> #3", out)
        self.assertIn("  #2", out)
        self.assertIn("  #4", out)
        self.assertNotIn("#1 ", out)
        self.assertIn("[1 attachment]", out)
        self.assertIn("    or the **taco** truck", out)

    def test_show_unknown_seq(self) -> None:
        with self.assertRaisesRegex(CacheError, "seqs run 1-6"):
            run("show", seq=99)

    def test_info(self) -> None:
        data = info()
        self.assertEqual(data.messages, 6)
        self.assertEqual(data.character, "Mango")
        self.assertEqual(data.roles, {"user": 2, "assistant": 2, "agent_request": 1, "subagent": 1})
        self.assertEqual(data.first, "2026-01-10 18:00")
        self.assertEqual(data.last, "2026-01-13 10:00")
        self.assertIn("ago", run("info"))

    def test_info_without_identity(self) -> None:
        self.write(item(1, "user", "hi"), name=None)
        self.assertIsNone(info().character)


class CommandTest(CacheTest):
    def call(self, *argv: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def test_search_and_show(self) -> None:
        self.sample()
        code, out, _ = self.call("search", "truck", "--role", "assistant", "--since", "2026-01-11")
        self.assertEqual((code, out.splitlines()[0][:3]), (0, "#6 "))
        code, out, _ = self.call("show", "6", "--context", "0")
        self.assertEqual(code, 0)
        self.assertIn("Red truck opens at 6", out)

    def test_a_request_or_task_with_empty_text_is_read_from_its_own_fields(self) -> None:
        self.write(
            item(1, "agent_request", "", request={"question": "Which truck?", "options": [{"label": "Red truck"}]}),
            item(2, "subagent", "", title="Looking up taco trucks"),
        )
        self.assertEqual([hit.seq for hit in search("truck")], [2, 1])
        self.assertEqual(around(1, 0)[0].text, "Which truck?\n- Red truck")
        self.assertEqual(around(2, 0)[0].text, "Looking up taco trucks")

    def test_your_own_text_is_kept_as_written(self) -> None:
        self.write(item(1, "user", "Please keep <bubble>hello</bubble> literally."))
        self.assertEqual([hit.seq for hit in search("<bubble>")], [1])
        self.assertEqual(around(1, 0)[0].text, "Please keep <bubble>hello</bubble> literally.")

    def test_missing_cache_exits_1_with_a_message(self) -> None:
        code, out, err = self.call("info")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("Run the openpickle desktop app", err)


if __name__ == "__main__":
    unittest.main()
