#!/usr/bin/env python3
"""Static PR screener used by the `PR Security Review` workflow.

Diffs the PR against its merge base, sorts the changed files into risk
categories, and scans only the *added* lines for risky patterns. The result is a
markdown report written to the path given as argv[1] (default `pr_report.md`).

The report never echoes the matched line itself, only `file:line` and the rule
name, so a leaked secret is not copied into a public PR comment.

Environment:
  BASE_SHA, HEAD_SHA  commits to compare (merge-base diff, so unrelated changes
                      that landed on the base branch are not attributed to the PR)
  PR_AUTHOR           login shown in the report

The screener is advisory: it always exits 0. See docs/ci-pr-screening.md.
"""

import os
import re
import subprocess
import sys
from collections import Counter, defaultdict

MAX_PER_RULE = 5

# Path categories, first match wins. (name, label, flags the PR for review)
STATIC_PATTERNS = [
    r"^webserver/[^/]+\.css$",
    r"^webserver/rjuhsd-assets/",
    r"^docs/",
    r".*\.md$",
    r"^LICENSE$",
    r".*\.txt$",
]
CATEGORIES = [
    ("privileges", "Admin/moderator lists", True,
     [r"^data/(admins|moderators)\.json$"]),
    ("workflows", "CI/CD workflows", True, [r"^\.github/"]),
    ("deploy", "Deploy tooling", True,
     [r"^tools/deploy", r"(^|/)Dockerfile", r"^deploy/"]),
    ("instructions", "Agent/contributor instructions", True,
     [r"(^|/)AGENTS\.md$", r"(^|/)CLAUDE\.md$"]),
    ("economy", "Economy (coins, shop, casino)", True,
     [r"/(coins|shop)\.rs$", r"routes/casino\.rs$", r"me/cosmetics\.rs$",
      r"shop_catalog\.json$"]),
    ("auth", "Auth and access control", True,
     []),  # matched by AUTH_PATH below instead of a pattern list
    ("deps", "Dependencies", False,
     [r"(^|/)Cargo\.(toml|lock)$", r"(^|/)package(-lock)?\.json$"]),
]
# Rust/JS paths whose name suggests auth or access control.
AUTH_PATH = re.compile(r"(^|/)[^/]*(auth|csrf|session|admin|permission|middleware)[^/]*\.(rs|js)$"
                       r"|/(auth|admin)/", re.I)
CODE_EXT = re.compile(r"\.(rs|js|mjs|ts|html|sh|py|sql|toml|ya?ml)$")

# (rule id, label, severity, regex, applies to files matching)
# severity: "secret" is shown as critical, "review" as a flag.
RULES = [
    ("secret-key", "Private key block", "secret",
     re.compile(r"BEGIN (RSA |EC |OPENSSH |DSA )?PRIVATE KEY"), None),
    ("secret-token", "Token-shaped string", "secret",
     re.compile(r"(ghp_|gho_|ghs_|github_pat_)[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}"
                r"|xox[abp]-[A-Za-z0-9-]{10,}|sk-[A-Za-z0-9]{32,}"), None),
    ("secret-assign", "Hardcoded credential assignment", "secret",
     re.compile(r"(api[_-]?key|secret[_-]?key|secret|token|passw(or)?d)\s*[:=]\s*[\"'][A-Za-z0-9/+_\-]{16,}[\"']", re.I),
     None),
    ("cf-token", "Cloudflare token assignment", "secret",
     re.compile(r"CLOUDFLARE_API_TOKEN\s*=\s*[A-Za-z0-9_\-]{8,}"), None),
    ("privilege", "Owner/privilege check touched", "review",
     re.compile(r"SITE_CO_OWNER|\bis_owner\b|OWNER_TEST_ACCOUNT_EMAIL|is_vm_owner_locked|GOD_MODE"),
     CODE_EXT),
    ("exec", "Dynamic execution or shell-out", "review",
     re.compile(r"\beval\(|\bexec\(|child_process|std::process::Command|Command::new\("), CODE_EXT),
    ("pipe-to-sh", "Download piped to a shell", "review",
     re.compile(r"(curl|wget)\b[^|\n]*\|\s*(sudo\s+)?(ba|z)?sh\b"), None),
    ("chmod-x", "Makes a file executable", "review", re.compile(r"chmod \+x"), CODE_EXT),
    ("unsafe", "Rust unsafe block", "review", re.compile(r"\bunsafe\s*(\{|fn|impl)"), re.compile(r"\.rs$")),
    ("sql-format", "SQL built with format!", "review",
     re.compile(r"format!\(\s*\"[^\"]*\b(SELECT|INSERT|UPDATE|DELETE)\b", re.I), re.compile(r"\.rs$")),
    ("xss", "Raw HTML injection", "review",
     re.compile(r"\.innerHTML\s*=|insertAdjacentHTML|document\.write\(|dangerouslySetInnerHTML"),
     re.compile(r"\.(js|mjs|ts|html)$")),
    ("pr-target", "pull_request_target trigger", "review",
     re.compile(r"pull_request_target"), re.compile(r"^\.github/")),
    ("workflow-secrets", "Workflow reads secrets", "review",
     re.compile(r"secrets\.[A-Za-z_]+"), re.compile(r"^\.github/")),
    ("jumpscare", "Jumpscare/broadcast reference", "review",
     re.compile(r"jumpscare", re.I), None),
]


def git(*args):
    return subprocess.run(["git", "-c", "core.quotepath=off", *args],
                          check=True, capture_output=True, text=True,
                          errors="replace").stdout


def categorize(path):
    """Return (category key, label, flagged) for a changed path."""
    for key, label, flagged, patterns in CATEGORIES:
        if key == "auth":
            if AUTH_PATH.search(path):
                return key, label, flagged
            continue
        if any(re.search(p, path) for p in patterns):
            return key, label, flagged
    if any(re.search(p, path) for p in STATIC_PATTERNS):
        return "static", "Static content", False
    return "code", "Application code", False


def added_lines(base, head):
    """Yield (path, line number, text) for every line the PR adds."""
    path, lineno = None, 0
    for raw in git("diff", "-U0", "--no-color", f"{base}...{head}").splitlines():
        if raw.startswith("+++ "):
            path = None if raw.endswith("/dev/null") else raw[6:]
        elif raw.startswith("@@"):
            m = re.search(r"\+(\d+)", raw)
            lineno = int(m.group(1)) if m else 0
        elif raw.startswith("+") and path:
            yield path, lineno, raw[1:]
            lineno += 1


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else "pr_report.md"
    base, head = os.environ["BASE_SHA"], os.environ["HEAD_SHA"]
    author = os.environ.get("PR_AUTHOR", "unknown")

    # --- files ---------------------------------------------------------
    files = {}  # path -> (status, category key, label, flagged)
    for line in git("diff", "--name-status", "--no-renames", f"{base}...{head}").splitlines():
        status, _, path = line.partition("\t")
        if path:
            files[path] = (status[0], *categorize(path))
    numstat = [l.split("\t") for l in git("diff", "--numstat", "--no-renames", f"{base}...{head}").splitlines()]
    added = sum(int(a) for a, _, _ in numstat if a.isdigit())
    removed = sum(int(d) for _, d, _ in numstat if d.isdigit())

    by_cat = defaultdict(list)
    for path, (status, key, label, flagged) in sorted(files.items()):
        by_cat[key].append(path)
    flagged_cats = [(k, l) for k, l, f, _ in CATEGORIES if f and by_cat.get(k)]

    # --- patterns ------------------------------------------------------
    hits = defaultdict(list)  # rule id -> [(path, line)]
    for path, lineno, text in added_lines(base, head):
        for rid, _, _, rx, scope in RULES:
            if scope is not None:
                if isinstance(scope, re.Pattern):
                    if not scope.search(path):
                        continue
            if rx.search(text):
                hits[rid].append((path, lineno))

    # New JSON blob files go against the storage policy in AGENTS.md.
    new_json = [p for p, (s, *_ ) in files.items()
                if s == "A" and re.match(r"^data/.*\.json$", p)]

    secrets = [(r, hits[r[0]]) for r in RULES if r[2] == "secret" and hits.get(r[0])]
    reviews = [(r, hits[r[0]]) for r in RULES if r[2] == "review" and hits.get(r[0])]

    # --- report --------------------------------------------------------
    counts = Counter(k for _, (_, k, *_r) in files.items())
    only_static = bool(files) and set(counts) <= {"static"}
    need_review = bool(secrets or reviews or flagged_cats or new_json)

    md = ["### 🛡️ Automated PR Security Screener Report", ""]
    md += ["| | |", "| :--- | :--- |",
           f"| **Author** | `{author}` |",
           f"| **Changed** | {len(files)} files (+{added} / −{removed}) |"]
    names = {k: l for k, l, *_ in CATEGORIES} | {"code": "Application code", "static": "Static content"}
    for key in [k for k, *_ in CATEGORIES] + ["code", "static"]:
        if counts.get(key):
            md.append(f"| **{names[key]}** | {counts[key]} |")
    md.append("")

    if secrets:
        md += ["#### 🚨 Possible secret in the diff", ""]
    elif need_review:
        md += ["#### ⚠️ Flagged for careful review", ""]
    elif only_static:
        md += ["#### ✅ Static changes only",
               "Only styles, documentation, or public assets changed. No backend code, "
               "execution paths, or privilege sets were touched."]
    elif not files:
        md += ["#### ℹ️ No changes", "The diff against the base branch is empty."]
    else:
        md += ["#### ℹ️ Code review required",
               "This PR changes application code and needs the owner's review before deploy."]

    findings = []
    for r, locs in secrets + reviews:
        shown = ", ".join(f"`{p}:{n}`" for p, n in locs[:MAX_PER_RULE])
        more = f" (+{len(locs) - MAX_PER_RULE} more)" if len(locs) > MAX_PER_RULE else ""
        findings.append(f"- **{r[1]}**: {shown}{more}")
    for key, label in flagged_cats:
        paths = ", ".join(f"`{p}`" for p in by_cat[key][:MAX_PER_RULE])
        findings.append(f"- **{label} modified**: {paths}")
    if new_json:
        findings.append("- **New JSON data file** (new storage must be a SQL table): "
                        + ", ".join(f"`{p}`" for p in new_json))
    if by_cat.get("deps"):
        findings.append("- **Dependencies changed**: " + ", ".join(f"`{p}`" for p in by_cat["deps"][:MAX_PER_RULE]))
    if findings:
        md += findings + [""]
    if need_review:
        md += ["Required: the owner reviews this diff before it is approved.", ""]

    if files:
        md += ["<details><summary>Files by category</summary>", ""]
        for key in [k for k, *_ in CATEGORIES] + ["code", "static"]:
            if by_cat.get(key):
                md.append(f"**{names[key]}**")
                md += [f"- `{p}`" for p in by_cat[key][:40]]
                if len(by_cat[key]) > 40:
                    md.append(f"- …and {len(by_cat[key]) - 40} more")
                md.append("")
        md.append("</details>")

    with open(out_path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(md).rstrip() + "\n")


if __name__ == "__main__":
    main()
