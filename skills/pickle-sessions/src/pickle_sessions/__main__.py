from __future__ import annotations

import argparse
import sys

from . import CacheError, run


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="pickle-sessions", description="Search and read the cached Pickle conversation.")
    actions = parser.add_subparsers(dest="action", required=True)
    actions.add_parser("info", help="cache path, size, time span and age")
    find = actions.add_parser("search", help="messages containing a text, newest first")
    find.add_argument("query")
    find.add_argument("--regex", action="store_true", help="treat the query as a regular expression")
    find.add_argument("--role", help="comma list: user, assistant, agent_request, subagent")
    find.add_argument("--since", help="local date or date-time, e.g. 2026-11-01")
    find.add_argument("--until", help="local date (whole day included) or date-time")
    find.add_argument("--limit", type=int, default=20, help="hits shown (0 = all)")
    show = actions.add_parser("show", help="the messages around a seq")
    show.add_argument("seq", type=int)
    show.add_argument("--context", type=int, default=5, help="messages before and after")
    args = vars(parser.parse_args(argv))
    try:
        print(run(**args))
    except CacheError as error:
        print(f"pickle-sessions: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
