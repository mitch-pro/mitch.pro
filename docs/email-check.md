# Email check (PR scan for hardcoded emails)

`.github/workflows/email-check.yml` runs `.github/scripts/check_new_emails.py`
on every PR to `master`. It scans only the lines the PR **adds** and fails if
one contains an email address that is not allowlisted, so addresses already in
the repo never fail it.

## Allowlist

`.github/email-allowlist.txt`. `@domain` entries allow that domain and its
subdomains (`@example.com`, `@mitch.pro`, `@x.test`, ...). Other entries are
exact addresses. There is no inline ignore marker, since anyone could use it to
bypass the check; add the address to the allowlist (owner-approved) instead.

## Test addresses for new code

Use these in tests, fixtures and docs instead of inventing addresses:

| Domain | Addresses |
| --- | --- |
| `student.rjuhsd.us` | `test.student1@`, `test.student2@`, `test.student3@`, `test.teacher1@` |
| `example.com` | `test1@`, `test2@`, `test3@` (any local part works) |
| `mitch.pro` | `test1@`, `test2@`, `admin@` (any local part works) |

Never use a real person's address. Other fake student-domain addresses fail the
check on purpose: that domain holds real student accounts, so it is allowlisted
only by exact address.

## Security

The script, allowlist and workflow are under `.github/`, which CODEOWNERS gates
on the owner, so a PR cannot loosen the check without owner approval. Make
`email-check` a required status check on `master` for it to block merges.

## Known limits

It is a regex scan of a PR's net diff, so it catches accidents, not deliberate
evasion: split strings (`"a" + "@b.com"`), `[at]`/`&#64;`/base64 encodings,
addresses inside binary files, and addresses added then removed within one PR
(they stay in the PR branch's history until squash-merge) all pass. It also
does not scan commit messages, PR text or branch names. GitHub secret scanning
with custom patterns and push protection covers some of that.

## Tests

`.github/scripts/test_check_new_emails.py` runs in the workflow before the scan
and has a regression test for each bypass that was found: `++ ` lines hiding
later lines, look-alike lock-file names, `.gitattributes` `-diff`, and the
removed `email-ok` marker.
