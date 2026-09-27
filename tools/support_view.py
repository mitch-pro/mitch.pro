#!/usr/bin/env python3
import curses, json, os, subprocess, textwrap, time, tempfile, sys, re, threading
import urllib.request, urllib.parse

BASE        = os.path.dirname(os.path.abspath(__file__))
READ_SCRIPT = os.path.join(BASE, 'mail', 'support_read.js')
SEND_SCRIPT = os.path.join(BASE, 'mail', 'support_send.js')
SMS_CONVOS_FILE = os.path.join(BASE, 'data', 'sms_conversations.json')
sms_convos_lock = threading.Lock()

# Load .env
try:
    for _line in open(os.path.join(os.path.dirname(BASE), '.env')):
        _m = re.match(r'^\s*(?:export\s+)?([A-Z_]+)\s*=\s*"?([^"#]*)"?\s*$', _line)
        if _m: os.environ.setdefault(_m.group(1), _m.group(2).strip())
except: pass

NTFY_TOPIC  = os.environ.get('NTFY_TOPIC', '').strip()
NTFY_USER   = os.environ.get('NTFY_USER', '').strip()
NTFY_PASS   = os.environ.get('NTFY_PASS', '').strip()
TEXTBELT_KEY = os.environ.get('TEXTBELT_API_KEY', '')

def _site():
    try: return json.load(open(os.path.join(BASE, 'data', 'site.json')))
    except: return {'primary': 'https://mitch.pro'}

WEBHOOK_URL = _site().get('primary', 'https://mitch.pro') + '/api/sms-reply'

def ntfy(msg, title=None):
    if not NTFY_TOPIC: return
    try:
        headers = {'Content-Type': 'text/plain'}
        if title: headers['Title'] = title
        if NTFY_USER or NTFY_PASS:
            import base64
            cred = base64.b64encode(f'{NTFY_USER}:{NTFY_PASS}'.encode()).decode()
            headers['Authorization'] = f'Basic {cred}'
        url = NTFY_TOPIC if NTFY_TOPIC.startswith(('http://', 'https://')) else f'https://ntfy.sh/{NTFY_TOPIC.lstrip("/")}'
        req = urllib.request.Request(
            url,
            data=msg.encode(), headers=headers, method='POST')
        urllib.request.urlopen(req, timeout=5)
    except Exception: pass

def textbelt_quota():
    if not TEXTBELT_KEY: return None
    try:
        with urllib.request.urlopen(f'https://textbelt.com/quota/{TEXTBELT_KEY}', timeout=5) as r:
            return json.loads(r.read()).get('quotaRemaining')
    except: return None

def load_convos():
    try: return json.load(open(SMS_CONVOS_FILE))
    except: return {}

def save_convos(convos):
    with open(SMS_CONVOS_FILE, 'w') as f: json.dump(convos, f)

def log_sms(phone, direction, text):
    digits = re.sub(r'\D', '', phone)
    if len(digits) == 11 and digits.startswith('1'): digits = digits[1:]
    if not digits: return
    with sms_convos_lock:
        convos = load_convos()
        if digits not in convos: convos[digits] = {'messages': [], 'unread': False}
        convos[digits]['messages'].append({'dir': direction, 'text': text, 'ts': int(time.time())})
        if direction == 'in': convos[digits]['unread'] = True
        save_convos(convos)

def send_sms(phone, msg):
    if not TEXTBELT_KEY:
        return False, 'TEXTBELT_API_KEY not set'
    digits = re.sub(r'\D', '', phone)
    if len(digits) == 10: digits = '1' + digits
    try:
        data = urllib.parse.urlencode({
            'phone': '+' + digits,
            'message': msg,
            'key': TEXTBELT_KEY,
            'replyWebhookUrl': WEBHOOK_URL,
        }).encode()
        req = urllib.request.Request('https://textbelt.com/text', data=data)
        with urllib.request.urlopen(req, timeout=10) as r:
            res = json.loads(r.read())
        if res.get('success'):
            log_sms(phone, 'out', msg)
            return True, f'textId={res.get("textId","?")}'
        return False, res.get('error', 'unknown error')
    except Exception as e:
        return False, str(e)

# ── Email helpers ──────────────────────────────────────────────────────────────

def fetch_list(all_mail=False, verbose=False):
    args = ['node', READ_SCRIPT]
    if all_mail: args.append('--all')
    try:
        r = subprocess.run(args, capture_output=True, text=True, timeout=20)
        if r.returncode != 0 or r.stderr.strip():
            if verbose: print(f'[support_read] stderr: {r.stderr.strip()}', flush=True)
        if not r.stdout.strip():
            if verbose: print('[support_read] empty output', flush=True)
            return []
        return json.loads(r.stdout)
    except Exception as e:
        if verbose: print(f'[fetch_list] error: {e}', flush=True)
        return []

def fetch_full(uid):
    try:
        r = subprocess.run(['node', READ_SCRIPT, '--uid', str(uid)],
                           capture_output=True, text=True, timeout=20)
        return json.loads(r.stdout)
    except: return None

def send_reply(to, subject, body, message_id=None):
    args = ['node', SEND_SCRIPT]
    if message_id:
        args += ['--in-reply-to', message_id]
    args += [to, subject]
    try:
        r = subprocess.run(args, input=body, capture_output=True, text=True, timeout=15)
        return r.returncode == 0, r.stderr.strip() or r.stdout.strip()
    except Exception as e:
        return False, str(e)

def fmt_date(iso):
    try:
        from datetime import datetime, timezone, timedelta
        dt = datetime.fromisoformat(iso.replace('Z','+00:00'))
        la = dt.astimezone(timezone(timedelta(hours=-7)))
        return la.strftime('%m/%d %I:%M %p')
    except: return iso[:16]

def fmt_ts(ts):
    try:
        from datetime import datetime, timezone, timedelta
        dt = datetime.fromtimestamp(ts, tz=timezone(timedelta(hours=-7)))
        return dt.strftime('%m/%d %I:%M %p')
    except: return str(ts)

def init_colors():
    curses.start_color(); curses.use_default_colors()
    curses.init_pair(1, curses.COLOR_CYAN,    -1)
    curses.init_pair(2, curses.COLOR_BLACK,   curses.COLOR_CYAN)
    curses.init_pair(3, curses.COLOR_YELLOW,  -1)
    curses.init_pair(4, curses.COLOR_GREEN,   -1)
    curses.init_pair(5, curses.COLOR_RED,     -1)
    curses.init_pair(6, curses.COLOR_MAGENTA, -1)

# ── SMS conversation thread view ───────────────────────────────────────────────

def convo_thread_view(stdscr, phone):
    curses.curs_set(1)
    input_buf = []
    flash = ''
    stdscr.timeout(2000)

    while True:
        H, W = stdscr.getmaxyx()
        stdscr.erase()

        # Mark as read
        with sms_convos_lock:
            convos = load_convos()
            if convos.get(phone, {}).get('unread'):
                convos[phone]['unread'] = False
                save_convos(convos)

        msgs = convos.get(phone, {}).get('messages', [])

        # Header
        try: stdscr.addstr(0, 0, f' ← {phone}'[:W-1], curses.color_pair(1)|curses.A_BOLD)
        except: pass
        try: stdscr.addstr(1, 0, '─'*(W-1), curses.color_pair(3))
        except: pass

        # Build display lines from messages
        msg_area_h = H - 5
        lines = []
        for m in msgs:
            ts   = fmt_ts(m.get('ts', 0))
            text = m.get('text', '')
            d    = m.get('dir', 'in')
            wrap_w = max(10, W - 18)
            wrapped = textwrap.wrap(text, wrap_w) or ['']
            for i, seg in enumerate(wrapped):
                lines.append((d, seg, ts if i == 0 else ''))

        visible = lines[max(0, len(lines) - msg_area_h):]
        for i, (d, seg, ts) in enumerate(visible):
            row = 2 + i
            if d == 'out':
                ts_s = f' {ts}' if ts else ''
                right = seg + ts_s
                col = max(0, W - len(right) - 2)
                try: stdscr.addstr(row, col, right[:W-1], curses.color_pair(4))
                except: pass
            else:
                prefix = f'{ts} ' if ts else '        '
                try: stdscr.addstr(row, 1, (prefix + seg)[:W-2], curses.color_pair(1))
                except: pass

        # Input area
        try: stdscr.addstr(H-3, 0, '─'*(W-1), curses.color_pair(3))
        except: pass

        if flash:
            attr = curses.color_pair(5) if '✗' in flash else curses.color_pair(4)
            try: stdscr.addstr(H-2, 0, f' {flash}'[:W-1], attr)
            except: pass
            flash = ''
        else:
            label = 'Reply: '
            inp = ''.join(input_buf)
            try:
                stdscr.addstr(H-2, 0, label + inp[:W-len(label)-1], curses.color_pair(3))
                stdscr.move(H-2, min(len(label)+len(inp), W-2))
            except: pass

        try: stdscr.addstr(H-1, 0, ' [Enter] send  [Esc] back'[:W-1], curses.color_pair(3))
        except: pass

        stdscr.refresh()
        key = stdscr.getch()

        if key == -1:
            continue
        elif key == 27 and not input_buf:
            break
        elif key in (curses.KEY_BACKSPACE, 127, 8):
            if input_buf: input_buf.pop()
        elif key in (10, 13):
            text = ''.join(input_buf).strip()
            if text:
                ok, result = send_sms(phone, text)
                flash = '✓ Sent' if ok else f'✗ {result}'
                if ok: input_buf = []
        elif 32 <= key <= 126:
            input_buf.append(chr(key))

    curses.curs_set(0)
    stdscr.timeout(-1)

# ── SMS conversation list ──────────────────────────────────────────────────────

def convo_list_view(stdscr):
    sel = 0; off = 0; flash = ''
    stdscr.timeout(3000)

    while True:
        H, W = stdscr.getmaxyx()
        stdscr.erase()

        convos = load_convos()
        phones = sorted(convos.keys(),
            key=lambda p: convos[p]['messages'][-1]['ts'] if convos[p].get('messages') else 0,
            reverse=True)

        try:
            hdr = ' SMS CONVERSATIONS'
            tab = '[Tab → Emails]'
            stdscr.addstr(0, 0, hdr.ljust(W-len(tab)-1) + tab, curses.A_BOLD)
        except: pass

        list_h = H - 2
        sel = max(0, min(sel, len(phones)-1 if phones else 0))
        if sel < off: off = sel
        if sel >= off + list_h: off = sel - list_h + 1

        if not phones:
            try: stdscr.addstr(2, 2, 'No SMS conversations yet. Texts to support@mitch.pro will appear here.', curses.color_pair(3))
            except: pass
        else:
            for row, phone in enumerate(phones[off:off+list_h]):
                idx  = row + off
                sel_ = idx == sel
                attr = curses.color_pair(2) if sel_ else 0
                c    = convos[phone]
                msgs = c.get('messages', [])
                last = msgs[-1] if msgs else {}
                unread = c.get('unread', False)
                dot  = ' ● ' if unread else '   '
                dot_a = curses.color_pair(2) if sel_ else curses.color_pair(1)
                ts_s = fmt_ts(last.get('ts', 0))[:14].ljust(14) if last else ' '*14
                preview = last.get('text', '')
                dir_s = '→' if last.get('dir') == 'out' else '←'
                preview_s = f'{dir_s} {preview}'[:max(0, W-35)]
                try: stdscr.addstr(row+1, 0, dot, dot_a)
                except: pass
                try: stdscr.addstr(row+1, 3, phone.ljust(14), attr)
                except: pass
                try: stdscr.addstr(row+1, 17, ts_s, attr)
                except: pass
                try: stdscr.addstr(row+1, 31, preview_s, attr)
                except: pass

        if flash:
            try: stdscr.addstr(H-1, 0, f' {flash}'[:W-1], curses.color_pair(4))
            except: pass
        else:
            try: stdscr.addstr(H-1, 0, ' [↑↓] nav  [Enter] open  [Tab] emails  [q] quit'[:W-1], curses.color_pair(3))
            except: pass

        stdscr.refresh()
        flash = ''
        key = stdscr.getch()

        if key == -1: continue
        elif key in (ord('q'), ord('Q')):
            stdscr.timeout(-1); return 'quit'
        elif key == ord('\t'):
            stdscr.timeout(-1); return 'email'
        elif key == curses.KEY_UP:   sel = max(0, sel-1)
        elif key == curses.KEY_DOWN: sel = min(len(phones)-1, sel+1) if phones else 0
        elif key in (curses.KEY_ENTER, 10, 13):
            if phones:
                convo_thread_view(stdscr, phones[sel])

# ── Email list view ────────────────────────────────────────────────────────────

def reply_editor(stdscr, email):
    curses.endwin()
    with tempfile.NamedTemporaryFile(suffix='.txt', mode='w', delete=False) as f:
        fname = f.name
    editor = os.environ.get('EDITOR', 'nano')
    subprocess.call([editor, fname])
    body = open(fname).read().strip()
    os.unlink(fname)
    stdscr = curses.initscr()
    curses.curs_set(0); init_colors()
    return stdscr, body

def read_view(stdscr, email):
    full = fetch_full(email['uid'])
    body = full.get('body', email.get('body','')) if full else email.get('body','')
    offset = 0; flash = ''
    while True:
        H, W = stdscr.getmaxyx()
        stdscr.erase()
        try: stdscr.addstr(0, 0, f' From: {email["fromName"]} <{email["from"]}>'[:W-1], curses.color_pair(1)|curses.A_BOLD)
        except: pass
        try: stdscr.addstr(1, 0, f' To:   {email["to"]}'[:W-1], curses.color_pair(3))
        except: pass
        try: stdscr.addstr(2, 0, f' Subj: {email["subject"]}'[:W-1], curses.color_pair(3))
        except: pass
        try: stdscr.addstr(3, 0, f' Date: {fmt_date(email["date"])}'[:W-1], curses.color_pair(3))
        except: pass
        try: stdscr.addstr(4, 0, '─'*(W-1), curses.color_pair(3))
        except: pass

        lines = []
        for para in body.split('\n'):
            wrapped = textwrap.wrap(para, W-3) if para.strip() else ['']
            lines.extend(wrapped)

        vh = H - 7
        offset = max(0, min(offset, max(0, len(lines) - vh)))
        for i, line in enumerate(lines[offset: offset+vh]):
            try: stdscr.addstr(5+i, 2, line[:W-3])
            except: pass

        scroll = f' {offset+1}-{min(offset+vh,len(lines))}/{len(lines)} ' if len(lines)>vh else ''
        if flash:
            status = f' {flash}'
        else:
            status = f' [↑↓] scroll  [r] reply  [q] back{scroll.rjust(max(0,W-1-30))}'
        try: stdscr.addstr(H-1, 0, status[:W-1], curses.color_pair(5) if flash else curses.color_pair(3))
        except: pass
        stdscr.refresh()

        key = stdscr.getch()
        flash = ''
        if key in (ord('q'), 27): break
        elif key in (curses.KEY_UP, ord('k')):    offset = max(0, offset-1)
        elif key in (curses.KEY_DOWN, ord('j')):  offset = min(max(0,len(lines)-vh), offset+1)
        elif key == curses.KEY_PPAGE:             offset = max(0, offset-vh)
        elif key == curses.KEY_NPAGE:             offset = min(max(0,len(lines)-vh), offset+vh)
        elif key == ord('r'):
            stdscr, body_reply = reply_editor(stdscr, email)
            if body_reply:
                subj = email['subject']
                if not subj.lower().startswith('re:'): subj = 'Re: ' + subj
                ok, msg = send_reply(email['from'], subj, body_reply, email.get('messageId'))
                flash = ('✓ Sent' if ok else f'✗ {msg}')
            else:
                flash = 'Cancelled.'

def email_list_view(stdscr):
    sel = 0; off = 0; show_all = False; emails = []; flash = ''; loading = True

    def load():
        nonlocal emails, loading, flash
        loading = True; flash = 'Loading...'
        prev_count = len(emails)
        emails = fetch_list(show_all)
        new_count = len(emails)
        loading = False; flash = f'{new_count} email(s) loaded.'
        if new_count > prev_count:
            threading.Thread(target=ntfy,
                args=(f'{new_count - prev_count} new message(s)',),
                kwargs={'title': 'New Support Email'}, daemon=True).start()

    load()

    while True:
        H, W = stdscr.getmaxyx()
        stdscr.erase()

        col_from = max(18, W // 3)
        col_date = 14
        col_seen = 3

        try:
            hdr = ' '*col_seen + 'FROM'.ljust(col_from) + 'DATE'.ljust(col_date) + 'SUBJECT'
            tab = '[Tab → SMS]'
            stdscr.addstr(0, 0, (hdr[:W-len(tab)-1]).ljust(W-len(tab)-1) + tab, curses.A_BOLD)
        except: pass

        list_h = H - 2
        sel = max(0, min(sel, len(emails)-1 if emails else 0))
        if sel < off: off = sel
        if sel >= off + list_h: off = sel - list_h + 1

        if not emails:
            try: stdscr.addstr(2, 2, 'No emails.' if not loading else 'Loading...', curses.color_pair(3))
            except: pass
        else:
            for row, e in enumerate(emails[off: off+list_h]):
                i    = row + off
                sel_ = i == sel
                attr = curses.color_pair(2) if sel_ else 0
                seen_s = '   ' if e.get('seen') else ' ● '
                seen_a = curses.color_pair(2) if sel_ else curses.color_pair(1)
                from_s = (e.get('fromName') or e.get('from',''))[:col_from-1].ljust(col_from)
                date_s = fmt_date(e.get('date',''))[:col_date-1].ljust(col_date)
                subj_s = e.get('subject','')[:max(0, W-col_seen-col_from-col_date-1)]
                try: stdscr.addstr(row+1, 0, seen_s, seen_a)
                except: pass
                try: stdscr.addstr(row+1, col_seen, from_s, attr)
                except: pass
                try: stdscr.addstr(row+1, col_seen+col_from, date_s, attr)
                except: pass
                try: stdscr.addstr(row+1, col_seen+col_from+col_date, subj_s, attr)
                except: pass

        if flash:
            status = f' {flash}'
            sattr = curses.color_pair(4) if '✓' in flash or 'loaded' in flash else curses.color_pair(3)
        else:
            mode = 'all' if show_all else 'unread'
            status = f' [↑↓] nav  [Enter] read  [r] reply  [a] toggle({mode})  [R] refresh  [Tab] sms  [q] quit'
            sattr = curses.color_pair(3)
        try: stdscr.addstr(H-1, 0, status[:W-1], sattr)
        except: pass
        stdscr.refresh()
        key = stdscr.getch()
        flash = ''

        if key in (ord('q'), ord('Q')): return 'quit'
        elif key == ord('\t'): return 'sms'
        elif key == curses.KEY_UP:   sel = max(0, sel-1)
        elif key == curses.KEY_DOWN: sel = min(len(emails)-1, sel+1) if emails else 0
        elif key in (ord('R'),):
            load(); sel = 0; off = 0
        elif key == ord('a'):
            show_all = not show_all; load(); sel = 0; off = 0
        elif key in (curses.KEY_ENTER, 10, 13):
            if emails: read_view(stdscr, emails[sel])
        elif key == ord('r'):
            if emails:
                stdscr, body_reply = reply_editor(stdscr, emails[sel])
                if body_reply:
                    e = emails[sel]
                    subj = e['subject']
                    if not subj.lower().startswith('re:'): subj = 'Re: ' + subj
                    ok, msg = send_reply(e['from'], subj, body_reply, e.get('messageId'))
                    flash = '✓ Sent' if ok else f'✗ {msg}'
                else:
                    flash = 'Cancelled.'

# ── Top-level ──────────────────────────────────────────────────────────────────

def main(stdscr):
    curses.curs_set(0)
    init_colors()
    mode = 'sms'
    while True:
        if mode == 'sms':
            result = convo_list_view(stdscr)
            if result == 'quit': break
            elif result == 'email': mode = 'email'
        else:
            result = email_list_view(stdscr)
            if result == 'quit': break
            elif result == 'sms': mode = 'sms'

# ── Daemon ─────────────────────────────────────────────────────────────────────

AUTO_REPLY = "Thanks for contacting mitch.pro support. Reply with your issue and we'll get back to you. - support@mitch.pro"

SEEN_FILE = os.path.join(BASE, 'data', '.support_daemon_seen.json')

def load_seen():
    try: return set(json.load(open(SEEN_FILE)))
    except: return set()

def save_seen(seen):
    with open(SEEN_FILE, 'w') as f: json.dump(list(seen), f)

def daemon():
    import re as _re
    seen = load_seen()
    print(f'[daemon] started — polling every 10s, {len(seen)} emails already seen', flush=True)
    poll = 0
    while True:
        poll += 1
        print(f'[daemon] poll #{poll} — fetching...', flush=True)
        try:
            emails = fetch_list(all_mail=True, verbose=True)
            print(f'[daemon] got {len(emails)} email(s)', flush=True)
            new = [e for e in emails if str(e.get('uid','')) not in seen]
            print(f'[daemon] {len(new)} new (unseen)', flush=True)
            for e in new:
                uid      = str(e.get('uid', ''))
                to_addr  = (e.get('to') or '').lower()
                if 'support@mitch.pro' not in to_addr:
                    print(f'[daemon] uid={uid} — not addressed to support@mitch.pro (to={to_addr!r}), skipping', flush=True)
                    seen.add(uid); save_seen(seen)
                    continue
                raw_body = (e.get('body') or '').strip()
                subj_raw = (e.get('subject') or '')
                if not raw_body:
                    print(f'[daemon] uid={uid} — empty body from list, doing full fetch', flush=True)
                    full = fetch_full(uid)
                    raw_body = (full.get('body') or '').strip() if full else ''
                clean   = _re.sub(r'[^a-zA-Z0-9]', '', raw_body).upper()
                subj_cl = _re.sub(r'[^a-zA-Z0-9]', '', subj_raw).upper()
                print(f'[daemon] uid={uid} from={e.get("from","")} body={repr(raw_body[:60])} clean={repr(clean[:30])}', flush=True)
                is_support = (
                    clean == 'SUPPORT' or
                    clean.startswith('SUPPORT') or
                    'SUPPORT' in subj_cl
                )
                seen.add(uid)
                save_seen(seen)
                if not is_support:
                    print(f'[daemon] uid={uid} — no SUPPORT keyword, skipping', flush=True)
                    continue
                frm        = e.get('from', '')
                reply_subj = ('Re: ' + subj_raw) if not subj_raw.upper().startswith('RE:') else subj_raw
                phone_m    = _re.match(r'^(\+?1?(\d{10}))@', frm)
                display    = f'{phone_m.group(2)} ({frm})' if phone_m else frm
                print(f'[daemon] MATCH — replying to {display}', flush=True)

                if phone_m and TEXTBELT_KEY:
                    phone = phone_m.group(2)
                    log_sms(phone, 'in', raw_body)
                    print(f'[daemon] sending SMS via Textbelt to {phone}', flush=True)
                    ok, msg = send_sms(phone, AUTO_REPLY)
                    quota = textbelt_quota()
                    quota_str = f' — {quota} texts remaining' if quota is not None else ''
                    print(f'[daemon] sms result: ok={ok} msg={msg!r}{quota_str}', flush=True)
                    ntfy(f'{display}: {raw_body[:120]}{quota_str}', title='Support Request')
                else:
                    ok, msg = send_reply(frm, reply_subj, AUTO_REPLY, e.get('messageId'))
                    print(f'[daemon] email result: ok={ok} msg={msg!r}', flush=True)
                    ntfy(f'{display}: {raw_body[:120]}', title='Support Request')
        except Exception as ex:
            print(f'[daemon] error: {ex}', flush=True)
        print(f'[daemon] sleeping 10s...', flush=True)
        time.sleep(10)

if '-d' in sys.argv:
    daemon()
else:
    curses.wrapper(main)
