#!/usr/bin/env python3
"""Fail if a pull request adds an email address that is not allowlisted.

Only lines *added* by the PR are scanned (`git diff BASE...HEAD`), so email
addresses already in the repo never fail the check. The allowlist lives in
`.github/email-allowlist.txt`; there is deliberately no inline "ignore" marker,
because anyone could add one to defeat the check.

This file lives under `.github/`, which CODEOWNERS gates on the owner, so a PR
cannot weaken the check without owner approval.

Usage: check_new_emails.py BASE_SHA HEAD_SHA
"""
import re
import subprocess
import sys
from pathlib import Path

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}")
# Exact repo-relative paths only. Matching by suffix or basename would let a PR
# hide an address in e.g. `evil-package-lock.json` or `x/Cargo.lock`.
SKIP_PATHS = frozenset({"rust/Cargo.lock", ".github/email-allowlist.txt"})
ALLOWLIST = Path(__file__).resolve().parent.parent / "email-allowlist.txt"


def load_allowlist():
    """Return (exact addresses, domain suffixes). `@domain` entries match the
    domain and its subdomains; anything else is an exact address."""
    exact, domains = set(), []
    for raw in ALLOWLIST.read_text().splitlines():
        entry = raw.split("#", 1)[0].strip().lower()
        if not entry:
            continue
        if entry.startswith("@"):
            domains.append(entry[1:])
        else:
            exact.add(entry)
    return exact, domains


def allowed(addr, exact, domains):
    addr = addr.lower()
    if addr in exact:
        return True
    domain = addr.rsplit("@", 1)[1]
    return any(domain == d or domain.endswith("." + d) for d in domains)


def added_lines(base, head):
    """Yield (path, line number, text) for every added line in the diff.

    The diff is parsed as a state machine, not by line prefix: a file's header
    ends at its first `@@`, and after that *every* line starting with `+` is
    added content, even one whose text begins with `++ ` (which would otherwise
    look like a `+++ ` file header). `--text` makes git show lines for files
    marked `-diff`/binary in .gitattributes instead of "Binary files differ".
    """
    diff = subprocess.run(
        ["git", "-c", "core.quotePath=false", "diff", "-U0", "--text", "--no-color",
         "--no-ext-diff", "--no-renames", f"{base}...{head}"],
        check=True, capture_output=True, text=True, errors="replace",
    ).stdout
    path, lineno, in_hunk = None, 0, False
    for line in diff.splitlines():
        if line.startswith("diff --git "):
            path, in_hunk = None, False
        elif not in_hunk and line.startswith("+++ "):
            name = line[4:].split("\t")[0].strip('"')
            path = name[2:] if name.startswith("b/") else None
        elif line.startswith("@@"):
            in_hunk = True
            m = re.search(r"\+(\d+)", line)
            lineno = int(m.group(1)) if m else 0
        elif in_hunk and line.startswith("+"):
            yield path, lineno, line[1:]
            lineno += 1


def main():
    base, head = sys.argv[1], sys.argv[2]
    exact, domains = load_allowlist()
    found = []
    for path, lineno, text in added_lines(base, head):
        if path in SKIP_PATHS:
            continue
        path = path or "<unknown file>"  # fail closed: still scan unparsed paths
        for addr in EMAIL.findall(text):
            if not allowed(addr, exact, domains):
                found.append((path, lineno, addr))
    for path, lineno, addr in found:
        print(f"::error file={path},line={lineno}::New email address {addr!r} added. "
              "Remove it, or ask the owner to allowlist it.")
    if found:
        print(f"\n{len(found)} non-allowlisted email address(es) added.")
        return 1
    print("No new non-allowlisted email addresses.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
