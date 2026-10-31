import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const server = readFileSync('server.js', 'utf8');
const admin = readFileSync('webserver/admin/index.html', 'utf8');
const vmAdmin = readFileSync('webserver/admin/vms/admin-vms.js', 'utf8');
const shell = readFileSync('webserver/app-shell.js', 'utf8');
const shellCss = readFileSync('webserver/app.css', 'utf8');
const tester = readFileSync('webserver/tester/index.html', 'utf8');

assert.match(server, /path === '\/api\/admin\/admins'/, 'owner admin-role endpoint must exist');
assert.match(server, /if \(!isOwnerEmail\(actor\)\) return jsonResp\(403/, 'admin-role endpoint must be owner-only');
assert.match(server, /saveJsonSync\(ADMINS_FILE, config\)/, 'admin-role changes must persist immediately');
assert.match(server, /logAdminAction\(actor, active \? 'add_admin' : 'remove_admin'/, 'admin-role changes must be audited');
assert.match(admin, /Danger zone[\s\S]*Grant full admin/, 'owner panel must warn before direct full-admin grants');

assert.match(server, /isOwner: isOwnerEmail\(email\)/, 'VM actor must carry owner authority');
assert.match(server, /if \(actor\?\.isOwner\) return true/, 'owners must be able to reach every VM record');
assert.match(server, /isAdminUsingOtherVm && !actor\.isOwner/, 'only non-owner admins should require a VM access grant');
assert.match(server, /desktopCredentials: actor\.isOwner/, 'VM credentials must only be serialized for owners');
assert.match(server, /createCipheriv\('aes-256-gcm', vmCredentialsKey\(\), iv\)/, 'saved VM passwords must be encrypted at rest');
assert.match(server, /path === '\/api\/admin\/vms\/credentials'[\s\S]*if \(!actor\.isOwner\)/, 'only owners may reset and store an existing VM login');
assert.match(vmAdmin, /overview\.viewerIsOwner/, 'VM admin UI must only render credentials for owners');
assert.match(vmAdmin, /Set new login/, 'owners must be able to replace credentials for older VMs');

for (const label of ['Home', 'VM Lab', 'Chat', 'Games', 'People', 'Schedule']) {
  assert(shell.includes(`label: '${label}'`), `shared masthead must include ${label}`);
}
assert.match(shell, /unified-masthead/, 'home and interior mastheads must share one class');
assert.match(shellCss, /Shared masthead/, 'shared masthead styling must be global');
assert.match(tester, /href="\/app\.css"/, 'tester page must use the shared site stylesheet');
assert.doesNotMatch(tester, /home-burning-cherry|staff-galaxy|--t-bgr/, 'tester page must not carry its own backdrop');

console.log('Owner admin controls, owner VM access, shared navigation, and tester redesign checks passed.');
