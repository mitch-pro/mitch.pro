#!/usr/bin/env python3
"""
mitch.pro Admin Tools — Unified CLI Dashboard
Consolidates: Applications, Access, Sessions, DMs, Suggestions, Cheat Logs,
Economy, Moderation, Casino, Prox, Content, Notifications, Invites, Leaderboard.
Usage: ./admin_tools.py
"""

import curses
import json
import os
import subprocess
import sys
import textwrap
import time
import base64
import hashlib
import hmac
import secrets
import sqlite3
import urllib.request
import urllib.error
from datetime import datetime, timezone, timedelta
from pathlib import Path
from collections import Counter, defaultdict

# ── Globals & Constants ───────────────────────────────────────────────────────

BASE             = Path(__file__).parent.parent.absolute()
DATA             = BASE / 'data'
ID_SECRET_FILE   = DATA / 'id_secret.key'
SITE_FILE        = DATA / 'site.json'
APPS_FILE        = DATA / 'applications.json'
TOKENS_FILE      = DATA / 'tokens.json'
REVOKED_FILE     = DATA / 'revoked.json'
APPEALS_FILE     = DATA / 'appeals.json'
BLACKLIST_FILE   = DATA / 'blacklist.json'
GENERATIONS_FILE = DATA / 'generations.json'
NAMES_FILE       = DATA / 'names.json'
SESS_LOG_FILE    = DATA / 'sessions.json'
DMS_FILE         = DATA / 'dms.json'
SUGGESTIONS_FILE = DATA / 'suggestions.json'
CHEAT_LOGS_FILE  = DATA / 'cheat_logs.json'
COINS_FILE       = DATA / 'coins.json'
USER_STATS_FILE  = DATA / 'user_stats.json'
INVITE_CODES_FILE  = DATA / 'invite_codes.json'
INVITE_CLAIMS_FILE = DATA / 'invite_claims.json'
PUSH_SUBS_FILE   = DATA / 'push_subs.json'
NEWSLETTER_UNSUB_FILE  = DATA / 'newsletter_unsub.json'
NEWSLETTER_EXTRA_FILE  = DATA / 'newsletter_extra.json'
UNSUB_REQS_FILE  = DATA / 'unsubscribe_requests.json'
SEND_SCRIPT      = BASE / 'mail' / 'send_email.js'
NOREPLY_SCRIPT   = BASE / 'mail' / 'noreply_send.js'
APPS_SEND_JS     = BASE / 'mail' / 'support_send.js'

LA = timezone(timedelta(hours=-7))

def load_site():
    try: return json.loads(SITE_FILE.read_text())
    except: return {'primary': 'https://mitch.pro', 'alternate': 'https://mitch.88chan.me', 'name': 'mitch.pro'}

def get_id_secret():
    try: return ID_SECRET_FILE.read_bytes()
    except: return b''

SITE = load_site()
ID_SECRET = get_id_secret()

# ── Curses helpers ────────────────────────────────────────────────────────────

def safe_addstr(win, y, x, text, attr=0):
    try:
        h, w = win.getmaxyx()
        if y < 0 or y >= h or x < 0 or x >= w: return
        max_len = (w - x - 1) if y == h - 1 else (w - x)
        win.addstr(y, x, str(text)[:max_len], attr)
    except Exception: pass

def draw_header(stdscr, title, color_pair=3):
    h, w = stdscr.getmaxyx()
    stdscr.attron(curses.A_BOLD | curses.color_pair(color_pair))
    safe_addstr(stdscr, 0, 0, f' {title} '.center(w))
    stdscr.attroff(curses.A_BOLD | curses.color_pair(color_pair))

def draw_footer(stdscr, text, color_pair=1):
    h, w = stdscr.getmaxyx()
    stdscr.attron(curses.color_pair(color_pair) | curses.A_DIM)
    safe_addstr(stdscr, h - 1, 0, text[:w])
    stdscr.attroff(curses.color_pair(color_pair) | curses.A_DIM)

def confirm(stdscr, prompt):
    h, w = stdscr.getmaxyx()
    stdscr.attron(curses.color_pair(4) | curses.A_BOLD)
    safe_addstr(stdscr, h - 2, 2, (prompt + '  [y/N] ')[:w - 2])
    stdscr.attroff(curses.color_pair(4) | curses.A_BOLD)
    stdscr.refresh()
    return stdscr.getch() in (ord('y'), ord('Y'))

def init_colors():
    curses.start_color(); curses.use_default_colors()
    curses.init_pair(1,  curses.COLOR_WHITE,  -1)
    curses.init_pair(2,  curses.COLOR_GREEN,  -1)
    curses.init_pair(3,  curses.COLOR_YELLOW, -1)
    curses.init_pair(4,  curses.COLOR_RED,    -1)
    curses.init_pair(5,  curses.COLOR_CYAN,   -1)
    curses.init_pair(6,  curses.COLOR_MAGENTA,-1)
    curses.init_pair(7,  curses.COLOR_BLACK,  curses.COLOR_CYAN)
    curses.init_pair(8,  curses.COLOR_BLACK,  curses.COLOR_YELLOW)
    curses.init_pair(9,  curses.COLOR_BLACK,  curses.COLOR_GREEN)
    curses.init_pair(10, curses.COLOR_BLACK,  curses.COLOR_RED)

# ── Util ──────────────────────────────────────────────────────────────────────

def fmt_ts(ts, ms=False):
    try:
        t = ts / 1000 if ms else ts
        return datetime.fromtimestamp(t, tz=LA).strftime('%m/%d %I:%M %p')
    except: return str(ts)

def fmt_iso(ts_str):
    try:
        dt = datetime.fromisoformat(ts_str.replace('Z', '+00:00')).astimezone(LA)
        return dt.strftime('%m/%d %I:%M %p')
    except: return ts_str[:16]

def ago(ts):
    s = time.time() - ts
    if s < 60:    return f'{int(s)}s ago'
    if s < 3600:  return f'{int(s/60)}m ago'
    if s < 86400: return f'{int(s/3600)}h ago'
    return f'{int(s/86400)}d ago'

def load_json(path, default=None):
    try: return json.loads(Path(path).read_text())
    except: return default if default is not None else {}

def prox_blocklist_domains():
    """Blocked proxy domains, read from the server's prox_blocklist table."""
    try:
        conn = sqlite3.connect('file:%s?mode=ro' % (DATA / 'mitchpro.db'), uri=True, timeout=5)
        try:
            return [r[0] for r in conn.execute('SELECT domain FROM prox_blocklist')]
        finally:
            conn.close()
    except Exception:
        return []

def save_json(path, data):
    tmp = str(path) + '.tmp'
    with open(tmp, 'w') as f: json.dump(data, f, indent=2)
    os.replace(tmp, path)

def osc52_copy(text):
    try:
        b64 = base64.b64encode(text.encode()).decode()
        with open('/dev/tty', 'wb', buffering=0) as tty:
            tty.write(f'\033]52;c;{b64}\007'.encode())
        return True
    except: return False

def normalize_email(email):
    if '@' not in email: return email
    l, d = email.rsplit('@', 1)
    return l.split('+')[0].replace('.', '') + '@' + d

def send_email(to, subject, body):
    script = SEND_SCRIPT if to.lower().endswith('@student.rjuhsd.us') else NOREPLY_SCRIPT
    try:
        r = subprocess.run(['node', str(script), to, subject, body],
                           capture_output=True, text=True, timeout=15)
        return r.returncode == 0, r.stderr.strip() or r.stdout.strip()
    except Exception as e: return False, str(e)

def call_admin_api(path, payload=None, method='POST'):
    """Call the local server admin API with the admin key."""
    try:
        key = (BASE / 'admin' / 'admin.key').read_text().strip()
    except: return None, 'admin.key missing'
    url = f'http://127.0.0.1:6800{path}'
    data = json.dumps(payload or {}).encode() if method == 'POST' else None
    req = urllib.request.Request(url, data=data,
        headers={'Content-Type': 'application/json', 'X-Admin-Key': key},
        method=method)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.loads(r.read()), None
    except urllib.error.HTTPError as e:
        return None, f'HTTP {e.code}: {e.read().decode()[:200]}'
    except Exception as e: return None, str(e)

def fmt_coins(n):
    try:
        n = float(n)
        if n >= 1_000_000: return f'{n/1_000_000:.2f}M'
        if n >= 1_000: return f'{n/1_000:.1f}K'
        return f'{n:.1f}'
    except: return str(n)

# ── Shared list navigator ─────────────────────────────────────────────────────

def list_nav(stdscr, items, draw_row, draw_detail, footer,
             allow_delete=False, delete_fn=None):
    """Generic scrollable list with detail view. Returns when user presses q."""
    sel = 0; off = 0; view = 'list'; flash_msg = ''; flash_until = 0

    while True:
        h, w = stdscr.getmaxyx()
        list_h = h - 4

        if view == 'list':
            stdscr.erase()
            sel = max(0, min(sel, len(items) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            for i, item in enumerate(items[off:off + list_h]):
                abs_i = i + off
                draw_row(stdscr, item, i + 2, abs_i == sel, w)
            draw_footer(stdscr, footer)
            if flash_msg and time.time() < flash_until:
                safe_addstr(stdscr, h - 2, 2, flash_msg, curses.color_pair(2))
            stdscr.refresh()
            key = stdscr.getch()
            if key in (ord('q'), 27): break
            elif key == curses.KEY_UP: sel = max(0, sel - 1)
            elif key == curses.KEY_DOWN: sel = min(len(items) - 1, sel + 1)
            elif key in (curses.KEY_ENTER, 10, 13, curses.KEY_RIGHT) and items:
                view = 'detail'
            elif key == ord('E') and items:
                email = getattr(items[sel], 'get', lambda k, d='': items[sel].get(k, d) if isinstance(items[sel], dict) else '')('email', '')
                if email and osc52_copy(email):
                    flash_msg = f'Copied {email}'; flash_until = time.time() + 2
        else:
            stdscr.erase()
            if items: draw_detail(stdscr, items[sel], h, w)
            draw_footer(stdscr, ' [q/←] back  [E] copy email ')
            stdscr.refresh()
            key = stdscr.getch()
            if key in (ord('q'), 27, curses.KEY_LEFT): view = 'list'

# ══════════════════════════════════════════════════════════════════════════════
# 1. APPLICATIONS
# ══════════════════════════════════════════════════════════════════════════════

PREMIUM_APPROVAL_BODY = """Hi {name},

Great news — your mitch.pro Premium application has been approved!

Your account ({email}) now has Premium access. Log in to enjoy:
  • Exclusive games, canvas brush sizes, custom palette colours
  • Supporter badge, custom profile, chess board themes
  • 2× MitchCoins on games, offline gains, and invites
  • Priority support & early feature access

If anything isn't working reply to this email or reach support@mitch.pro.

Welcome to Premium!
— mitch.pro"""

def applications_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    filter_type = 'all'; sel = 0; off = 0; view = 'list'; status_msg = ''
    filters = ['all', 'premium', 'team', 'approved', 'rejected']

    while True:
        all_apps = load_json(APPS_FILE, [])
        apps = [a for a in all_apps if filter_type in ('all', a.get('status', 'pending'))
                or a.get('type', 'team') == filter_type]
        h, w = stdscr.getmaxyx(); list_h = h - 6

        if view == 'list':
            stdscr.erase()
            draw_header(stdscr, f'Applications — {filter_type.upper()} ({len(apps)})')
            # Tab bar
            tx = 2; safe_addstr(stdscr, 1, 0, ' ' * w)
            for f in filters:
                tag = f' [{f}] ' if f == filter_type else f'  {f}  '
                attr = curses.A_BOLD | curses.color_pair(2) if f == filter_type else curses.color_pair(1)
                stdscr.attron(attr); safe_addstr(stdscr, 1, tx, tag); stdscr.attroff(attr)
                tx += len(tag) + 1
            stdscr.attron(curses.color_pair(1) | curses.A_DIM)
            hdr = f"  {'TYPE':<8} {'STATUS':<10} {'NAME':<20} {'EMAIL':<28} DATE"
            safe_addstr(stdscr, 2, 0, hdr[:w]); safe_addstr(stdscr, 3, 0, '─' * w)
            stdscr.attroff(curses.color_pair(1) | curses.A_DIM)
            sel = max(0, min(sel, len(apps) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            for i, app in enumerate(apps[off:off + list_h]):
                abs_i = i + off
                atype = app.get('type', 'team')
                status = app.get('status', 'pending')
                name = app.get('name', '')[:19]
                email = app.get('email', '')[:27]
                ts = app.get('submitted_at', 0)
                date = fmt_ts(ts / 1000 if ts > 1e10 else ts)[:16]
                line = f"  {atype:<8} {status:<10} {name:<20} {email:<28} {date}"
                attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else \
                       (curses.color_pair(2) if status == 'approved' else
                        (curses.color_pair(4) if status == 'rejected' else 0))
                safe_addstr(stdscr, 4 + i, 0, line.ljust(w), attr)
            draw_footer(stdscr, ' [↑↓] nav  [Enter] view  [f] filter  [q] back ')
            if status_msg:
                safe_addstr(stdscr, h - 2, 2, status_msg,
                            curses.color_pair(2) if '✓' in status_msg else curses.color_pair(4))
            stdscr.refresh()
            key = stdscr.getch()
            if key in (ord('q'), 27): break
            elif key == curses.KEY_UP: sel = max(0, sel - 1)
            elif key == curses.KEY_DOWN: sel = min(len(apps) - 1, sel + 1)
            elif key in (ord('f'), ord('F')):
                filter_type = filters[(filters.index(filter_type) + 1) % len(filters)]
                sel = 0; off = 0
            elif key in (curses.KEY_ENTER, 10, 13, curses.KEY_RIGHT) and apps:
                view = 'detail'; status_msg = ''
            elif key == ord('E') and apps:
                em = apps[sel].get('email', '')
                if em and osc52_copy(em): status_msg = f'Copied {em}'

        elif view == 'detail':
            app = apps[sel]
            stdscr.erase()
            header = f" {app.get('type', 'team').upper()} Application — {app.get('status', '?').upper()} "
            draw_header(stdscr, header)
            row = 2
            def field(label, value, color=0):
                nonlocal row
                if row >= h - 4: return
                stdscr.attron(curses.A_BOLD); safe_addstr(stdscr, row, 2, f'{label}:'); stdscr.attroff(curses.A_BOLD)
                row += 1
                for part in str(value or '—').splitlines():
                    for l in textwrap.wrap(part, w - 6) or ['']:
                        if row >= h - 4: break
                        if color: stdscr.attron(curses.color_pair(color))
                        safe_addstr(stdscr, row, 4, l[:w - 4])
                        if color: stdscr.attroff(curses.color_pair(color))
                        row += 1
                row += 1
            field('Name', app.get('name', ''))
            field('Email', app.get('email', ''), color=5)
            field('Discord', app.get('discord', '') or '—')
            ts = app.get('submitted_at', 0)
            field('Submitted', fmt_ts(ts / 1000 if ts > 1e10 else ts))
            field('Why', app.get('why', ''))
            if app.get('skills'): field('Skills', app['skills'])
            if app.get('extra'): field('Extra', app['extra'])
            if status_msg and row < h - 2:
                col = curses.color_pair(2) if '✓' in status_msg else curses.color_pair(4)
                safe_addstr(stdscr, row, 2, status_msg[:w - 2], col)
            status = app.get('status', 'pending')
            acts = []
            if status == 'pending': acts += ['[a] approve', '[r] reject']
            if status == 'approved': acts.append('[v] revoke')
            if status != 'pending': acts.append('[u] reset→pending')
            acts.append('[E] copy email')
            draw_footer(stdscr, ' [←/q] back  ' + '  '.join(acts))
            stdscr.refresh()
            key = stdscr.getch()
            if key in (ord('q'), 27, curses.KEY_LEFT): view = 'list'
            elif key == ord('E'):
                em = app.get('email', '')
                if em and osc52_copy(em): status_msg = f'Copied {em}'
            elif key in (ord('a'), ord('A')) and status == 'pending':
                if confirm(stdscr, f"Approve {app.get('email')}?"):
                    all_apps2 = load_json(APPS_FILE, [])
                    for a in all_apps2:
                        if a.get('email') == app['email'] and a.get('submitted_at') == app.get('submitted_at'):
                            a['status'] = 'approved'; a['approved_at'] = int(time.time() * 1000)
                            if a.get('type', 'team') != 'premium':
                                a['grantPremium'] = True; a['neverExpire'] = True
                    save_json(APPS_FILE, all_apps2)
                    if app.get('type') == 'premium':
                        body = PREMIUM_APPROVAL_BODY.format(name=app.get('name', 'there'), email=app.get('email', ''))
                        r = subprocess.run(['node', str(APPS_SEND_JS), '--raw', app['email'],
                                            'Your mitch.pro Premium application has been approved', body],
                                           capture_output=True, timeout=30)
                        status_msg = '✓ Approved and emailed' if r.returncode == 0 else f'✗ Email failed'
                    else: status_msg = '✓ Approved (premium granted)'
            elif key in (ord('r'), ord('R')) and status == 'pending':
                if confirm(stdscr, f"Reject {app.get('email')}?"):
                    all_apps2 = load_json(APPS_FILE, [])
                    for a in all_apps2:
                        if a.get('email') == app['email'] and a.get('submitted_at') == app.get('submitted_at'):
                            a['status'] = 'rejected'
                    save_json(APPS_FILE, all_apps2); status_msg = '✗ Rejected'
            elif key in (ord('u'), ord('U')) and status != 'pending':
                all_apps2 = load_json(APPS_FILE, [])
                for a in all_apps2:
                    if a.get('email') == app['email'] and a.get('submitted_at') == app.get('submitted_at'):
                        a['status'] = 'pending'; a.pop('approved_at', None); a.pop('grantPremium', None)
                save_json(APPS_FILE, all_apps2); status_msg = '↩ Reset to pending'
            elif key in (ord('v'), ord('V')) and status == 'approved':
                if confirm(stdscr, f"Revoke premium from {app.get('email')}?"):
                    all_apps2 = load_json(APPS_FILE, [])
                    for a in all_apps2:
                        if a.get('email') == app['email'] and a.get('submitted_at') == app.get('submitted_at'):
                            a['status'] = 'rejected'; a.pop('grantPremium', None)
                    save_json(APPS_FILE, all_apps2); status_msg = '✓ Premium revoked'

# ══════════════════════════════════════════════════════════════════════════════
# 2. ACCESS / TOKENS
# ══════════════════════════════════════════════════════════════════════════════

def access_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    state = {'view': 'requests', 'sel': 0, 'appeal_sel': 0, 'bl_sel': 0,
             'unsub_sel': 0, 'flash': None, 'req_off': 0, 'appeal_off': 0,
             'bl_off': 0, 'unsub_off': 0, 'show_ip': False}
    tabs = [('1', 'Requests', 'requests'), ('2', 'Appeals', 'appeals'),
            ('3', 'Blacklist', 'blacklist'), ('4', 'Unsub', 'unsub')]

    def flash(msg, col=2, dur=3): state['flash'] = (msg, time.time() + dur, col)

    def load_pending():
        tokens = load_json(TOKENS_FILE, {})
        res = []
        for t, d in tokens.items():
            if not d.get('used') or d.get('claimed_domains'):
                res.append({'token': t, 'email': d.get('email', '?'),
                            'ip': d.get('enroll_ip', ''), 'ts': d.get('created_at', 0),
                            'claimed': list(d.get('claimed_domains', {}).keys())})
        return sorted(res, key=lambda x: x['ts'])

    while True:
        pending = load_pending()
        appeals = load_json(APPEALS_FILE, [])
        blacklist = load_json(BLACKLIST_FILE, {})
        unsub_reqs = load_json(UNSUB_REQS_FILE, [])
        h, w = stdscr.getmaxyx(); stdscr.erase()

        x = 0
        for k, l, v in tabs:
            attr = curses.color_pair(7) | curses.A_BOLD if state['view'] == v else curses.color_pair(3)
            safe_addstr(stdscr, 0, x, f' {k}:{l} ', attr); x += len(l) + 4
        safe_addstr(stdscr, 1, 0, '─' * w, curses.color_pair(3))

        view = state['view']
        items = {'requests': pending, 'appeals': appeals,
                 'blacklist': list(blacklist.items()), 'unsub': unsub_reqs}[view]
        sk = {'requests': 'sel', 'appeals': 'appeal_sel', 'blacklist': 'bl_sel', 'unsub': 'unsub_sel'}[view]
        ok = {'requests': 'req_off', 'appeals': 'appeal_off', 'blacklist': 'bl_off', 'unsub': 'unsub_off'}[view]
        state[sk] = max(0, min(state[sk], max(0, len(items) - 1)))
        sel = state[sk]; list_h = h - 8
        if sel < state[ok]: state[ok] = sel
        if sel >= state[ok] + list_h: state[ok] = sel - list_h + 1

        for i, it in enumerate(items[state[ok]:state[ok] + list_h]):
            abs_i = i + state[ok]
            attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
            if view == 'requests':
                display = it['ip'] if state['show_ip'] else it['email']
                txt = f"{display:<42} {fmt_ts(it['ts'])}"
            elif view == 'appeals':
                txt = f"{it.get('email','?'):<42} {fmt_ts(it.get('submitted_at', 0))}"
            elif view == 'blacklist':
                txt = f"{it[0]:<42} {fmt_ts(it[1].get('blacklisted_at', 0))}"
            else:
                txt = f"{it.get('email','?'):<42} {fmt_ts(it.get('submitted_at', 0))}"
            safe_addstr(stdscr, 3 + i, 2, txt[:w - 4], attr)

        safe_addstr(stdscr, h - 4, 0, '─' * w, curses.color_pair(3))
        if items:
            it = items[sel]
            if view == 'requests':
                link = f"{SITE['alternate']}/claim.html?token={it['token']}"
                safe_addstr(stdscr, h - 3, 2, f"Token link: {link}"[:w - 4], curses.color_pair(5))
            elif view == 'appeals':
                safe_addstr(stdscr, h - 3, 2, f"Reason: {it.get('reason','')}"[:w - 4], curses.color_pair(4))
            elif view == 'blacklist':
                safe_addstr(stdscr, h - 3, 2, f"Reason: {it[1].get('reason','')}"[:w - 4], curses.color_pair(4))

        if state['flash'] and time.time() < state['flash'][1]:
            safe_addstr(stdscr, h - 2, 2, state['flash'][0], curses.color_pair(state['flash'][2]))
        else:
            safe_addstr(stdscr, h - 2, 2, ' ↑↓ nav  Enter send/approve  E copy  p IP  1-4 tabs  T team-approve  q back', curses.color_pair(3))

        stdscr.refresh(); stdscr.timeout(1000); key = stdscr.getch()
        if key in (ord('q'), 27): break
        elif ord('1') <= key <= ord('4'): state['view'] = tabs[key - ord('1')][2]
        elif key == curses.KEY_UP: state[sk] = max(0, state[sk] - 1)
        elif key == curses.KEY_DOWN: state[sk] = min(len(items) - 1, state[sk] + 1)
        elif key == ord('p'): state['show_ip'] = not state['show_ip']
        elif key == ord('E') and items:
            em = items[sel]['email'] if view in ('requests', 'appeals', 'unsub') else items[sel][0]
            if osc52_copy(em): flash(f'Copied {em}')
        elif key == ord('T') and view == 'requests' and items:
            it = items[sel]; email = it['email']
            if confirm(stdscr, f'Silent Team Approve {email}?'):
                try:
                    toks = load_json(TOKENS_FILE, {})
                    newTok = secrets.token_hex(24)
                    toks[newTok] = {'email': email, 'created_at': time.time(), 'used': False}
                    if it['token'] in toks: del toks[it['token']]
                    save_json(TOKENS_FILE, toks)
                    apps = load_json(APPS_FILE, [])
                    apps.insert(0, {'name': email.split('@')[0], 'email': email.lower(),
                                    'type': 'team', 'status': 'approved', 'grantPremium': True,
                                    'why': 'Silent approved as Team',
                                    'submitted_at': int(time.time() * 1000),
                                    'approved_at': int(time.time() * 1000)})
                    save_json(APPS_FILE, apps); flash(f'✓ Team approved {email}')
                except Exception as e: flash(f'✗ {e}', 3)
        elif key in (curses.KEY_ENTER, 10, 13) and items:
            if view == 'requests':
                it = items[sel]
                link = f"{SITE['alternate']}/claim.html?token={it['token']}"
                body = f"Access granted!\n\nLink: {link}\nToken: {it['token']}\n\n— {SITE['name']}"
                ok2, msg = send_email(it['email'], f"Access Granted to {SITE['name']}", body)
                flash(f"✓ Sent to {it['email']}" if ok2 else f"✗ {msg}", 2 if ok2 else 3)
            elif view == 'appeals':
                it = items[sel]; email = it.get('email', '')
                try:
                    toks = load_json(TOKENS_FILE, {}); token = secrets.token_hex(24)
                    toks[token] = {'email': email, 'created_at': time.time(), 'used': False}
                    save_json(TOKENS_FILE, toks)
                    new_appeals = [a for a in appeals if not (
                        a.get('email') == email and a.get('submitted_at') == it.get('submitted_at'))]
                    save_json(APPEALS_FILE, new_appeals)
                    link = f"{SITE['alternate']}/claim.html?token={token}"
                    send_email(email, f"Appeal Approved — {SITE['name']}",
                               f"Appeal approved!\n\nLink: {link}\n\n— {SITE['name']}")
                    flash(f'✓ Approved {email}')
                except Exception as e: flash(f'✗ {e}', 3)

# ══════════════════════════════════════════════════════════════════════════════
# 3. SESSIONS
# ══════════════════════════════════════════════════════════════════════════════

def sessions_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    sel = 0; off = 0; sub_off = 0; view = 'list'; show_ip = True

    while True:
        try: logs = load_json(SESS_LOG_FILE, [])
        except: logs = []
        names = load_json(NAMES_FILE, {})
        id_groups = defaultdict(list)
        for e in logs:
            uid = str(e.get('id', '?'))
            id_groups[names.get(uid, uid)].append(e)
        for g in id_groups.values(): g.sort(key=lambda x: x.get('timestamp', ''), reverse=True)
        display = sorted([u for u in id_groups if len(id_groups[u]) > 1],
                         key=lambda u: id_groups[u][0].get('timestamp', ''), reverse=True)

        h, w = stdscr.getmaxyx(); stdscr.erase()

        if view == 'list':
            lw = max(24, w // 3)
            sel = max(0, min(sel, len(display) - 1))
            if sel < off: off = sel
            if sel >= off + h - 2: off = sel - h + 3
            for i, gk in enumerate(display[off:off + h - 2]):
                abs_i = i + off
                count = len(id_groups[gk])
                attr = curses.color_pair(7) if abs_i == sel else 0
                safe_addstr(stdscr, i + 1, 0, f' {gk[:lw-12]:<20} ({count}) '.ljust(lw), attr)
            for r in range(h - 1):
                try: stdscr.addch(r, lw, '│')
                except: pass
            if display:
                uid = display[sel]; entries = id_groups[uid]
                safe_addstr(stdscr, 0, lw + 2, f' {uid} — {len(entries)} visits', curses.color_pair(5))
                for i, e in enumerate(entries[:h - 3]):
                    ts = fmt_iso(e.get('timestamp', ''))
                    page = e.get('page', '')
                    ip_str = f"[{e.get('ip','')}] " if show_ip else ''
                    safe_addstr(stdscr, i + 1, lw + 2, f'{ts}  {ip_str}{page}'[:w - lw - 4])
            safe_addstr(stdscr, h - 1, 0, ' ↑↓ nav  Enter full  i IP  q back', curses.color_pair(3))
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27): break
            elif key == curses.KEY_UP: sel = max(0, sel - 1)
            elif key == curses.KEY_DOWN: sel = min(len(display) - 1, sel + 1)
            elif key == ord('i'): show_ip = not show_ip
            elif key in (curses.KEY_ENTER, 10, 13, curses.KEY_RIGHT) and display: view = 'detail'; sub_off = 0
        else:
            uid = display[sel]; entries = id_groups[uid]
            safe_addstr(stdscr, 0, 0, f' Full History: {uid} '.center(w), curses.A_REVERSE)
            for i, e in enumerate(entries[sub_off:sub_off + h - 3]):
                ts = fmt_iso(e.get('timestamp', ''))
                ip_str = f"[{e.get('ip','')}] " if show_ip else ''
                safe_addstr(stdscr, i + 1, 2, f'{ts}  {ip_str}{e.get("page","")}'[:w - 4])
            safe_addstr(stdscr, h - 1, 0, ' ↑↓ scroll  i IP  q back', curses.color_pair(3))
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27, curses.KEY_LEFT): view = 'list'
            elif key == ord('i'): show_ip = not show_ip
            elif key == curses.KEY_UP: sub_off = max(0, sub_off - 1)
            elif key == curses.KEY_DOWN: sub_off = min(max(0, len(entries) - (h - 3)), sub_off + 1)

# ══════════════════════════════════════════════════════════════════════════════
# 4. ENCRYPTED DMs
# ══════════════════════════════════════════════════════════════════════════════

def encrypt_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    state = 'users'; sel = 0; off = 0; chosen_user = ''; scroll = 0

    while True:
        dms = load_json(DMS_FILE, [])
        if state == 'users':
            users = sorted(set(m.get('from', '') for m in dms) | set(m.get('to', '') for m in dms))
            users = [u for u in users if u]
            h, w = stdscr.getmaxyx(); stdscr.erase()
            safe_addstr(stdscr, 0, 0, ' DM VIEWER '.center(w), curses.A_REVERSE)
            for i, u in enumerate(users[off:off + h - 2]):
                abs_i = i + off
                attr = curses.color_pair(7) if abs_i == sel else 0
                safe_addstr(stdscr, i + 1, 0, f'  {u} '.ljust(w), attr)
            safe_addstr(stdscr, h - 1, 0, ' ↑↓ nav  Enter view  q back', curses.A_DIM)
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27): break
            elif key == curses.KEY_UP: sel = max(0, sel - 1)
            elif key == curses.KEY_DOWN: sel = min(len(users) - 1, sel + 1)
            elif key in (curses.KEY_ENTER, 10, 13) and users:
                chosen_user = users[sel]; state = 'convo'; sel = 0; scroll = 0
        else:
            msgs = sorted([m for m in dms if m.get('from') == chosen_user or m.get('to') == chosen_user],
                          key=lambda x: x.get('ts', 0))
            h, w = stdscr.getmaxyx(); stdscr.erase()
            safe_addstr(stdscr, 0, 0, f' {chosen_user} '.center(w), curses.A_REVERSE)
            lines = []
            for m in msgs:
                sender = m.get('from', '?')
                text = m.get('text', '[encrypted]')
                lines.extend(textwrap.wrap(f'{sender}: {text}', w - 4))
                lines.append('')
            max_scroll = max(0, len(lines) - (h - 3))
            scroll = max(0, min(scroll, max_scroll))
            for i, line in enumerate(lines[scroll:scroll + h - 3]):
                safe_addstr(stdscr, i + 1, 2, line[:w - 4])
            safe_addstr(stdscr, h - 1, 0, ' ↑↓ scroll  q back', curses.A_DIM)
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27): state = 'users'
            elif key == curses.KEY_UP: scroll = max(0, scroll - 1)
            elif key == curses.KEY_DOWN: scroll = min(max_scroll, scroll + 1)

# ══════════════════════════════════════════════════════════════════════════════
# 5. SUGGESTIONS
# ══════════════════════════════════════════════════════════════════════════════

def suggestions_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    sel = 0; off = 0; view = 'list'

    while True:
        names = load_json(NAMES_FILE, {})
        sugs = load_json(SUGGESTIONS_FILE, [])
        for s in sugs:
            s['_name'] = names.get(s.get('id', ''), s.get('id', '?')[:15])
        sugs.sort(key=lambda x: x.get('ts', 0), reverse=True)
        h, w = stdscr.getmaxyx()
        if view == 'list':
            stdscr.erase()
            draw_header(stdscr, f'Suggestions ({len(sugs)})')
            sel = max(0, min(sel, len(sugs) - 1))
            if sel < off: off = sel
            if sel >= off + h - 4: off = sel - (h - 5)
            for i, s in enumerate(sugs[off:off + h - 4]):
                abs_i = i + off
                attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
                dt = datetime.fromtimestamp(s.get('ts', 0), tz=LA).strftime('%m/%d %H:%M')
                line = f" {dt} {s.get('type','?'):<10} {s.get('_name','?'):<15} {s.get('text','')[:w-46]}"
                safe_addstr(stdscr, i + 2, 0, line.ljust(w), attr)
            draw_footer(stdscr, ' ↑↓ nav  Enter view  q back')
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27): break
            elif key == curses.KEY_UP: sel = max(0, sel - 1)
            elif key == curses.KEY_DOWN: sel = min(len(sugs) - 1, sel + 1)
            elif key in (curses.KEY_ENTER, 10, 13): view = 'detail'
        else:
            stdscr.erase(); s = sugs[sel]
            dt = datetime.fromtimestamp(s.get('ts', 0), tz=LA).strftime('%Y-%m-%d %H:%M')
            draw_header(stdscr, f"Suggestion from {s.get('_name','?')}")
            safe_addstr(stdscr, 2, 2, f"Date: {dt}"); safe_addstr(stdscr, 3, 2, f"Type: {s.get('type','?')}")
            safe_addstr(stdscr, 5, 2, 'Text:')
            for i, line in enumerate(textwrap.wrap(s.get('text', ''), w - 4)[:h - 8]):
                safe_addstr(stdscr, 6 + i, 4, line)
            draw_footer(stdscr, ' q/← back')
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27, curses.KEY_LEFT): view = 'list'

# ══════════════════════════════════════════════════════════════════════════════
# 6. CHEAT LOG VIEWER  (from admin_view.py)
# ══════════════════════════════════════════════════════════════════════════════

def cheat_log_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    sel = 0; off = 0; scroll = 0; view = 'list'

    def load():
        logs = load_json(CHEAT_LOGS_FILE, [])
        names = load_json(NAMES_FILE, {})
        for l in logs:
            l['_display'] = names.get(l.get('email', ''), '') or l.get('email', 'unknown')
        return sorted(logs, key=lambda x: x.get('ts', 0), reverse=True)

    while True:
        entries = load()
        h, w = stdscr.getmaxyx()

        if view == 'list':
            stdscr.erase()
            col_name = max(14, w // 4); col_game = 20; col_time = 14
            draw_header(stdscr, f'Cheat Log ({len(entries)} alerts)')
            stdscr.attron(curses.A_BOLD)
            hdr = 'EMAIL/NAME'.ljust(col_name) + 'GAME'.ljust(col_game) + 'TIME'.ljust(col_time) + 'DETAILS'
            safe_addstr(stdscr, 1, 0, hdr[:w])
            stdscr.attroff(curses.A_BOLD)
            if not entries:
                safe_addstr(stdscr, 3, 2, 'No cheat attempts logged. All clear!', curses.color_pair(2))
            list_h = h - 3
            sel = max(0, min(sel, len(entries) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            for row, e in enumerate(entries[off:off + list_h]):
                abs_i = row + off; is_sel = abs_i == sel
                attr = curses.color_pair(8) | curses.A_BOLD if is_sel else 0
                name_s = e['_display'][:col_name - 1].ljust(col_name)
                game_s = e.get('game', '')[:col_game - 1].ljust(col_game)
                time_s = fmt_ts(e.get('ts', 0), ms=True)[:col_time - 1].ljust(col_time)
                prev_w = max(0, w - col_name - col_game - col_time - 1)
                prev_s = e.get('details', '').replace('\n', ' ')[:prev_w]
                try:
                    stdscr.addstr(row + 2, 0, name_s, attr)
                    stdscr.addstr(row + 2, col_name, game_s,
                                  (curses.color_pair(8) if is_sel else curses.color_pair(3)) | curses.A_BOLD)
                    stdscr.addstr(row + 2, col_name + col_game, time_s, attr)
                    stdscr.addstr(row + 2, col_name + col_game + col_time, prev_s, attr)
                except: pass
            draw_footer(stdscr, f' ↑↓ nav  Enter details  E copy  q back — {len(entries)} alert(s)')
            stdscr.timeout(4000); stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27): break
            elif key == curses.KEY_UP: sel = max(0, sel - 1)
            elif key == curses.KEY_DOWN: sel = min(len(entries) - 1, sel + 1)
            elif key == ord('E') and entries:
                em = entries[sel].get('email', '')
                if em: osc52_copy(em)
            elif key in (curses.KEY_ENTER, 10, 13) and entries:
                view = 'detail'; scroll = 0
        else:
            e = entries[sel]
            h, w = stdscr.getmaxyx(); stdscr.erase()
            header = f" {e['_display']}  [{e.get('game','?')}]  {fmt_ts(e.get('ts',0), ms=True)} "
            safe_addstr(stdscr, 0, 0, header[:w - 1], curses.color_pair(5) | curses.A_BOLD)
            lines = []
            for para in e.get('details', '').split('\n'):
                lines.extend(textwrap.wrap(para, w - 4) or [''])
            visible_h = h - 3
            scroll = max(0, min(scroll, max(0, len(lines) - visible_h)))
            for row, line in enumerate(lines[scroll:scroll + visible_h]):
                safe_addstr(stdscr, row + 1, 2, line[:w - 3])
            draw_footer(stdscr, ' ↑↓/PgUp/PgDn scroll  q/← back')
            stdscr.refresh(); key = stdscr.getch()
            if key in (ord('q'), 27, curses.KEY_LEFT): view = 'list'
            elif key in (curses.KEY_UP, ord('k')): scroll = max(0, scroll - 1)
            elif key in (curses.KEY_DOWN, ord('j')): scroll = min(max(0, len(lines) - visible_h), scroll + 1)
            elif key == curses.KEY_PPAGE: scroll = max(0, scroll - visible_h)
            elif key == curses.KEY_NPAGE: scroll = min(max(0, len(lines) - visible_h), scroll + visible_h)

# ══════════════════════════════════════════════════════════════════════════════
# 7. ECONOMY  (gift, burn, multiplier, audit — from admin dashboard)
# ══════════════════════════════════════════════════════════════════════════════

def economy_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    coins = load_json(COINS_FILE, {}); stats = load_json(USER_STATS_FILE, {})
    sel = 0; off = 0; msg = ''; input_mode = None; input_buf = ''; input_ctx = {}

    leaderboard = sorted(coins.items(), key=lambda x: float(x[1] or 0), reverse=True)

    def draw():
        nonlocal sel, off
        h, w = stdscr.getmaxyx(); stdscr.erase()
        draw_header(stdscr, f'Economy — {len(coins)} accounts')
        # Summary row
        total = sum(float(v or 0) for v in coins.values())
        top5 = sum(float(v or 0) for _, v in leaderboard[:5])
        safe_addstr(stdscr, 1, 2, f'Total supply: {fmt_coins(total)}   Top-5 hold: {fmt_coins(top5)} ({100*top5/total:.1f}% of supply)' if total else '', curses.color_pair(3))
        # Header row
        safe_addstr(stdscr, 2, 0, f"  {'#':<4} {'EMAIL':<42} {'COINS':>10}  LIFETIME", curses.A_DIM)
        list_h = h - 7
        sel = max(0, min(sel, len(leaderboard) - 1))
        if sel < off: off = sel
        if sel >= off + list_h: off = sel - list_h + 1
        for i, (norm, bal) in enumerate(leaderboard[off:off + list_h]):
            abs_i = i + off
            attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
            life = stats.get(norm, {}).get('lifetime_earned', 0)
            line = f"  {abs_i+1:<4} {norm:<42} {fmt_coins(float(bal or 0)):>10}  {fmt_coins(life)}"
            safe_addstr(stdscr, 3 + i, 0, line.ljust(w), attr)
        if msg:
            safe_addstr(stdscr, h - 2, 2, msg, curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
        if input_mode:
            safe_addstr(stdscr, h - 2, 2, f'{input_mode}: {input_buf}_', curses.color_pair(3) | curses.A_BOLD)
        draw_footer(stdscr, ' ↑↓ nav  g gift  b burn  m multiplier  a audit  E copy  q back')
        stdscr.refresh()

    while True:
        leaderboard = sorted(load_json(COINS_FILE, {}).items(),
                             key=lambda x: float(x[1] or 0), reverse=True)
        draw()
        key = stdscr.getch()
        if input_mode:
            if key in (curses.KEY_BACKSPACE, 127): input_buf = input_buf[:-1]
            elif key in (curses.KEY_ENTER, 10, 13):
                if input_mode.startswith('Gift'):
                    amt_str = input_buf.strip(); input_mode = None; input_buf = ''
                    try:
                        amt = float(amt_str)
                        target = input_ctx.get('email', '')
                        d, err = call_admin_api('/api/admin/gift-coins', {'email': target, 'amount': amt, 'reason': 'CLI admin gift'})
                        msg = f'✓ Gifted {fmt_coins(amt)} to {target}' if d and d.get('success') else f'✗ {err or d}'
                    except Exception as e: msg = f'✗ {e}'
                elif input_mode.startswith('Burn'):
                    amt_str = input_buf.strip(); input_mode = None; input_buf = ''
                    try:
                        amt = float(amt_str)
                        target = input_ctx.get('email', '')
                        d, err = call_admin_api('/api/admin/economy/burn', {'email': target, 'amount': amt})
                        msg = f'✓ Burned {fmt_coins(amt)} from {target}' if d and d.get('success') else f'✗ {err or d}'
                    except Exception as e: msg = f'✗ {e}'
                elif input_mode.startswith('Multiplier'):
                    mult_str = input_buf.strip(); input_mode = None; input_buf = ''
                    try:
                        mult = float(mult_str)
                        d, err = call_admin_api('/api/admin/economy/multiplier', {'multiplier': mult})
                        msg = f'✓ Multiplier set to {mult}x' if d and d.get('success') else f'✗ {err or d}'
                    except Exception as e: msg = f'✗ {e}'
                else: input_mode = None; input_buf = ''
            elif 32 <= key <= 126: input_buf += chr(key)
            elif key == 27: input_mode = None; input_buf = ''
            continue
        if key in (ord('q'), 27): break
        elif key == curses.KEY_UP: sel = max(0, sel - 1)
        elif key == curses.KEY_DOWN: sel = min(len(leaderboard) - 1, sel + 1)
        elif key == ord('E') and leaderboard:
            em = leaderboard[sel][0]
            if osc52_copy(em): msg = f'Copied {em}'
        elif key == ord('g') and leaderboard:
            em = leaderboard[sel][0]
            input_mode = f'Gift coins to {em[:30]}'; input_buf = ''; input_ctx = {'email': em}
        elif key == ord('b') and leaderboard:
            em = leaderboard[sel][0]
            if confirm(stdscr, f'Burn coins from {em}?'):
                input_mode = f'Burn amount from {em[:28]}'; input_buf = ''; input_ctx = {'email': em}
        elif key == ord('m'):
            input_mode = 'Multiplier (e.g. 1.5)'; input_buf = ''; input_ctx = {}
        elif key == ord('a'):
            d, err = call_admin_api('/api/admin/economy/audit', method='GET')
            if d: msg = f"✓ Audit: {d.get('total_accounts',0)} accts, supply {fmt_coins(d.get('total_coins',0))}"
            else: msg = f'✗ {err}'

# ══════════════════════════════════════════════════════════════════════════════
# 8. MODERATION  (ban/unban/shadow-ban, premium grant/revoke)
# ══════════════════════════════════════════════════════════════════════════════

def moderation_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    msg = ''; input_mode = None; input_buf = ''; input_ctx = {}
    tabs = ['Users', 'Banned', 'Notifications']; tab = 0

    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        # Tab bar
        x = 0
        for i, t in enumerate(tabs):
            attr = curses.color_pair(7) | curses.A_BOLD if i == tab else curses.color_pair(3)
            safe_addstr(stdscr, 0, x, f' {i+1}:{t} ', attr); x += len(t) + 4

        draw_header = lambda title: safe_addstr(stdscr, 1, 2, title, curses.color_pair(5) | curses.A_BOLD)

        if tab == 0:  # User actions
            draw_header('Enter email below, then choose action')
            safe_addstr(stdscr, 3, 2, 'Email: ')
            safe_addstr(stdscr, 3, 9, input_buf + ('_' if input_mode == 'email' else ''),
                        curses.color_pair(3) | curses.A_BOLD)
            acts = [
                ('[P] Grant Premium', 'grant'),
                ('[V] Revoke Premium', 'revoke'),
                ('[B] Ban Account', 'ban'),
                ('[U] Unban Account', 'unban'),
                ('[S] Shadow-ban', 'shadow'),
            ]
            for i, (label, _) in enumerate(acts):
                safe_addstr(stdscr, 5 + i, 4, label, curses.color_pair(1))
        elif tab == 1:  # Banned list
            draw_header('Banned Accounts')
            banned = load_json(BLACKLIST_FILE, {})
            items = list(banned.items())
            for i, (norm, info) in enumerate(items[:h - 5]):
                ts = info.get('blacklisted_at', 0)
                line = f"  {norm:<42} {fmt_ts(ts)}  {info.get('reason','')[:30]}"
                safe_addstr(stdscr, 2 + i, 0, line[:w])
        else:  # Notifications
            draw_header('Send Notification to All Users')
            safe_addstr(stdscr, 3, 2, 'Title:')
            safe_addstr(stdscr, 4, 4, input_ctx.get('title', '') + ('_' if input_mode == 'title' else ''), curses.color_pair(3))
            safe_addstr(stdscr, 5, 2, 'Body:')
            safe_addstr(stdscr, 6, 4, input_ctx.get('body', '') + ('_' if input_mode == 'body' else ''), curses.color_pair(3))
            safe_addstr(stdscr, 8, 2, '[T] Set title  [O] Set body  [Enter] Send broadcast', curses.color_pair(1))

        if msg:
            safe_addstr(stdscr, h - 2, 2, msg,
                        curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
        safe_addstr(stdscr, h - 1, 0, ' 1-3 tabs  [e] set email  q back  (mod actions call server API)', curses.color_pair(3) | curses.A_DIM)
        stdscr.refresh()

        key = stdscr.getch()
        if input_mode == 'email':
            if key in (curses.KEY_BACKSPACE, 127): input_buf = input_buf[:-1]
            elif key in (curses.KEY_ENTER, 10, 13): input_mode = None
            elif key == 27: input_mode = None; input_buf = ''
            elif 32 <= key <= 126: input_buf += chr(key)
            continue
        if input_mode in ('title', 'body'):
            field = input_mode
            if key in (curses.KEY_BACKSPACE, 127): input_ctx[field] = input_ctx.get(field, '')[:-1]
            elif key in (curses.KEY_ENTER, 10, 13): input_mode = None
            elif key == 27: input_mode = None
            elif 32 <= key <= 126: input_ctx[field] = input_ctx.get(field, '') + chr(key)
            continue

        if key in (ord('q'), 27): break
        elif ord('1') <= key <= ord('3'): tab = key - ord('1')
        elif key == ord('e') or key == ord('E'): input_mode = 'email'; input_buf = ''

        if tab == 0:
            em = input_buf.strip()
            if key == ord('P') and em:
                if confirm(stdscr, f'Grant premium to {em}?'):
                    d, err = call_admin_api('/api/admin/grant-premium', {'email': em, 'reason': 'CLI grant'})
                    msg = f'✓ Premium granted to {em}' if d and d.get('success') else f'✗ {err or d}'
            elif key == ord('V') and em:
                if confirm(stdscr, f'Revoke premium from {em}?'):
                    d, err = call_admin_api('/api/admin/revoke-premium', {'email': em})
                    msg = f'✓ Premium revoked from {em}' if d and d.get('success') else f'✗ {err or d}'
            elif key == ord('B') and em:
                if confirm(stdscr, f'BAN {em}?'):
                    d, err = call_admin_api('/api/admin/ban-account', {'email': em, 'reason': 'CLI ban'})
                    msg = f'✓ Banned {em}' if d and d.get('success') else f'✗ {err or d}'
            elif key == ord('U') and em:
                d, err = call_admin_api('/api/admin/unban-account', {'email': em})
                msg = f'✓ Unbanned {em}' if d and d.get('success') else f'✗ {err or d}'
            elif key == ord('S') and em:
                if confirm(stdscr, f'Shadow-ban {em}?'):
                    d, err = call_admin_api('/api/admin/restricted-mode', {'email': em, 'enabled': True})
                    msg = f'✓ Shadow-banned {em}' if d and d.get('success') else f'✗ {err or d}'
        elif tab == 2:
            if key == ord('T') or key == ord('t'): input_mode = 'title'
            elif key == ord('O') or key == ord('o'): input_mode = 'body'
            elif key in (curses.KEY_ENTER, 10, 13):
                title = input_ctx.get('title', '').strip()
                body = input_ctx.get('body', '').strip()
                if title and body and confirm(stdscr, f'Broadcast "{title}" to all users?'):
                    d, err = call_admin_api('/api/admin/broadcast', {'title': title, 'body': body})
                    msg = f'✓ Broadcast sent' if d and d.get('success') else f'✗ {err or d}'
                    input_ctx = {}
                elif not title or not body: msg = '✗ Both title and body required'

# ══════════════════════════════════════════════════════════════════════════════
# 9. CASINO  (RTP stats, rig chance, toggle)
# ══════════════════════════════════════════════════════════════════════════════

def casino_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    msg = ''; input_mode = None; input_buf = ''

    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        draw_header(stdscr, 'Casino Admin')
        safe_addstr(stdscr, 2, 2, '[r] Refresh RTP Stats', curses.color_pair(5))
        safe_addstr(stdscr, 3, 2, '[t] Toggle Casino on/off', curses.color_pair(5))
        safe_addstr(stdscr, 4, 2, '[c] Set Rig Chance (0.0–1.0)', curses.color_pair(5))
        if msg:
            for i, line in enumerate(msg.splitlines()[:h - 8]):
                safe_addstr(stdscr, 6 + i, 2, line[:w - 4],
                            curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
        if input_mode:
            safe_addstr(stdscr, h - 2, 2, f'{input_mode}: {input_buf}_', curses.color_pair(3) | curses.A_BOLD)
        draw_footer(stdscr, ' r refresh  t toggle  c rig chance  q back')
        stdscr.refresh(); key = stdscr.getch()
        if input_mode:
            if key in (curses.KEY_BACKSPACE, 127): input_buf = input_buf[:-1]
            elif key in (curses.KEY_ENTER, 10, 13):
                try:
                    chance = float(input_buf)
                    d, err = call_admin_api('/api/admin/casino/rig', {'chance': chance})
                    msg = f'✓ Rig chance set to {chance}' if d and d.get('success') else f'✗ {err or d}'
                except Exception as e: msg = f'✗ {e}'
                input_mode = None; input_buf = ''
            elif key == 27: input_mode = None; input_buf = ''
            elif 32 <= key <= 126: input_buf += chr(key)
            continue
        if key in (ord('q'), 27): break
        elif key == ord('r'):
            d, err = call_admin_api('/api/admin/casino/rtp', method='GET')
            if d: msg = '\n'.join(f"  {k}: {v}" for k, v in d.items() if k != 'success')
            else: msg = f'✗ {err}'
        elif key == ord('t'):
            if confirm(stdscr, 'Toggle casino?'):
                d, err = call_admin_api('/api/admin/casino/toggle', {})
                msg = f'✓ Casino toggled: {d}' if d else f'✗ {err}'
        elif key == ord('c'):
            input_mode = 'Rig chance (0.0–1.0)'; input_buf = ''

# ══════════════════════════════════════════════════════════════════════════════
# 10. INVITES / REFERRALS
# ══════════════════════════════════════════════════════════════════════════════

def invites_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    sel = 0; off = 0

    while True:
        codes = load_json(INVITE_CODES_FILE, {})
        claims = load_json(INVITE_CLAIMS_FILE, {})
        # Count signups per referrer
        ref_counts = Counter(c.get('refNorm', '') for c in claims.values() if c.get('paid'))
        leaderboard = sorted([(norm, code, ref_counts.get(norm, 0)) for norm, code in codes.items()],
                             key=lambda x: -x[2])
        h, w = stdscr.getmaxyx(); stdscr.erase()
        draw_header(stdscr, f'Invite System — {len(codes)} codes  {len(claims)} total signups')
        safe_addstr(stdscr, 1, 2, f"{'EMAIL':<44} {'CODE':<22} SIGNUPS  COINS EARNED", curses.A_DIM)
        list_h = h - 4
        sel = max(0, min(sel, len(leaderboard) - 1))
        if sel < off: off = sel
        if sel >= off + list_h: off = sel - list_h + 1
        for i, (norm, code, count) in enumerate(leaderboard[off:off + list_h]):
            abs_i = i + off
            attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
            line = f"  {norm:<44} {code:<22} {count:>7}  {fmt_coins(count * 2000):>12}"
            safe_addstr(stdscr, 2 + i, 0, line.ljust(w), attr)
        draw_footer(stdscr, ' ↑↓ nav  E copy email  q back')
        stdscr.refresh(); key = stdscr.getch()
        if key in (ord('q'), 27): break
        elif key == curses.KEY_UP: sel = max(0, sel - 1)
        elif key == curses.KEY_DOWN: sel = min(len(leaderboard) - 1, sel + 1)
        elif key == ord('E') and leaderboard:
            em = leaderboard[sel][0]
            if osc52_copy(em): pass

# ══════════════════════════════════════════════════════════════════════════════
# 11. LIVE DASHBOARD  (from dashboard.py, inline curses version)
# ══════════════════════════════════════════════════════════════════════════════

def dashboard_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    stdscr.timeout(10000)  # Auto-refresh every 10 s

    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        now = time.time()
        tokens = load_json(DATA / 'tokens.json', {})
        sess_logs = load_json(SESS_LOG_FILE, [])
        names = load_json(NAMES_FILE, {})
        coins = load_json(COINS_FILE, {})
        sugs = load_json(SUGGESTIONS_FILE, [])
        appeals = load_json(APPEALS_FILE, [])
        bl = load_json(BLACKLIST_FILE, {})
        cheats = load_json(CHEAT_LOGS_FILE, [])
        invite_claims = load_json(INVITE_CLAIMS_FILE, {})
        push_subs = load_json(PUSH_SUBS_FILE, {})

        # Activity
        latest_by_uid = {}
        for e in sess_logs:
            uid = str(e.get('id', ''))
            ts_str = e.get('timestamp', '')
            try:
                ts = datetime.fromisoformat(ts_str.replace('Z', '+00:00')).timestamp()
            except: continue
            if uid not in latest_by_uid or ts > latest_by_uid[uid][0]:
                latest_by_uid[uid] = (ts, e.get('page', ''))
        active_5m = [(uid, ts, pg) for uid, (ts, pg) in latest_by_uid.items() if now - ts < 300]
        active_1h = [(uid, ts, pg) for uid, (ts, pg) in latest_by_uid.items() if now - ts < 3600]

        today0 = datetime.now(tz=LA).replace(hour=0, minute=0, second=0, microsecond=0).timestamp()
        signups_today = sum(1 for d in tokens.values() if d.get('created_at', 0) >= today0)
        pending = sum(1 for d in tokens.values() if not d.get('used') and not d.get('claimed_domains'))
        total_coins = sum(float(v or 0) for v in coins.values())

        row = 0
        def prow(label, value, cp=2):
            nonlocal row
            if row >= h - 1: return
            safe_addstr(stdscr, row, 2, f'{label:<28}', curses.A_DIM)
            safe_addstr(stdscr, row, 30, str(value)[:w - 32], curses.color_pair(cp) | curses.A_BOLD)
            row += 1

        def section(title):
            nonlocal row
            if row >= h - 1: return
            safe_addstr(stdscr, row, 0, f' {title} ', curses.color_pair(5) | curses.A_BOLD)
            row += 1

        safe_addstr(stdscr, row, 0,
                    f" mitch.pro LIVE DASHBOARD  {datetime.now(tz=LA).strftime('%a %b %d %I:%M %p')} ".center(w),
                    curses.color_pair(6) | curses.A_BOLD); row += 1

        section('Active Users')
        prow('Online now (5 min)', len(active_5m), 2 if active_5m else 1)
        prow('Active last hour', len(active_1h), 3)
        prow('Total unique users', len(latest_by_uid), 5)
        for uid, ts, pg in sorted(active_5m, key=lambda x: -x[1])[:4]:
            if row >= h - 4: break
            name = names.get(uid, uid[:16])
            page = pg.replace('https://mitch.pro', '').replace('https://mitch.88chan.me', '') or '/'
            safe_addstr(stdscr, row, 4, f'● {name:<22} {page[:40]:<40} {ago(ts)}'[:w - 5]); row += 1

        if row < h - 1: row += 1
        section('Signups & Economy')
        prow('Signups today', signups_today, 2 if signups_today else 1)
        prow('Total accounts', len([d for d in tokens.values() if not d.get('infinite')]), 5)
        prow('Pending approval', pending, 4 if pending else 1)
        prow('MitchCoin supply', fmt_coins(total_coins), 3)
        prow('Invite signups', len(invite_claims), 5)

        if row < h - 1: row += 1
        section('Misc')
        prow('Cheat alerts', len(cheats), 4 if cheats else 1)
        prow('Suggestions', len(sugs), 5)
        prow('Appeals', len(appeals), 3 if appeals else 1)
        prow('Blacklisted', len(bl), 4 if bl else 1)
        prow('Push subscribers', len(push_subs), 5)

        draw_footer(stdscr, ' Auto-refreshes every 10s  q to exit')
        stdscr.refresh(); key = stdscr.getch()
        if key in (ord('q'), 27): break

# ══════════════════════════════════════════════════════════════════════════════
# 12. PROXY & TRAFFIC MANAGER
# ══════════════════════════════════════════════════════════════════════════════

def proxy_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    msg = ''; input_mode = None; input_buf = ''; tab = 0
    sel = 0; off = 0

    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        tabs = ['Blocklist', 'Active Sessions']
        x = 0
        for i, t in enumerate(tabs):
            attr = curses.color_pair(7) | curses.A_BOLD if i == tab else curses.color_pair(3)
            safe_addstr(stdscr, 0, x, f' {i+1}:{t} ', attr)
            x += len(t) + 4

        draw_header = lambda win, title: safe_addstr(win, 1, 2, title, curses.color_pair(5) | curses.A_BOLD)
        list_h = h - 5

        if tab == 0:  # Blocklist
            blocklist = prox_blocklist_domains()
            blocklist.sort()
            
            draw_header(stdscr, f'Proxy Domain Blocklist ({len(blocklist)} domains)  [a] Block new domain')
            safe_addstr(stdscr, 2, 2, f"{'BLOCKED DOMAIN':<40}", curses.A_DIM)
            
            sel = max(0, min(sel, len(blocklist) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            
            for i, dom in enumerate(blocklist[off:off + list_h]):
                abs_i = i + off
                attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
                line = f"  {dom:<40}  [Press Backspace/Delete to Unblock]"
                safe_addstr(stdscr, 3 + i, 0, line.ljust(w), attr)
                
        elif tab == 1:  # Active sessions
            sess, err = call_admin_api('/api/admin/prox/sessions', method='GET')
            sessions_list = []
            if sess and isinstance(sess, dict) and 'sessions' in sess:
                sessions_list = sess['sessions']
            elif sess and isinstance(sess, list):
                sessions_list = sess
            
            draw_header(stdscr, f'Active Proxy Sessions ({len(sessions_list)})  r refresh')
            safe_addstr(stdscr, 2, 2, f"{'USER/IP':<30} {'TARGET':<40} {'BYTES':<12}", curses.A_DIM)
            
            sel = max(0, min(sel, len(sessions_list) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            
            for i, s in enumerate(sessions_list[off:off + list_h]):
                abs_i = i + off
                attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
                user = s.get('user', s.get('ip', 'unknown'))
                target = s.get('target', 'unknown')
                bytes_sent = fmt_coins(s.get('bytes', 0))
                line = f"  {user:<30} {target[:40]:<40} {bytes_sent:>12}"
                safe_addstr(stdscr, 3 + i, 0, line.ljust(w), attr)

        if msg:
            safe_addstr(stdscr, h - 2, 2, msg,
                        curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
        
        safe_addstr(stdscr, h - 1, 0, ' 1-2 tabs  ↑↓ nav  q back', curses.color_pair(3) | curses.A_DIM)
        if input_mode:
            safe_addstr(stdscr, h - 2, 2, f'Enter {input_mode}: {input_buf}_', curses.color_pair(3) | curses.A_BOLD)
            
        stdscr.refresh(); key = stdscr.getch()
        
        if input_mode == 'Block Domain':
            if key in (curses.KEY_BACKSPACE, 127): input_buf = input_buf[:-1]
            elif key in (curses.KEY_ENTER, 10, 13):
                dom = input_buf.strip().lower()
                if dom:
                    d, err = call_admin_api('/api/admin/prox/block', {'domain': dom})
                    msg = f'✓ Blocked: {dom}' if d and d.get('ok') else f'✗ {err or d}'
                input_mode = None; input_buf = ''
            elif key == 27: input_mode = None; input_buf = ''
            elif 32 <= key <= 126: input_buf += chr(key)
            continue
            
        if key in (ord('q'), 27): break
        elif ord('1') <= key <= ord('2'): tab = key - ord('1'); sel = 0; off = 0; msg = ''
        elif key == curses.KEY_UP: sel = max(0, sel - 1)
        elif key == curses.KEY_DOWN: sel = min(sel + 1, (len(blocklist) if tab == 0 else len(sessions_list)) - 1)
        elif key == ord('r') and tab == 1: msg = '✓ Refreshed active sessions.'
        elif key == ord('a') and tab == 0:
            input_mode = 'Block Domain'; input_buf = ''
        elif key in (curses.KEY_BACKSPACE, 127, 330) and tab == 0 and blocklist:
            dom = blocklist[sel]
            if confirm(stdscr, f"Unblock domain {dom}?"):
                d, err = call_admin_api('/api/admin/prox/unblock', {'domain': dom})
                msg = f'✓ Unblocked {dom}' if d and d.get('ok') else f'✗ {err or d}'

# ══════════════════════════════════════════════════════════════════════════════
# 13. SHOP CATALOG & COSMETIC EDITOR
# ══════════════════════════════════════════════════════════════════════════════

def shop_catalog_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    msg = ''; input_mode = None; input_buf = ''; tab = 0
    sel = 0; off = 0; catalog = []
    custom_item = {}

    def fetch_catalog():
        nonlocal catalog
        d, err = call_admin_api('/api/admin/shop/catalog', method='GET')
        if d and d.get('success'):
            catalog = d.get('catalog', [])
        else:
            catalog = []
            return f"✗ Failed to load catalog: {err or d}"
        return ''

    err = fetch_catalog()
    if err: msg = err

    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        tabs = ['Catalog List', 'Add Custom Cosmetic']
        x = 0
        for i, t in enumerate(tabs):
            attr = curses.color_pair(7) | curses.A_BOLD if i == tab else curses.color_pair(3)
            safe_addstr(stdscr, 0, x, f' {i+1}:{t} ', attr)
            x += len(t) + 4

        draw_header = lambda win, title: safe_addstr(win, 1, 2, title, curses.color_pair(5) | curses.A_BOLD)
        list_h = h - 5

        if tab == 0:  # Catalog List
            draw_header(stdscr, f'Shop Items ({len(catalog)})  [Backspace/Del] Delete item  [s] Save catalog  [r] Reset to defaults')
            safe_addstr(stdscr, 2, 2, f"{'ID':<15} {'NAME':<20} {'SECTION':<15} {'TYPE':<10} {'COST':<8} {'PREMIUM/ADMIN':<12}", curses.A_DIM)
            
            sel = max(0, min(sel, len(catalog) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            
            for i, item in enumerate(catalog[off:off + list_h]):
                abs_i = i + off
                attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
                badges = []
                if item.get('premiumOnly'): badges.append('PREM')
                if item.get('adminOnly'): badges.append('ADMIN')
                badge_str = '/'.join(badges) if badges else 'NONE'
                cost = item.get('cost', 0)
                cost_type = item.get('costType')
                type_str = item.get('type', '') + (f"({cost_type})" if cost_type else "")
                line = f"  {item.get('id',''):<15} {item.get('name',''):<20} {item.get('section',''):<15} {type_str:<10} {cost:<8} {badge_str:<12}"
                safe_addstr(stdscr, 3 + i, 0, line.ljust(w), attr)
                
        elif tab == 1:  # Add Custom Cosmetic
            draw_header(stdscr, 'Create Custom Shop Item')
            fields = [
                ('1. ID (alphanumeric, no spaces)', 'id'),
                ('2. Name', 'name'),
                ('3. Section (Name Colors, Badges, Chat Effects, Profile Effects, Site Themes, Passes)', 'section'),
                ('4. Cost (MitchCoins)', 'cost'),
                ('5. Cost Type (name_color, chat_badge, chat_effect, profile_effect, site_theme, canvas_tool)', 'costType'),
                ('6. Description', 'desc'),
                ('7. Premium Only? (y/n)', 'premiumOnly'),
                ('8. Admin Only? (y/n)', 'adminOnly'),
            ]
            for i, (label, key) in enumerate(fields):
                val = str(custom_item.get(key, ''))
                safe_addstr(stdscr, 3 + i*2, 4, f"{label:<30}", curses.A_DIM)
                safe_addstr(stdscr, 3 + i*2, 35, val, curses.color_pair(2) if val else curses.color_pair(4))
                
            safe_addstr(stdscr, 3 + len(fields)*2 + 1, 4, '[A] Create & Insert Item into Catalog', curses.color_pair(5) | curses.A_BOLD)

        if msg:
            safe_addstr(stdscr, h - 2, 2, msg,
                        curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
        
        safe_addstr(stdscr, h - 1, 0, ' 1-2 tabs  q back', curses.color_pair(3) | curses.A_DIM)
        if input_mode:
            safe_addstr(stdscr, h - 2, 2, f'Set {input_mode}: {input_buf}_', curses.color_pair(3) | curses.A_BOLD)
            
        stdscr.refresh(); key = stdscr.getch()
        
        if input_mode:
            if key in (curses.KEY_BACKSPACE, 127): input_buf = input_buf[:-1]
            elif key in (curses.KEY_ENTER, 10, 13):
                custom_item[input_mode] = input_buf.strip()
                input_mode = None; input_buf = ''
            elif key == 27: input_mode = None; input_buf = ''
            elif 32 <= key <= 126: input_buf += chr(key)
            continue

        if key in (ord('q'), 27): break
        elif ord('1') <= key <= ord('2'): tab = key - ord('1'); sel = 0; off = 0; msg = ''
        elif key == curses.KEY_UP and tab == 0: sel = max(0, sel - 1)
        elif key == curses.KEY_DOWN and tab == 0: sel = min(sel + 1, len(catalog) - 1)
        elif key == ord('r') and tab == 0:
            if confirm(stdscr, "Reset catalog to defaults? This will erase all server overrides."):
                d, err = call_admin_api('/api/admin/shop/catalog/save', {'catalog': []})
                if d and d.get('success'):
                    msg = '✓ Catalog reset successfully.'
                    fetch_catalog()
                else: msg = f'✗ Reset failed: {err or d}'
        elif key == ord('s') and tab == 0:
            if confirm(stdscr, f"Save {len(catalog)} catalog items to server?"):
                d, err = call_admin_api('/api/admin/shop/catalog/save', {'catalog': catalog})
                if d and d.get('success'):
                    msg = '✓ Catalog saved successfully!'
                    fetch_catalog()
                else: msg = f'✗ Save failed: {err or d}'
        elif key in (curses.KEY_BACKSPACE, 127, 330) and tab == 0 and catalog:
            item = catalog[sel]
            if confirm(stdscr, f"Delete item {item.get('id')} from local catalog?"):
                catalog.pop(sel)
                msg = f"✓ Deleted {item.get('id')} locally. Press 's' to save."
        elif tab == 1 and ord('1') <= key <= ord('8'):
            idx = key - ord('1')
            keys = ['id', 'name', 'section', 'cost', 'costType', 'desc', 'premiumOnly', 'adminOnly']
            field_name = keys[idx]
            input_mode = field_name
            input_buf = str(custom_item.get(field_name, ''))
        elif tab == 1 and key in (ord('a'), ord('A')):
            cid = custom_item.get('id')
            cname = custom_item.get('name')
            csection = custom_item.get('section')
            ccost = custom_item.get('cost')
            cdesc = custom_item.get('desc')
            
            if not cid or not cname or not cdesc:
                msg = '✗ ID, Name, and Description are required!'
                continue
            
            try: cost_val = int(ccost or 0)
            except: cost_val = 0
            
            prem = str(custom_item.get('premiumOnly','')).lower() in ('y', 'yes', 'true', '1')
            adm = str(custom_item.get('adminOnly','')).lower() in ('y', 'yes', 'true', '1')
            
            new_item = {
                'id': cid,
                'name': cname,
                'section': csection or 'Badges',
                'type': 'cosmetic',
                'costType': custom_item.get('costType') or 'chat_badge',
                'cost': cost_val,
                'desc': cdesc,
            }
            if prem: new_item['premiumOnly'] = True
            if adm: new_item['adminOnly'] = True
            
            if any(item.get('id') == cid for item in catalog):
                msg = f'✗ Item with ID {cid} already exists in catalog!'
            else:
                catalog.append(new_item)
                custom_item = {}
                msg = f"✓ Inserted {cid} locally! Press 's' on Tab 1 to save."
                tab = 0

# ══════════════════════════════════════════════════════════════════════════════
# 14. SOFT MAINTENANCE & SYSTEM CONTROLS
# ══════════════════════════════════════════════════════════════════════════════

def system_control_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    msg = ''; input_mode = None; input_buf = ''
    
    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        draw_header(stdscr, 'Soft Maintenance & System Controls')
        
        d, err = call_admin_api('/api/admin/maintenance-status', method='GET')
        maint_status = 'Active (Offline)' if d and d.get('active') else 'Inactive (Online)'
        maint_cp = 4 if d and d.get('active') else 2
        
        safe_addstr(stdscr, 3, 4, 'System Status:', curses.A_DIM)
        safe_addstr(stdscr, 3, 20, maint_status, curses.color_pair(maint_cp) | curses.A_BOLD)
        
        safe_addstr(stdscr, 5, 4, '[m] Toggle Soft Maintenance Mode', curses.color_pair(5))
        safe_addstr(stdscr, 6, 4, '[g] Set Featured Game (Spotlight)', curses.color_pair(5))
        safe_addstr(stdscr, 7, 4, '[u] Set Mitch.pro Alternate Mirror URL', curses.color_pair(5))
        
        if msg:
            for i, line in enumerate(msg.splitlines()[:h - 10]):
                safe_addstr(stdscr, 9 + i, 4, line[:w - 8],
                            curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
                            
        if input_mode:
            safe_addstr(stdscr, h - 2, 2, f'Enter new {input_mode}: {input_buf}_', curses.color_pair(3) | curses.A_BOLD)
            
        draw_footer(stdscr, ' m toggle maintenance  g set featured game  u set mirror URL  q back')
        stdscr.refresh(); key = stdscr.getch()
        
        if input_mode:
            if key in (curses.KEY_BACKSPACE, 127): input_buf = input_buf[:-1]
            elif key in (curses.KEY_ENTER, 10, 13):
                val = input_buf.strip()
                if input_mode == 'Featured Game':
                    d, err = call_admin_api('/api/admin/content/featured', {'href': val})
                    msg = f'✓ Featured game set to: {val}' if d and d.get('ok') else f'✗ {err or d}'
                elif input_mode == 'Mirror URL':
                    d, err = call_admin_api('/api/admin/content/mirror', {'url': val})
                    msg = f'✓ Alternate mirror set to: {val}' if d and d.get('ok') else f'✗ {err or d}'
                input_mode = None; input_buf = ''
            elif key == 27: input_mode = None; input_buf = ''
            elif 32 <= key <= 126: input_buf += chr(key)
            continue
            
        if key in (ord('q'), 27): break
        elif key == ord('m'):
            is_active = d.get('active') if d else False
            if confirm(stdscr, f"{'Deactivate' if is_active else 'Activate'} soft maintenance mode?"):
                res, err = call_admin_api('/api/admin/maintenance-toggle', {'active': not is_active})
                msg = f"✓ Soft maintenance mode {'activated' if res and res.get('active') else 'deactivated'}!" if res and res.get('success') else f'✗ {err or res}'
        elif key == ord('g'):
            input_mode = 'Featured Game'; input_buf = ''
        elif key == ord('u'):
            input_mode = 'Mirror URL'; input_buf = ''

# ══════════════════════════════════════════════════════════════════════════════
# 15. SAFETY REPORTS MODERATOR
# ══════════════════════════════════════════════════════════════════════════════

def safety_reports_tool(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    msg = ''; sel = 0; off = 0; reports = []
    view = 'list'

    def fetch_reports():
        nonlocal reports
        d, err = call_admin_api('/api/admin/advanced-data', method='GET')
        if d and 'reports' in d:
            reports = d['reports']
        elif d and 'chatReports' in d:
            reports = d['chatReports']
        else:
            reports = []
            return f"✗ Failed to load safety reports: {err or d}"
        return ''

    err = fetch_reports()
    if err: msg = err

    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        list_h = h - 4
        
        if view == 'list':
            draw_header(stdscr, f'Chat Safety Triage ({len(reports)} reports)  r refresh')
            safe_addstr(stdscr, 1, 2, f"{'DATE/TIME':<15} {'REPORTED BY':<30} {'REASON':<30} {'STATUS':<15}", curses.A_DIM)
            
            sel = max(0, min(sel, len(reports) - 1))
            if sel < off: off = sel
            if sel >= off + list_h: off = sel - list_h + 1
            
            for i, r in enumerate(reports[off:off + list_h]):
                abs_i = i + off
                attr = curses.color_pair(7) | curses.A_BOLD if abs_i == sel else 0
                ts = r.get('ts', 0)
                by = r.get('reportedBy', r.get('reporter', 'unknown'))
                reason = r.get('reason', 'Chat report')
                status = r.get('status', 'Needs review')
                
                status_cp = 4 if status == 'Needs review' else 2
                status_attr = attr | curses.color_pair(status_cp)
                
                line = f"  {fmt_ts(ts):<15} {by:<30} {reason[:30]:<30} "
                safe_addstr(stdscr, 2 + i, 0, line.ljust(w), attr)
                safe_addstr(stdscr, 2 + i, 78, status[:15], status_attr)
                
            draw_footer(stdscr, ' ↑↓ nav  Enter view detail  r refresh  q back')
            
        elif view == 'detail':
            r = reports[sel]
            draw_header(stdscr, 'Safety Report Details')
            
            safe_addstr(stdscr, 2, 4, 'Reported By:', curses.A_DIM)
            safe_addstr(stdscr, 2, 18, r.get('reportedBy', 'unknown'), curses.A_BOLD)
            
            safe_addstr(stdscr, 3, 4, 'Timestamp:', curses.A_DIM)
            safe_addstr(stdscr, 3, 18, fmt_ts(r.get('ts', 0)), curses.A_BOLD)
            
            safe_addstr(stdscr, 4, 4, 'Reason:', curses.A_DIM)
            safe_addstr(stdscr, 4, 18, r.get('reason', ''), curses.color_pair(3) | curses.A_BOLD)
            
            safe_addstr(stdscr, 5, 4, 'Status:', curses.A_DIM)
            safe_addstr(stdscr, 5, 18, r.get('status', 'Needs review'), curses.color_pair(4) | curses.A_BOLD)
            
            safe_addstr(stdscr, 7, 2, '--- CHAT CONTEXT ---', curses.A_DIM)
            
            context = r.get('context', [])
            row = 8
            for msg_item in context:
                if row >= h - 6: break
                sender = msg_item.get('from', '')
                text = msg_item.get('text', '')
                is_reported = ' [REPORTED]' if msg_item.get('reported') else ''
                
                cp = 4 if is_reported else (2 if sender == r.get('reportedBy') else 1)
                safe_addstr(stdscr, row, 4, f"{sender}: {text}{is_reported}"[:w - 6], curses.color_pair(cp))
                row += 1
                
            safe_addstr(stdscr, h - 3, 4, '[K] Keep / Resolve Report  [D] Delete Report', curses.color_pair(5) | curses.A_BOLD)
            draw_footer(stdscr, ' Esc back to list')
            
        if msg:
            safe_addstr(stdscr, h - 2, 2, msg,
                        curses.color_pair(2) if '✓' in msg else curses.color_pair(4))
                        
        stdscr.refresh(); key = stdscr.getch()
        
        if key in (ord('q'), 27):
            if view == 'detail': view = 'list'; msg = ''
            else: break
        elif key == ord('r') and view == 'list':
            fetch_reports()
            msg = '✓ Refreshed safety reports.'
        elif key == curses.KEY_UP and view == 'list': sel = max(0, sel - 1)
        elif key == curses.KEY_DOWN and view == 'list': sel = min(sel + 1, len(reports) - 1)
        elif key in (curses.KEY_ENTER, 10, 13) and view == 'list' and reports:
            view = 'detail'; msg = ''
        elif key in (ord('k'), ord('K')) and view == 'detail':
            r = reports[sel]
            if confirm(stdscr, "Resolve report (mark as Resolved)?"):
                d, err = call_admin_api('/api/admin/chat-reports/resolve', {'ts': r.get('ts'), 'action': 'resolve'})
                if d and d.get('ok'):
                    msg = '✓ Report marked as Resolved.'
                    view = 'list'
                    fetch_reports()
                else: msg = f'✗ Action failed: {err or d}'
        elif key in (ord('d'), ord('D')) and view == 'detail':
            r = reports[sel]
            if confirm(stdscr, "Delete safety report completely?"):
                d, err = call_admin_api('/api/admin/chat-reports/resolve', {'ts': r.get('ts'), 'action': 'delete'})
                if d and d.get('ok'):
                    msg = '✓ Report deleted successfully.'
                    view = 'list'
                    fetch_reports()
                else: msg = f'✗ Action failed: {err or d}'

# ══════════════════════════════════════════════════════════════════════════════
# MAIN MENU
# ══════════════════════════════════════════════════════════════════════════════

TOOLS = [
    ('Live Dashboard',           dashboard_tool,     '📊'),
    ('Applications',             applications_tool,  '📋'),
    ('Access / Tokens',          access_tool,        '🔑'),
    ('Sessions (Logs)',          sessions_tool,      '🕵️'),
    ('Encrypted DMs',            encrypt_tool,       '💬'),
    ('Suggestions',              suggestions_tool,   '💡'),
    ('Cheat Log Viewer',         cheat_log_tool,     '🚨'),
    ('Economy',                  economy_tool,       '💰'),
    ('Moderation',               moderation_tool,    '🔨'),
    ('Casino',                   casino_tool,        '🎰'),
    ('Invite System',            invites_tool,       '🎁'),
    ('Proxy & Traffic',          proxy_tool,         '🌐'),
    ('Shop Catalog Manager',     shop_catalog_tool,  '🛍️'),
    ('Soft Maintenance Mode',    system_control_tool,'🔧'),
    ('Safety Reports Moderator', safety_reports_tool,'🛡️'),
]

def main_menu(stdscr):
    init_colors(); curses.curs_set(0); stdscr.keypad(True)
    sel = 0
    while True:
        h, w = stdscr.getmaxyx(); stdscr.erase()
        title = f" {SITE.get('name', 'mitch.pro').upper()} ADMIN TOOLS "
        safe_addstr(stdscr, 2, max(0, (w - len(title)) // 2), title,
                    curses.color_pair(5) | curses.A_BOLD)
        safe_addstr(stdscr, 3, max(0, (w - 40) // 2), '─' * min(40, w), curses.color_pair(5) | curses.A_DIM)
        for i, (name, _, icon) in enumerate(TOOLS):
            attr = curses.color_pair(7) | curses.A_BOLD if i == sel else 0
            label = f' {i+1:>2}. {icon} {name:<32} '
            safe_addstr(stdscr, 5 + i, max(0, (w - len(label)) // 2), label, attr)
        safe_addstr(stdscr, 5 + len(TOOLS) + 1, max(0, (w - 20) // 2), ' [q] Quit ',
                    curses.color_pair(3))
        stdscr.refresh(); key = stdscr.getch()
        if key in (ord('q'), 27): break
        elif key == curses.KEY_UP: sel = max(0, sel - 1)
        elif key == curses.KEY_DOWN: sel = min(len(TOOLS) - 1, sel + 1)
        elif ord('1') <= key <= ord('9'):
            idx = key - ord('1')
            if idx < len(TOOLS): TOOLS[idx][1](stdscr)
        elif key in (curses.KEY_ENTER, 10, 13): TOOLS[sel][1](stdscr)

if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '-a':
        auto_file = DATA / 'auto_approve'
        if len(sys.argv) < 3 or sys.argv[2] not in ('0', '1'):
            print('Usage: ./admin_tools.py -a 0|1', file=sys.stderr); sys.exit(1)
        auto_file.write_text(sys.argv[2])
        print(f"Auto-approve {'ON' if sys.argv[2] == '1' else 'OFF'}")
    else:
        curses.wrapper(main_menu)
