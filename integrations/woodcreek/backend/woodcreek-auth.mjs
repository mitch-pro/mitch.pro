import { randomBytes, createHash, timingSafeEqual } from 'node:crypto';

const SESSION = '__Host-tylerium_session';
const FLOW = '__Host-tylerium_flow';
const lifetime = 7 * 24 * 60 * 60;
const equal = (a, b) => typeof a === 'string' && typeof b === 'string' && Buffer.byteLength(a) === Buffer.byteLength(b) && timingSafeEqual(Buffer.from(a), Buffer.from(b));
const safeNext = value => typeof value === 'string' && /^\/(?!\/)/.test(value) && !/[\\\r\n]/.test(value) ? value : '/';
const cookies = req => Object.fromEntries(String(req.headers.cookie || '').split(';').map(part => {
  const i = part.indexOf('='); return i < 0 ? ['', ''] : [part.slice(0, i).trim(), part.slice(i + 1).trim()];
}));
function cookie(name, value, age) {
  return `${name}=${value}; Path=/; Max-Age=${Math.max(0, age)}; HttpOnly; Secure; SameSite=Lax`;
}
function reply(res, status, data) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'private, no-store', 'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff' });
  res.end(JSON.stringify(data));
}
function redirect(res, target, setCookies = []) {
  res.writeHead(303, { Location: target, 'Set-Cookie': setCookies, 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer' }); res.end();
}

export function createAccountAuth({ provider = process.env.MITCH_AUTH_ORIGIN || 'https://mitchdog.com', origin = 'https://woodcreek.site', fetcher = fetch, clock = Date.now } = {}) {
  const providerUrl = new URL(provider);
  if (providerUrl.protocol !== 'https:' && !(['localhost', '127.0.0.1'].includes(providerUrl.hostname) && providerUrl.protocol === 'http:')) throw new Error('Account provider must use HTTPS');
  if (providerUrl.pathname !== '/' || providerUrl.search || providerUrl.hash || providerUrl.username || providerUrl.password) throw new Error('Account provider must be an origin');
  const cache = new Map();
  const tokenOf = req => req.headers.authorization?.startsWith('Bearer ') ? req.headers.authorization.slice(7) : cookies(req)[SESSION] || '';
  const hasBearer = req => req.headers.authorization?.startsWith('Bearer ');
  const sameOrigin = req => req.headers.origin === origin;
  async function providerPost(path, body = {}, token = '') {
    return fetcher(new URL(`/api/woodcreek/auth/${path}`, providerUrl), { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(8000), headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, body: JSON.stringify(body) });
  }
  async function authenticate(req, fresh = false) {
    const token = tokenOf(req);
    if (!token.startsWith('wc1.') || token.length > 4096) return null;
    const key = createHash('sha256').update(token).digest('hex');
    const cached = cache.get(key);
    if (!fresh && cached && cached.until > clock()) return cached.value;
    try {
      const response = await providerPost('session', {}, token);
      if (!response.ok) { cache.delete(key); return null; }
      const value = await response.json();
      if (!value.active || !value.account || value.expires_at <= clock()) { cache.delete(key); return null; }
      if (cache.size >= 1000) cache.delete(cache.keys().next().value);
      cache.set(key, { until: Math.min(clock() + 10_000, value.expires_at), value });
      return value;
    } catch { return null; }
  }
  async function handle(req, res, url) {
    if (url.pathname === '/auth/login' && req.method === 'GET') {
      const state = randomBytes(32).toString('base64url'), verifier = randomBytes(32).toString('base64url');
      const flow = Buffer.from(JSON.stringify({ state, verifier, next: safeNext(url.searchParams.get('next')), created: clock() })).toString('base64url');
      const target = new URL('/api/woodcreek/auth/authorize', providerUrl);
      target.searchParams.set('state', state); target.searchParams.set('code_challenge', createHash('sha256').update(verifier).digest('base64url')); target.searchParams.set('code_challenge_method', 'S256');
      redirect(res, target.href, [cookie(FLOW, flow, 600)]); return true;
    }
    if (url.pathname === '/auth/callback' && req.method === 'GET') {
      let flow;
      try { flow = JSON.parse(Buffer.from(cookies(req)[FLOW] || '', 'base64url').toString()); } catch {}
      const code = url.searchParams.get('code') || '';
      if (!flow || !equal(flow.state, url.searchParams.get('state')) || !Number.isFinite(flow.created) || clock() - flow.created > 600_000 || flow.created > clock() || !/^[\w-]{43}$/.test(code)) {
        redirect(res, '/?signin=expired', [cookie(FLOW, '', 0)]); return true;
      }
      try {
        const response = await providerPost('token', { code, code_verifier: flow.verifier });
        if (!response.ok) throw new Error('Account exchange failed');
        const value = await response.json();
        if (typeof value.access_token !== 'string' || !value.access_token.startsWith('wc1.') || value.access_token.length > 4096 || !/^[\w.-]+$/.test(value.access_token) || !(value.expires_in > 0)) throw new Error('Invalid session');
        redirect(res, safeNext(flow.next), [cookie(FLOW, '', 0), cookie(SESSION, value.access_token, Math.min(lifetime, Math.floor(value.expires_in)))]);
      } catch { redirect(res, '/?signin=expired', [cookie(FLOW, '', 0)]); }
      return true;
    }
    if (url.pathname === '/auth/logout' && req.method === 'POST') {
      if (!sameOrigin(req)) { reply(res, 403, { error: 'Request origin is not allowed.' }); return true; }
      const token = tokenOf(req);
      try {
        if (token) { const response = await providerPost('revoke', {}, token); if (!response.ok && response.status !== 401) throw new Error('Sign out failed'); }
      } catch { reply(res, 503, { error: 'Sign out could not finish. Please try again.' }); return true; }
      cache.delete(createHash('sha256').update(token).digest('hex'));
      res.setHeader('Set-Cookie', cookie(SESSION, '', 0)); reply(res, 200, { success: true }); return true;
    }
    if (url.pathname === '/api/account' && req.method === 'GET') {
      const session = await authenticate(req);
      reply(res, session ? 200 : 401, { account: session?.account || null }); return true;
    }
    if (url.pathname.startsWith('/auth/')) { reply(res, 404, { error: 'Not found.' }); return true; }
    if (url.pathname.startsWith('/api/')) {
      if (req.method !== 'GET' && req.method !== 'HEAD' && !hasBearer(req) && (!sameOrigin(req) || req.headers['x-mitch-requested-with'] !== '1')) { reply(res, 403, { error: 'Request origin is not allowed.' }); return true; }
      if (!await authenticate(req)) { reply(res, 401, { error: 'Sign in with your Mitch account to continue.' }); return true; }
    }
    return false;
  }
  return { handle, authenticate, socketAllowed: req => hasBearer(req) || sameOrigin(req) };
}
