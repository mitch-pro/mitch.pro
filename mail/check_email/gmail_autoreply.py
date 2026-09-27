#!/usr/bin/env python3
# Check emails.json for SUPPORT emails and send auto-replies via send_email.js.
# Deduplicates by messageId (falls back to sender|subject for emails without one).
# Uses a lock file to prevent concurrent runs from the loop and watcher.

import json, os, re, subprocess, sys, time, urllib.request

BASE        = os.path.dirname(os.path.abspath(__file__))
EMAILS_FILE = os.path.join(BASE, 'emails.json')
SENT_FILE   = os.path.join(BASE, 'gmail_autoreply_sent.json')
LOCK_FILE   = os.path.join(BASE, 'gmail_autoreply.lock')
SEND_SCRIPT = os.path.join(os.path.dirname(BASE), 'send_email.js')
# Try loading .env file if it exists
def load_env():
    # project root is two levels up from check_email
    env_path = os.path.join(os.path.dirname(os.path.dirname(BASE)), '.env')
    if os.path.exists(env_path):
        try:
            with open(env_path, encoding='utf-8') as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith('#') and '=' in line:
                        k, v = line.split('=', 1)
                        # strip quotes if present
                        v = v.strip().strip("'").strip('"')
                        os.environ[k.strip()] = v
        except Exception:
            pass

load_env()
NTFY_TOPIC = os.environ.get('NTFY_TOPIC', '').strip()
NTFY_USER  = os.environ.get('NTFY_USER', '').strip()
NTFY_PASS  = os.environ.get('NTFY_PASS', '').strip()
GMAIL_ADDR  = 'mitchell.fogler@student.rjuhsd.us'

def acquire_lock():
    # Spin for up to 10s waiting for any concurrent run to finish
    for _ in range(20):
        try:
            fd = os.open(LOCK_FILE, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            os.close(fd)
            return True
        except FileExistsError:
            time.sleep(0.5)
    return False

def release_lock():
    try: os.remove(LOCK_FILE)
    except Exception: pass

def get_sent():
    try:
        with open(SENT_FILE, encoding='utf-8') as f:
            return set(json.load(f))
    except Exception:
        return set()

def save_sent(sent):
    with open(SENT_FILE, 'w', encoding='utf-8') as f:
        json.dump(list(sent), f)

def make_key(email):
    # Prefer messageId (unique) over sender|subject
    mid = email.get('messageId', '').strip()
    return mid if mid else f"{email.get('sender','')}|{email.get('subject','')}"

def first_part_only(body):
    """Strip quoted lines (starting with >) and On...wrote: blocks."""
    lines = (body or '').splitlines()
    result = []
    for line in lines:
        s = line.strip()
        if s.startswith('>') or re.match(r'On .{10,} wrote:', s):
            break
        result.append(line)
    return '\n'.join(result)

def ntfy(msg, title='Support Email', priority='default'):
    if not NTFY_TOPIC:
        return
    url = NTFY_TOPIC if NTFY_TOPIC.startswith(('http://', 'https://')) else f'https://ntfy.sh/{NTFY_TOPIC.lstrip("/")}'
    headers = {'Title': title, 'Priority': priority}
    if NTFY_USER or NTFY_PASS:
        import base64
        cred = base64.b64encode(f'{NTFY_USER}:{NTFY_PASS}'.encode()).decode()
        headers['Authorization'] = f'Basic {cred}'
    try:
        req = urllib.request.Request(
            url,
            data=msg.encode(),
            headers=headers,
            method='POST',
        )
        urllib.request.urlopen(req, timeout=5)
    except Exception as e:
        print(f'[gmail_autoreply] ntfy failed: {e}', file=sys.stderr, flush=True)

def sender_addr(email):
    s = email.get('sender', '')
    m = re.search(r'<([^>]+)>', s)
    return (m.group(1) if m else s).strip().lower()

def is_eligible(email):
    """Only auto-reply to SUPPORT emails TO us FROM another @student.rjuhsd.us address."""
    to_field   = email.get('to', '').lower()
    from_addr  = sender_addr(email)
    if GMAIL_ADDR not in to_field:
        return False
    if not from_addr.endswith('@student.rjuhsd.us'):
        return False
    if from_addr == GMAIL_ADDR:
        return False
    return True

def send_autoreply(email):
    from_addr = sender_addr(email)
    if not from_addr:
        return False
    subject = email.get('subject', '')
    if not subject.lower().startswith('re:'):
        subject = 'Re: ' + subject
    body = ('Thank you for contacting mitch.pro support. Please reply with your problem '
            'and we will get you into contact with a mitch.pro representative as soon as possible.')
    msg_id = email.get('messageId', '')
    args = ['node', SEND_SCRIPT]
    if msg_id:
        args += ['--in-reply-to', f'<{msg_id}>']
    args += [from_addr, subject, body]
    r = subprocess.run(args, capture_output=True, text=True, timeout=30)
    if r.returncode == 0:
        print(f'[gmail_autoreply] Auto-replied to {from_addr}', flush=True)
        ntfy(f'SUPPORT email from {from_addr} — auto-replied', title='Support Request (Gmail)', priority='high')
        return True
    else:
        print(f'[gmail_autoreply] Failed to reply to {from_addr}: {r.stderr.strip()}', file=sys.stderr, flush=True)
        return False

def check():
    if not acquire_lock():
        print('[gmail_autoreply] Could not acquire lock, skipping run', flush=True)
        return
    try:
        try:
            with open(EMAILS_FILE, encoding='utf-8') as f:
                emails = json.load(f)
        except Exception:
            return
        sent = get_sent()
        changed = False
        for email in emails:
            key = make_key(email)
            if key in sent:
                continue
            unquoted_body = first_part_only(email.get('body') or '')
            if 'SUPPORT' not in unquoted_body:
                continue
            if not is_eligible(email):
                continue
            sent.add(key)
            changed = True
            try:
                send_autoreply(email)
            except Exception as e:
                print(f'[gmail_autoreply] Error: {e}', file=sys.stderr, flush=True)
        if changed:
            save_sent(sent)
    finally:
        release_lock()

if __name__ == '__main__':
    check()
