import assert from 'node:assert/strict';
import { createHash, createHmac, randomBytes } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import webpush from 'web-push';

const repoRoot = join(import.meta.dir, '..');
const dataDir = mkdtempSync(join(tmpdir(), 'mitch-profile-test-'));
const secret = randomBytes(32);
const email = 'profile.integration@student.rjuhsd.us';
const raw = 'e' + createHash('sha256').update(email).digest('hex').slice(0, 24);
const sid = raw + '.' + createHmac('sha256', secret).update(raw).digest('hex').slice(0, 16);
const port = 6861;
const baseUrl = `http://127.0.0.1:${port}`;
const vapid = webpush.generateVAPIDKeys();

writeFileSync(join(dataDir, 'id_secret.key'), secret);
writeFileSync(join(dataDir, 'names.json'), JSON.stringify({ [sid]: email }));
writeFileSync(join(dataDir, 'profiles.json'), JSON.stringify({}));

const server = Bun.spawn(['bun', join(repoRoot, 'server.js')], {
  cwd: repoRoot,
  env: {
    ...process.env,
    PORT: String(port),
    DATA_DIR: dataDir,
    NODE_ENV: 'test',
    DEV_TEST_ACCESS: '0',
    VAPID_PUBLIC_KEY: vapid.publicKey,
    VAPID_PRIVATE_KEY: vapid.privateKey,
  },
  stdout: 'ignore',
  stderr: 'inherit',
});

try {
  let ready = false;
  for (let i = 0; i < 40; i++) {
    try {
      const response = await fetch(`${baseUrl}/api/site-info`);
      if (response.ok) { ready = true; break; }
    } catch {}
    await Bun.sleep(200);
  }
  assert(ready, 'Test server did not start');

  const initial = await fetch(`${baseUrl}/api/profile`, {
    headers: { Cookie: `id=${sid}` },
  });
  assert.equal(initial.status, 200, 'Legacy id cookie must load the profile editor');

  const picture = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';
  const save = await fetch(`${baseUrl}/api/profile`, {
    method: 'POST',
    headers: {
      Cookie: `id=${sid}`,
      Origin: baseUrl,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      displayName: 'Display Name',
      username: 'profile.integration',
      nickname: 'Nickname',
      bio: 'Updated biography',
      website: 'https://example.com/profile',
      pfp: picture,
    }),
  });
  const saved = await save.json();
  assert.equal(save.status, 200, saved.error || 'Legacy id cookie must save the profile');
  assert.equal(saved.profile.displayName, 'Display Name');
  assert.equal(saved.profile.nickname, 'Nickname');
  assert.equal(saved.profile.bio, 'Updated biography');
  assert.equal(saved.profile.pfp, picture);

  const reload = await fetch(`${baseUrl}/api/profile`, {
    headers: { Cookie: `id=${sid}` },
  });
  const profile = await reload.json();
  assert.equal(reload.status, 200);
  assert.equal(profile.displayName, 'Display Name', 'Nickname must not replace the stored display name in the editor');
  assert.equal(profile.nickname, 'Nickname');
  assert.equal(profile.username, 'profile.integration');
  assert.equal(profile.bio, 'Updated biography');
  assert.equal(profile.pfp, picture);

  console.log('Profile read/write integration checks passed.');
} finally {
  server.kill();
  await server.exited;
  rmSync(dataDir, { recursive: true, force: true });
}
