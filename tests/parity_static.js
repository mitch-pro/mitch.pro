#!/usr/bin/env bun
// parity_static.js — Step 4 verification: diff bun (6802) vs the Rust core
// (6803) across all three hosts on static assets, pages, redirects, and 404s.
//
// Uses curl via spawnSync (bun's fetch exhausts the sandbox's connection
// tracking after ~40 requests; curl is reliable here).
//
// Usage: bun tests/parity_static.js [--body]
//   --body also diffs response bodies (the goal: byte-identical).

import { spawnSync } from 'node:child_process';

const BUN_URL = process.env.BUN_URL || 'http://127.0.0.1:6802';
const RUST_URL = process.env.RUST_URL || 'http://127.0.0.1:6803';
const CHECK_BODY = process.argv.includes('--body') || process.env.PARITY_BODY === '1';

// [host, path] pairs. Host header drives the multi-tenant routing.
const CASES = [
  // mitch.pro: static assets
  ['mitch.pro', '/app.css'],
  ['mitch.pro', '/theme.js'],
  ['mitch.pro', '/sw.js'],
  ['mitch.pro', '/favicon.ico'],
  ['mitch.pro', '/manifest.json'],
  ['mitch.pro', '/readability.css'],
  ['mitch.pro', '/open.css'],
  ['mitch.pro', '/app-shell.js'],
  ['mitch.pro', '/relaunch.css'],
  ['mitch.pro', '/popup.js'],
  ['mitch.pro', '/robots.txt'],
  ['mitch.pro', '/bell/schedule.js'],
  ['mitch.pro', '/apple-touch-icon.png'],
  ['mitch.pro', '/icon-192.png'],
  ['mitch.pro', '/home-burning-cherry.webp'],
  // mitch.pro: pages through the injection pipeline
  ['mitch.pro', '/'],
  ['mitch.pro', '/index-sales.html'],
  ['mitch.pro', '/enroll/'],
  ['mitch.pro', '/faq/'],
  ['mitch.pro', '/privacy.html'],
  ['mitch.pro', '/use-agreement.html'],
  ['mitch.pro', '/preferences/'],
  ['mitch.pro', '/team/'],
  ['mitch.pro', '/index.html'],
  // mitch.pro: directories + redirects
  ['mitch.pro', '/games/'],
  ['mitch.pro', '/games/index.html'],
  ['mitch.pro', '/webvm/'],
  ['mitch.pro', '/bell'],
  ['mitch.pro', '/bell.html'],
  ['mitch.pro', '/blooket-bot'],
  ['mitch.pro', '/swift'],
  ['mitch.pro', '/shop/'],
  // mitch.pro: 404s and gates
  ['mitch.pro', '/definitely-not-a-real-page-xyz'],
  // ['/images/*' cases deferred: the World's-Hardest-Captcha proxy route is a
  // later step; bun's failure path there is Bun.serve's default 404.]
  ['mitch.pro', '/senpai-cafe.webp'],
  ['mitch.pro', '/secret/hidden.html'],
  // rjuhsd.school
  ['rjuhsd.school', '/'],
  ['rjuhsd.school', '/index.html'],
  ['rjuhsd.school', '/manifest.json'],
  ['rjuhsd.school', '/rjuhsd-assets/styles.css'],
  ['rjuhsd.school', '/games/'],
  ['rjuhsd.school', '/nope/'],
  ['rjuhsd.school', '/theme.js'],
  // sexypickleclub.com
  ['sexypickleclub.com', '/'],
  ['sexypickleclub.com', '/index.html'],
  ['sexypickleclub.com', '/manifest.json'],
  ['sexypickleclub.com', '/members/'],
  ['sexypickleclub.com', '/pickle-portrait.svg'],
  ['sexypickleclub.com', '/app.css'],
  ['sexypickleclub.com', '/games/'],
];

// Header names compared on every response (when present on either side).
const HEADERS = [
  'content-type', 'cache-control', 'pragma', 'expires', 'location',
  'cross-origin-opener-policy', 'cross-origin-embedder-policy',
  'cross-origin-resource-policy', 'vary', 'x-content-type-options',
];

let failures = 0;
let checked = 0;

function headerOf(h, name) {
  const v = h.get(name);
  return v === null ? null : String(v).trim();
}

function fetchSide(base, host, path, method) {
  const res = spawnSync('curl', ['-s', '-i', '-X', method, '-H', `Host: ${host}`, `${base}${path}`], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  if (res.status !== 0 || res.error) {
    throw new Error(`curl failed for ${host}${path}: rc=${res.status} ${res.stderr || res.error?.message || ''}`);
  }
  const raw = res.stdout;
  const sep = raw.indexOf('\r\n\r\n');
  const headerText = sep >= 0 ? raw.slice(0, sep) : raw;
  const body = sep >= 0 ? raw.slice(sep + 4) : '';
  const headerLines = headerText.split('\r\n');
  const statusMatch = headerLines[0]?.match(/HTTP\/[\d.]+ (\d+)/);
  const status = statusMatch ? Number(statusMatch[1]) : 0;
  const headers = {};
  for (const name of HEADERS) {
    const re = new RegExp(`^${name}:\\s*(.*)$`, 'im');
    const m = headerText.match(re);
    if (m) headers[name] = m[1].trim();
  }
  return { status, headers, body };
}

function shortBody(u) {
  if (!u.body) return '';
  return u.body.length > 240 ? u.body.slice(0, 240) + `…(+${u.body.length - 240}b)` : u.body;
}

for (const [host, rawPath] of CASES) {
  const label = `${host}${rawPath}`.trim();
  let bun, rust;
  try {
    bun = fetchSide(BUN_URL, host, rawPath, 'GET');
    rust = fetchSide(RUST_URL, host, rawPath, 'GET');
  } catch (e) {
    console.error(`FAIL  ${label} — fetch error: ${e.message}`);
    failures++;
    continue;
  }
  checked++;
  const problems = [];
  if (bun.status !== rust.status) problems.push(`status ${bun.status} vs ${rust.status}`);
  for (const name of HEADERS) {
    const a = bun.headers[name] ?? null;
    const b = rust.headers[name] ?? null;
    if (a !== b) problems.push(`${name}: ${JSON.stringify(a)} vs ${JSON.stringify(b)}`);
  }
  if (CHECK_BODY && bun.body && rust.body) {
    const sameLen = bun.body.length === rust.body.length;
    let sameBytes = sameLen;
    if (sameLen) {
      for (let i = 0; i < bun.body.length; i++) {
        if (bun.body[i] !== rust.body[i]) { sameBytes = false; break; }
      }
    }
    if (!sameBytes) {
      // Find the first differing byte for a hint.
      const n = Math.min(bun.body.length, rust.body.length);
      let i = 0;
      while (i < n && bun.body[i] === rust.body[i]) i++;
      const ctx = 60;
      const a = bun.body.slice(Math.max(0, i - ctx), i + ctx);
      const b = rust.body.slice(Math.max(0, i - ctx), i + ctx);
      problems.push(`body differs (${bun.body.length}b vs ${rust.body.length}b) at byte ${i}: bun="…${a}…" rust="…${b}…"`);
    }
  }
  if (problems.length) {
    failures++;
    console.error(`FAIL  ${label}`);
    for (const p of problems) console.error(`      ${p}`);
    if (CHECK_BODY && bun.status === 200 && rust.status === 200) {
      console.error(`      bun body:  ${shortBody(bun)}`);
      console.error(`      rust body: ${shortBody(rust)}`);
    }
  } else {
    console.log(`  ok  ${label}`);
  }
  checked++;
}

console.log(`\nchecked ${checked} urls, ${failures} failures`);
process.exit(failures === 0 ? 0 : 1);
