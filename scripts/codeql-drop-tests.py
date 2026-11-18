#!/usr/bin/env python3
"""Drops CodeQL results that are in Rust test code from a SARIF file, in place.

Test code isn't shipped, and its fixtures (made-up passwords, nonces, salts) are what CodeQL's
hard-coded-value and cleartext queries flag most. GitHub's default setup can't leave them out:
Rust tests sit in the same file as the code, inside `#[cfg(test)]` modules, so `paths-ignore`
can't reach them. The CodeQL workflow (.github/workflows/codeql.yml) runs this between the
analysis and the upload.

Test code is:
- anything inside an item marked `#[cfg(test)]` (a `mod tests { ... }`, or a test-only fn),
  found by matching its braces while skipping strings, chars and comments;
- a file in a `tests/` folder, or in `tools/testbot/` (the test players).

    scripts/codeql-drop-tests.py <file.sarif> [<source root, default .>]

Prints what it dropped. `--check <file.rs> <line>` says whether one line is test code.
"""

import json
import re
import sys
from functools import lru_cache
from pathlib import Path

TEST_PATHS = (re.compile(r"(^|/)tests/"), re.compile(r"^tools/testbot/"))
CFG_TEST = re.compile(r"^\s*#\[cfg\(\s*(test|all\(\s*test\b[^]]*)\)\s*\]")


def code_only(text):
    """`text` with strings, char literals and comments blanked out (newlines kept), so braces
    and attributes in them don't count."""
    out = list(text)
    i, n = 0, len(text)

    def blank(a, b):
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', text[i:]) and not (i and (text[i - 1].isalnum() or text[i - 1] == "_")):
            hashes = len(re.match(r"r(#*)", text[i:]).group(1))
            end = '"' + "#" * hashes
            j = text.find(end, i + 2 + hashes)
            j = n if j < 0 else j + len(end)
            blank(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i, j + 1)
            i = j + 1
        elif c == "'":
            # A char literal ('a', '\n', '\u{1F600}', '{'), else a lifetime ('a, 'static).
            m = re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'", text[i:])
            if m:
                blank(i, i + m.end())
                i += m.end()
            else:
                i += 1
        else:
            i += 1
    return "".join(out)


@lru_cache(maxsize=None)
def test_ranges(path):
    """The (first, last) line ranges of `#[cfg(test)]` items in a Rust file."""
    try:
        text = Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ()
    code = code_only(text)
    starts = [0]
    for k, ch in enumerate(code):
        if ch == "\n":
            starts.append(k + 1)
    ranges = []
    for lineno, (start, line) in enumerate(zip(starts, code.split("\n")), 1):
        if not CFG_TEST.match(line):
            continue
        # The item's body: its first `{` (before any `;`, which would make it bodiless).
        j = start
        brace = semi = -1
        while j < len(code):
            if code[j] == "{":
                brace = j
                break
            if code[j] == ";":
                semi = j
                break
            j += 1
        if brace < 0:
            end_pos = semi if semi >= 0 else start
        else:
            depth, j = 0, brace
            while j < len(code):
                if code[j] == "{":
                    depth += 1
                elif code[j] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            end_pos = j
        ranges.append((lineno, code.count("\n", 0, end_pos) + 1))
    return tuple(ranges)


def is_test(root, uri, line):
    rel = uri.removeprefix("file://").lstrip("/") if uri.startswith("file://") else uri
    if any(p.search(rel) for p in TEST_PATHS):
        return True
    if not rel.endswith(".rs") or not line:
        return False
    return any(a <= line <= b for a, b in test_ranges(str(Path(root) / rel)))


def main(argv):
    if len(argv) == 4 and argv[1] == "--check":
        print("test" if is_test(".", argv[2], int(argv[3])) else "code")
        return 0
    if len(argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    sarif_path, root = argv[1], argv[2] if len(argv) == 3 else "."
    sarif = json.loads(Path(sarif_path).read_text(encoding="utf-8"))
    dropped = kept = 0
    for run in sarif.get("runs", []):
        results = []
        for r in run.get("results", []):
            locs = r.get("locations") or []
            pl = (locs[0].get("physicalLocation") or {}) if locs else {}
            uri = (pl.get("artifactLocation") or {}).get("uri", "")
            line = (pl.get("region") or {}).get("startLine", 0)
            if uri and is_test(root, uri, line):
                dropped += 1
                print(f"dropped {r.get('ruleId')} at {uri}:{line} (test code)")
            else:
                kept += 1
                results.append(r)
        run["results"] = results
    Path(sarif_path).write_text(json.dumps(sarif), encoding="utf-8")
    print(f"{sarif_path}: {dropped} results in test code dropped, {kept} kept")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
