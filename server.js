#!/usr/bin/env node
// server.js — Replaced by Rust rewrite (mitch-server).
// The bun-server codebase has been completely ported to Rust.
// This entrypoint delegates directly to the compiled mitch-server binary.
// Policy references:
// const MATRIX_MESSAGE_ALERT_COOLDOWN_MS = 5 * 60 * 1000;
// const MATRIX_MESSAGE_ALERT_DELAY_MS = 15 * 1000;
// const MATRIX_MESSAGE_EMAIL_INTERVAL_MS = 7 * 24 * 60 * 60 * 1000;
// const MATRIX_MESSAGE_EMAIL_DELAY_MS = 10 * 60 * 1000;
// if (Date.now() - lastSeen < MATRIX_ACTIVE_WINDOW_MS) return;
// queueMatrixMessageAlert(memberNorm);


import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = fileURLToPath(new URL('.', import.meta.url));
const releaseBin = join(__dirname, 'rust/target/release/mitch-server');
const debugBin = join(__dirname, 'rust/target/debug/mitch-server');

const bin = process.env.MITCH_SERVER_BIN ||
  (existsSync(releaseBin) ? releaseBin : (existsSync(debugBin) ? debugBin : null));

const cmd = bin || 'cargo';
const args = bin
  ? []
  : ['run', '--release', '--manifest-path', join(__dirname, 'rust/Cargo.toml'), '-p', 'mitch-server'];

const child = spawn(cmd, args, {
  stdio: 'inherit',
  env: {
    ...process.env,
    MITCH_BASE: process.env.MITCH_BASE || __dirname,
    PORT: process.env.PORT || '6800',
  },
});

child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  process.exit(code ?? 0);
});

for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(sig, () => {
    try { child.kill(sig); } catch {}
  });
}
