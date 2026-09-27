#!/usr/bin/env node
// Keeps IMAP IDLE connections open, caches full bodies + sent replies.
// Writes team_inbox_cache.json on any change.

const { ImapFlow }  = require('imapflow');
const { spawnSync } = require('child_process');
const fs   = require('fs');
const path = require('path');

try {
  fs.readFileSync(path.join(__dirname, '..', '.env'), 'utf8').split('\n').forEach(line => {
    const m = line.match(/^\s*(?:export\s+)?([A-Z_]+)\s*=\s*"?([^"]*)"?\s*$/);
    if (m) process.env[m[1]] = m[2];
  });
} catch {}

const USER             = process.env.SUPPORT_USER;
const PASS             = process.env.SUPPORT_PASS;
const IMAP_HOST        = process.env.MAIL_IMAP_HOST || 'mail.mitch.pro';
if (!USER || !PASS) {
  console.error("[imap-watcher] Error: SUPPORT_USER or SUPPORT_PASS not set in environment/dotenv.");
  process.exit(1);
}
const CACHE_FILE       = path.join(__dirname, '..', 'data', 'team_inbox_cache.json');
const AUTOREPLY_FILE   = path.join(__dirname, '..', 'data', 'autoreply_sent.json');
const SUPPORT_SEND     = path.join(__dirname, 'support_send.js');
const NTFY_TOPIC       = (process.env.NTFY_TOPIC || '').trim();
const NTFY_USER        = (process.env.NTFY_USER || '').trim();
const NTFY_PASS        = (process.env.NTFY_PASS || '').trim();

const AUTOREPLY_BODY = 'Thank you for contacting mitch.pro support. Please reply with your problem and we will get you into contact with a mitch.pro representative as soon as possible.';

function loadSent() {
  try { return new Set(JSON.parse(fs.readFileSync(AUTOREPLY_FILE, 'utf8'))); } catch { return new Set(); }
}
function saveSent(s) { fs.writeFileSync(AUTOREPLY_FILE, JSON.stringify([...s])); }

async function ntfy(msg, title = 'Support Email', priority = 'high') {
  if (!NTFY_TOPIC) return;
  const url = /^https?:\/\//i.test(NTFY_TOPIC)
    ? NTFY_TOPIC
    : `https://ntfy.sh/${NTFY_TOPIC.replace(/^\/+/, '')}`;
  const headers = { Title: title, Priority: priority };
  if (NTFY_USER || NTFY_PASS) {
    headers['Authorization'] = 'Basic ' + Buffer.from(`${NTFY_USER}:${NTFY_PASS}`).toString('base64');
  }
  try {
    await fetch(url, {
      method: 'POST', body: msg,
      headers,
    });
  } catch {}
}

function sendSupportAutoreply(msg) {
  const replyTo = msg.from;
  if (!replyTo) return;
  const subject = msg.subject?.startsWith('Re:') ? msg.subject : 'Re: ' + (msg.subject || '');
  const args = ['node', SUPPORT_SEND];
  if (msg.messageId) args.push('--in-reply-to', `<${msg.messageId}>`);
  args.push(replyTo, subject, AUTOREPLY_BODY);
  const r = spawnSync(args[0], args.slice(1), { encoding: 'utf8', timeout: 30000 });
  if (r.status === 0) log(`[autoreply] Sent to ${replyTo}`);
  else log(`[autoreply] Failed to ${replyTo}: ${r.stderr?.trim()}`);
}

function checkForSupportEmails(messages, prevMessageIds) {
  const sent = loadSent();
  let changed = false;
  for (const msg of messages) {
    if (msg.dir !== 'in') continue;
    const key = msg.messageId || `${msg.from}|${msg.subject}`;
    if (sent.has(key)) continue;
    if (prevMessageIds.has(key)) continue; // skip emails we've already seen
    if (!msg.body?.includes('SUPPORT')) continue;
    sent.add(key);
    changed = true;
    ntfy(`SUPPORT email from ${msg.from}: ${msg.subject}`, 'Support Request', 'high');
    sendSupportAutoreply(msg);
  }
  if (changed) saveSent(sent);
}

if (!USER || !PASS) { console.error('Set SUPPORT_USER and SUPPORT_PASS in environment/dotenv'); process.exit(1); }

function log(msg) { console.log(`[${new Date().toISOString()}] ${msg}`); }

function extractText(raw) {
  if (!raw) return '';
  raw = raw.toString();
  const bm = raw.match(/boundary="?([^"\r\n;]+)"?/i);
  if (bm) {
    const boundary = bm[1].trim();
    const parts = raw.split(new RegExp('--' + boundary.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
    const texts = [];
    for (const part of parts) {
      if (/content-type:\s*text\/plain/i.test(part)) {
        const sep = part.indexOf('\r\n\r\n') >= 0 ? '\r\n\r\n' : '\n\n';
        const idx = part.indexOf(sep);
        if (idx >= 0) texts.push(part.slice(idx + sep.length).trim());
      }
    }
    if (texts.length) return texts.join('\n').trim();
  }
  if (/^content-type:/im.test(raw)) {
    const sep = raw.indexOf('\r\n\r\n') >= 0 ? '\r\n\r\n' : '\n\n';
    const idx = raw.indexOf(sep);
    if (idx >= 0) return raw.slice(idx + sep.length).trim();
  }
  return raw.trim();
}

// Normalize subject for thread grouping
function threadKey(subject) {
  return (subject || '').replace(/^(re|fwd?):\s*/i, '').trim().toLowerCase();
}

async function fetchMailbox(client, mailbox) {
  const messages = [];
  let exists = false;
  try {
    await client.mailboxOpen(mailbox);
    exists = true;
  } catch { return messages; }
  if (!exists) return messages;

  for await (const msg of client.fetch({ all: true }, {
    uid: true, flags: true, envelope: true, source: true,
  })) {
    const fromRaw  = msg.envelope?.from?.[0];
    const toAddrs  = msg.envelope?.to?.map(a => a.address) || [];
    const to       = toAddrs.join(', ');
    const from     = fromRaw?.address || '';
    const isInbox  = mailbox === 'INBOX';

    // Inbox: only support@ emails. Sent: only emails to external addresses (replies).
    if (isInbox && !to.toLowerCase().includes('support@mitch.pro')) continue;
    if (!isInbox && toAddrs.every(a => a.toLowerCase().endsWith('@mitch.pro'))) continue;

    messages.push({
      uid:       msg.uid,
      mailbox,
      dir:       isInbox ? 'in' : 'out',
      from,
      fromName:  fromRaw?.name || '',
      to,
      subject:   msg.envelope?.subject || '(no subject)',
      threadKey: threadKey(msg.envelope?.subject || ''),
      date:      msg.envelope?.date?.toISOString() || '',
      messageId: msg.envelope?.messageId || '',
      references: msg.envelope?.messageId || '',
      seen:      msg.flags?.has('\\Seen') || false,
      body:      extractText(msg.source?.toString() || ''),
    });
  }
  return messages;
}

async function fetchAll(client) {
  const inbox = await fetchMailbox(client, 'INBOX');

  // Build set of thread keys from inbox to filter sent folder
  const inboxKeys = new Set(inbox.map(m => m.threadKey));

  const sent = await fetchMailbox(client, 'Sent');
  const relevantSent = sent.filter(m => inboxKeys.has(m.threadKey));

  const all = [...inbox, ...relevantSent];
  all.sort((a, b) => new Date(b.date) - new Date(a.date));
  return all;
}

async function writeCache(messages) {
  const tmp = CACHE_FILE + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(messages, null, 2));
  fs.renameSync(tmp, CACHE_FILE);
  const inbox = messages.filter(m => m.dir === 'in').length;
  const sent  = messages.filter(m => m.dir === 'out').length;
  log(`Cache updated: ${inbox} received, ${sent} sent`);
}


async function run() {
  const client = new ImapFlow({
    host: IMAP_HOST,
    port: 993,
    secure: true,
    auth: { user: USER, pass: PASS },
    logger: false,
  });

  client.on('error', err => log(`IMAP error: ${err.message}`));
  await client.connect();
  log('Connected');

  // Initial fetch
  let currentMessages = await fetchAll(client);
  await writeCache(currentMessages);
  let knownIds = new Set(currentMessages.map(m => m.messageId || `${m.from}|${m.subject}`));

  // Open INBOX for IDLE
  const lock = await client.getMailboxLock('INBOX');
  try {
    client.on('exists', async () => {
      log('New mail — refreshing cache…');
      try {
        const messages = await fetchAll(client);
        await writeCache(messages);
        checkForSupportEmails(messages, knownIds);
        knownIds = new Set(messages.map(m => m.messageId || `${m.from}|${m.subject}`));
      }
      catch (e) { log(`Fetch error: ${e.message}`); }
    });
    log('IDLE — waiting for new mail…');
    await client.idle();
  } finally {
    lock.release();
  }
  await client.logout();
}

async function runWithReconnect() {
  while (true) {
    try { await run(); }
    catch (e) { log(`Disconnected: ${e.message}. Reconnecting in 10s…`); }
    await new Promise(r => setTimeout(r, 10_000));
  }
}

// ── mitch-mail (Rust) shim ── while the Rust service's IMAP watcher is
// healthy, hold this supervisor slot (server.js restarts us every 10s on any
// exit) WITHOUT double-watching. If the service goes away we fall through to
// the original imapflow watcher below, which takes over seamlessly.
(async () => {
  const { rsWatcherActive } = await import('./_rs_shim.js');
  if (await rsWatcherActive()) {
    console.log('[imap-watcher] mitch-mail service owns watching — holding supervisor slot');
    while (await rsWatcherActive()) {
      await new Promise(r => setTimeout(r, 60_000));
    }
    console.log('[imap-watcher] mitch-mail service gone — taking over watching');
  }
  runWithReconnect();
})();
