#!/usr/bin/env python3
"""Send a newsletter to all enrolled mitch.88chan.me users.
Usage:
  ./newsletter.py <subject> <body>
  ./newsletter.py <subject>         (reads body from stdin)
  ./newsletter.py -h <hours> <subject> <body>   (spread send over N hours, background)
  ./newsletter.py -h <hours> <subject>          (reads body from stdin)
  ./newsletter.py -a <email>        (add email to manual list)
  ./newsletter.py -r <email>        (remove email from manual list)
  ./newsletter.py -u <email>        (unsubscribe email)
  ./newsletter.py -l                (list all recipients)

The body may be plain text (wrapped in the branded HTML template by the send
scripts, same as every other mitch.pro email) or raw HTML (passed through
unchanged when it already contains HTML tags). Credentials come from the
environment when available — `sudo doppler run -- ./newsletter.py ...` works
directly; otherwise secrets are fetched from Doppler (or .env) as a fallback.
"""
import json, os, sqlite3, subprocess, sys, time, threading

BASE          = os.path.dirname(os.path.abspath(__file__))
LOGS_DIR      = os.path.join(BASE, 'logs', 'newsletter_logs')
DB_PATH       = os.path.join(BASE, 'data', 'mitchpro.db')
SEND_SCRIPT    = os.path.join(BASE, 'mail', 'send_email.js')
NOREPLY_SCRIPT = os.path.join(BASE, 'mail', 'noreply_send.js')

def _email_script(to):
    domain = to.split('@')[-1].lower() if '@' in to else ''
    return SEND_SCRIPT if domain == 'student.rjuhsd.us' else NOREPLY_SCRIPT

def read_json_doc(rel_path, default=None):
    if os.path.isfile(DB_PATH):
        try:
            conn = sqlite3.connect(DB_PATH, timeout=5)
            cursor = conn.cursor()
            cursor.execute("SELECT content FROM json_documents WHERE path = ?", (rel_path,))
            row = cursor.fetchone()
            conn.close()
            if row and row[0]:
                return json.loads(row[0])
        except Exception:
            pass
    full_path = os.path.join(BASE, rel_path)
    if os.path.isfile(full_path):
        try:
            with open(full_path, 'r', encoding='utf-8') as f:
                return json.load(f)
        except Exception:
            pass
    return default if default is not None else []

def write_json_doc(rel_path, data):
    content = json.dumps(data, indent=2)
    now_ms = int(time.time() * 1000)
    if os.path.isfile(DB_PATH):
        try:
            conn = sqlite3.connect(DB_PATH, timeout=5)
            cursor = conn.cursor()
            cursor.execute(
                "INSERT OR REPLACE INTO json_documents (path, content, updated_at) VALUES (?, ?, ?)",
                (rel_path, content, now_ms)
            )
            conn.commit()
            conn.close()
        except Exception as e:
            print(f"Warning: failed to write {rel_path} to DB: {e}", file=sys.stderr)
    full_path = os.path.join(BASE, rel_path)
    try:
        os.makedirs(os.path.dirname(full_path), exist_ok=True)
        with open(full_path, 'w', encoding='utf-8') as f:
            f.write(content)
    except Exception:
        pass

def normalize_email(email):
    if not email or not isinstance(email, str) or '@' not in email:
        return (email or '').strip().lower()
    local, domain = email.strip().lower().split('@', 1)
    norm_local = local.replace('.', '')
    return f"{norm_local}@{domain}"

def load_extra():
    res = read_json_doc('data/newsletter_extra.json', [])
    return res if isinstance(res, list) else []

def save_extra(lst):
    write_json_doc('data/newsletter_extra.json', sorted(set(lst)))

def load_unsub():
    res = read_json_doc('data/newsletter_unsub.json', [])
    unsub_set = set()
    if isinstance(res, list):
        for e in res:
            if isinstance(e, str) and e.strip():
                unsub_set.add(normalize_email(e))
    return unsub_set

def save_unsub(s):
    write_json_doc('data/newsletter_unsub.json', sorted(set(s)))

def get_all_emails():
    unsub_norm = load_unsub()
    chosen = {}

    def candidate(email):
        if not email or not isinstance(email, str):
            return
        e = email.strip()
        if not e or '@' not in e:
            return
        norm = normalize_email(e)
        if norm in unsub_norm:
            return

        if norm not in chosen:
            chosen[norm] = e
        else:
            curr_local = chosen[norm].split('@')[0]
            new_local = e.split('@')[0]
            if '.' not in curr_local and '.' in new_local:
                chosen[norm] = e

    for e in load_extra():
        candidate(e)

    tokens = read_json_doc('data/tokens.json', {})
    if isinstance(tokens, dict):
        for data in tokens.values():
            if isinstance(data, dict):
                candidate(data.get('email', ''))

    if os.path.isfile(DB_PATH):
        try:
            conn = sqlite3.connect(DB_PATH, timeout=5)
            cursor = conn.cursor()
            cursor.execute("SELECT email FROM users WHERE email IS NOT NULL AND email != ''")
            for row in cursor.fetchall():
                candidate(row[0])
            conn.close()
        except Exception as e:
            print(f'Warning: could not read users from DB: {e}', file=sys.stderr)

    return sorted(chosen.values(), key=lambda s: s.lower())

UNSUB_FOOTER = ''  # footer is now appended by the send scripts

def has_profanity(email):
    # Words come from the bad_words table (data/mitchpro.db); a missing DB or
    # table means nothing is filtered.
    try:
        conn = sqlite3.connect('file:%s?mode=ro' % DB_PATH, uri=True, timeout=5)
        try:
            bad = [r[0] for r in conn.execute('SELECT word FROM bad_words')]
        finally:
            conn.close()
        local = email.split('@')[0].lower()
        return any(w.lower() in local for w in bad)
    except Exception:
        return False

def _has_mail_creds():
    # Match what the send scripts themselves require: send_email.js needs
    # GMAIL_USER+GMAIL_PASS, noreply_send.js needs NOREPLY_USER+NOREPLY_PASS.
    gmail  = bool(os.environ.get('GMAIL_USER')) and bool(os.environ.get('GMAIL_PASS'))
    noreply = bool(os.environ.get('NOREPLY_USER')) and bool(os.environ.get('NOREPLY_PASS'))
    return gmail or noreply

def _strip_quotes(v):
    v = str(v).strip()
    if len(v) >= 2 and v[0] == v[-1] and v[0] in '"\'':
        v = v[1:-1]
    return v

def load_doppler_env():
    os.environ['DOPPLER_ENABLE_DNS_RESOLVER'] = 'true'
    # Credentials already injected (e.g. `sudo doppler run -- newsletter.py`):
    # trust the environment as-is, never override or re-fetch.
    if _has_mail_creds():
        return
    try:
        res = subprocess.run(['doppler', 'secrets', 'download', '--format', 'json'], capture_output=True, text=True, timeout=5)
        if res.returncode == 0 and res.stdout:
            secrets = json.loads(res.stdout)
            for k, v in secrets.items():
                if k not in os.environ:
                    os.environ[k] = _strip_quotes(v)
            if _has_mail_creds():
                return
    except Exception:
        pass
    try:
        res = subprocess.run(['sudo', 'doppler', 'secrets', 'download', '--format', 'json'], capture_output=True, text=True, timeout=10)
        if res.returncode == 0 and res.stdout:
            secrets = json.loads(res.stdout)
            for k, v in secrets.items():
                if k not in os.environ:
                    os.environ[k] = _strip_quotes(v)
            if _has_mail_creds():
                return
    except Exception:
        pass
    env_file = os.path.join(BASE, '.env')
    if os.path.isfile(env_file):
        try:
            with open(env_file, 'r') as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith('#') and '=' in line:
                        k, v = line.split('=', 1)
                        k = k.replace('export ', '').strip()
                        v = v.strip(' "\'')
                        if k not in os.environ:
                            os.environ[k] = v
        except Exception:
            pass

def get_bun_runner():
    candidates = [
        '/home/mitch/.bun/bin/bun',
        os.path.expanduser('~/.bun/bin/bun'),
        '/usr/local/bin/bun',
        '/usr/bin/bun',
        '/root/.bun/bin/bun',
    ]
    for c in candidates:
        if os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    res = subprocess.run(['which', 'bun'], capture_output=True, text=True)
    if res.returncode == 0 and res.stdout.strip():
        return res.stdout.strip()
    return 'bun'

def send(to, subject, body):
    if has_profanity(to):
        return True, 'silently dropped (profanity)'
    load_doppler_env()
    runner = get_bun_runner()
    result = subprocess.run(
        [runner, _email_script(to), to, subject, body + UNSUB_FOOTER],
        capture_output=True, text=True, timeout=20, env=os.environ
    )
    return result.returncode == 0, result.stderr.strip() or result.stdout.strip()

class _Tee:
    def __init__(self, *streams): self.streams = streams
    def write(self, data):
        for s in self.streams: s.write(data)
    def flush(self):
        for s in self.streams: s.flush()

def main():
    args = sys.argv[1:]

    if not args:
        print(__doc__.strip())
        sys.exit(1)

    if args[0] == '-a':
        if len(args) < 2:
            print('Usage: ./newsletter.py -a <email>', file=sys.stderr)
            sys.exit(1)
        email = args[1].strip().lower()
        extra = load_extra()
        if email in extra:
            print(f'{email} is already in the manual list.')
        else:
            extra.append(email)
            save_extra(extra)
            print(f'Added {email} to manual list ({len(extra)} total).')
            ok, msg = send(email, 'Welcome to the mitch.pro newsletter',
                'You\'ve been signed up for the mitch.pro newsletter.')
            print(f'  {"✓" if ok else "✗"} Welcome email {"sent" if ok else "failed: " + msg}')
        return

    if args[0] == '-r':
        if len(args) < 2:
            print('Usage: ./newsletter.py -r <email>', file=sys.stderr)
            sys.exit(1)
        email = args[1].strip().lower()
        extra = load_extra()
        if email not in extra:
            print(f'{email} not in manual list.')
        else:
            extra.remove(email)
            save_extra(extra)
            print(f'Removed {email} from manual list.')
        return

    if args[0] == '-u':
        if len(args) < 2:
            print('Usage: ./newsletter.py -u <email>', file=sys.stderr)
            sys.exit(1)
        email = args[1].strip().lower()
        unsub = load_unsub()
        if email in unsub:
            print(f'{email} is already unsubscribed.')
        else:
            unsub.add(email)
            save_unsub(unsub)
            print(f'Unsubscribed {email}.')
        return

    if args[0] == '-l':
        emails = get_all_emails()
        extra = set(load_extra())
        print(f'{len(emails)} recipient(s):')
        for e in emails:
            tag = ' [manual]' if e in extra else ''
            print(f'  {e}{tag}')
        return

    if args[0] == '-h':
        if len(args) < 3:
            print('Usage: ./newsletter.py -h <hours> <subject> [body]', file=sys.stderr)
            sys.exit(1)
        try:
            hours = float(args[1])
        except ValueError:
            print('Error: hours must be a number', file=sys.stderr)
            sys.exit(1)
        subject = args[2]
        body = args[3] if len(args) >= 4 else None
        if body is None:
            print('Reading body from stdin (Ctrl+D when done):')
            body = sys.stdin.read().strip()
            if not body:
                print('Empty body, aborting.', file=sys.stderr)
                sys.exit(1)

        emails = get_all_emails()
        if not emails:
            print('No recipients found.')
            sys.exit(0)

        delay = (hours * 3600) / len(emails)
        os.makedirs(LOGS_DIR, exist_ok=True)
        log_name = time.strftime('%Y-%m-%d_%H-%M-%S') + '_background.log'
        log_path = os.path.join(LOGS_DIR, log_name)
        print(f'Queuing {len(emails)} recipients over {hours}h ({delay:.0f}s between each). Log: {log_path}')

        pid = os.fork()
        if pid > 0:
            sys.exit(0)

        # child: detach from terminal, log only to file
        os.setsid()
        with open('/dev/null', 'rb') as dn: os.dup2(dn.fileno(), sys.stdin.fileno())
        with open('/dev/null', 'wb') as dn:
            os.dup2(dn.fileno(), sys.stdout.fileno())
            os.dup2(dn.fileno(), sys.stderr.fileno())

        log_file = open(log_path, 'w', buffering=1)
        def log(msg):
            log_file.write(f'[{time.strftime("%Y-%m-%d %H:%M:%S")}] {msg}\n')
            log_file.flush()

        log(f'Background send started: {len(emails)} recipients over {hours}h ({delay:.0f}s/email)')
        log(f'Subject: {subject}')
        for i, email in enumerate(emails):
            if i > 0:
                time.sleep(delay)
            ok, msg = send(email, subject, body)
            log(f'{"✓" if ok else "✗"} {email}' + (f': {msg}' if not ok else ''))
        log('Done.')
        log_file.close()
        sys.exit(0)

    subject = args[0]
    body = args[1] if len(args) >= 2 else None

    if body is None:
        print('Reading body from stdin (Ctrl+D when done):')
        body = sys.stdin.read().strip()
        if not body:
            print('Empty body, aborting.', file=sys.stderr)
            sys.exit(1)

    emails = get_all_emails()
    if not emails:
        print('No recipients found.')
        sys.exit(0)

    os.makedirs(LOGS_DIR, exist_ok=True)
    log_name = time.strftime('%Y-%m-%d_%H-%M-%S') + '.log'
    log_path = os.path.join(LOGS_DIR, log_name)
    log_file = open(log_path, 'w', buffering=1)
    sys.stdout = _Tee(sys.__stdout__, log_file)
    sys.stderr = _Tee(sys.__stderr__, log_file)
    print(f'Log: {log_path}')
    print(f'Sending to {len(emails)} recipient(s):')
    for e in emails:
        print(f'  {e}')
    print(f'Subject: {subject}')
    print()

    results = {}
    lock = threading.Lock()
    sem = threading.Semaphore(5)

    def send_one(email):
        with sem:
            time.sleep(0.5)
            success, msg = send(email, subject, body)
        with lock:
            results[email] = (success, msg)
            if success:
                print(f'  ✓ {email}')
            else:
                print(f'  ✗ {email}: {msg}')

    threads = [threading.Thread(target=send_one, args=(e,)) for e in emails]
    for t in threads: t.start()
    for t in threads: t.join()

    ok_count  = sum(1 for s, _ in results.values() if s)
    fail_count = sum(1 for s, _ in results.values() if not s)
    print(f'\nDone: {ok_count} sent, {fail_count} failed.')

if __name__ == '__main__':
    main()
