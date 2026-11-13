#!/usr/bin/env python3
"""Fail if a pull request adds an email address that is not allowlisted.

Only lines *added* by the PR are scanned (`git diff BASE...HEAD`), so email
addresses already in the repo never fail the check. The allowlist lives in
`.github/email-allowlist.txt`. A line containing `email-ok` is also skipped.

This file lives under `.github/`, which CODEOWNERS gates on the owner, so a PR
cannot weaken the check without owner approval.

Usage: check_new_emails.py BASE_SHA HEAD_SHA
"""
import re
import subprocess
import sys
from pathlib import Path

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}")
SKIP_FILES = ("package-lock.json", "Cargo.lock", "yarn.lock", ".github/email-allowlist.txt")
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
    """Yield (path, line number, text) for every added line in the diff."""
    diff = subprocess.run(
        ["git", "diff", "-U0", "--no-color", "--no-ext-diff", f"{base}...{head}"],
        check=True, capture_output=True, text=True, errors="replace",
    ).stdout
    path, lineno = None, 0
    for line in diff.splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("@@"):
            m = re.search(r"\+(\d+)", line)
            lineno = int(m.group(1)) if m else 0
        elif line.startswith("+") and path:
            yield path, lineno, line[1:]
            lineno += 1


def main():
    base, head = sys.argv[1], sys.argv[2]
    exact, domains = load_allowlist()
    found = []
    for path, lineno, text in added_lines(base, head):
        if path.endswith(SKIP_FILES) or "email-ok" in text:
            continue
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
