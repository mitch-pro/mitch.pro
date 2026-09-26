import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { chromium } from '@playwright/test';

const script = readFileSync(new URL('../webserver/matrix/media-auth.js', import.meta.url));
const image = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jR6sAAAAASUVORK5CYII=', 'base64');
const browser = await chromium.launch({
  headless: true,
  executablePath: 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'
});
const page = await browser.newPage();
let authorizedRequests = 0;
await page.addInitScript(() => localStorage.setItem('cinny_access_token', 'test-token'));
await page.route('https://mitchdog.com/**', async route => {
  const url = new URL(route.request().url());
  if (url.pathname === '/matrix/media-auth.js') {
    return route.fulfill({ status: 200, contentType: 'application/javascript', body: script });
  }
  if (url.pathname.startsWith('/_matrix/client/v1/media/')) {
    assert.equal(route.request().headers().authorization, 'Bearer test-token');
    authorizedRequests++;
    return route.fulfill({ status: 200, contentType: 'image/png', body: image });
  }
  if (url.pathname.startsWith('/_matrix/media/v3/')) {
    return route.fulfill({ status: 404 });
  }
  return route.fulfill({ status: 200, contentType: 'text/html', body: `
    <img id="avatar" src="/_matrix/media/v3/thumbnail/mitch.pro/avatar-id?width=48&height=48">
    <img id="message" src="/_matrix/media/v3/download/mitch.pro/message-id">
    <div id="background" style="background-image:url('/_matrix/media/v3/download/mitch.pro/avatar-id')"></div>
    <script defer src="/matrix/media-auth.js"></script>` });
});
await page.goto('https://mitchdog.com/matrix/');
await page.waitForFunction(() => ['avatar', 'message'].every(id => {
  const image = document.getElementById(id);
  return image.src.startsWith('blob:') && image.naturalWidth > 0;
}) && document.getElementById('background').style.backgroundImage.includes('blob:'));
assert.equal(authorizedRequests, 3);
await browser.close();
console.log('Matrix authenticated image, profile avatar, and background repair passed');
