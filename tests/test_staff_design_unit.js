import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const admin = readFileSync('webserver/admin/index.html', 'utf8');
const moderator = readFileSync('webserver/moderator/index.html', 'utf8');
const home = readFileSync('webserver/index.html', 'utf8');
const homeCss = readFileSync('webserver/home.css', 'utf8');
const staffCss = readFileSync('webserver/staff-command-v2.css', 'utf8');
const vmPortal = readFileSync('webserver/vms/index.html', 'utf8');
const vmPortalJs = readFileSync('webserver/vms/portal.js', 'utf8');
const vmPortalCss = readFileSync('webserver/vms/portal.css', 'utf8');
const vmDesktop = readFileSync('webserver/vms/desktop/index.html', 'utf8');
const vmDesktopCss = readFileSync('webserver/vms/desktop/desktop.css', 'utf8');

assert(admin.includes('/staff-command-v2.css?v=1'), 'Admin must load the redesigned staff system');
assert(moderator.includes('/staff-command-v2.css?v=1'), 'Moderator must load the redesigned staff system');
assert(moderator.includes('class="moderator-hero"'), 'Moderator must have its dedicated workspace hero');
assert(home.match(/class="staff-access-copy"/g)?.length === 3, 'All homepage staff buttons need labels and descriptions');
assert(home.includes('/home.css?v=5'), 'Homepage must load its consolidated stylesheet');
assert(homeCss.includes('.feature-chat { grid-column: 2; grid-row: 1 / 3; }'), 'Matrix Chat must stay the featured (larger) tile');
assert(homeCss.includes('.feature-vms { grid-column: 1; grid-row: 1; }') && homeCss.includes('.feature-games { grid-column: 1; grid-row: 2; }'), 'VM Lab and Play Games must stack as the two compact tiles beside the featured one');
assert(homeCss.includes('body.home:is(.is-staff, .is-admin) .staff-access-rail'), 'Staff rail must only appear for staff');
assert(homeCss.includes('.staff-access-rail:focus-within'), 'Staff rail must expand for keyboard users');
assert(staffCss.includes('#command-center #owner-tools'), 'Owner tools need their own visual treatment');
assert(staffCss.includes('@media (max-width: 700px)'), 'Staff panels need a dedicated mobile layout');
assert(staffCss.includes('@media (prefers-reduced-motion: reduce)'), 'Staff motion must honor reduced-motion preferences');
assert(vmPortal.includes('/vms/portal.css?v=3'), 'VM portal must load the redesigned interface');
assert(vmPortalJs.includes('class="resource-grid"') && vmPortalJs.includes('class="machine-facts"'), 'VM card must expose a clear system overview');
assert(vmPortalCss.includes('@media(max-width:560px)') && vmPortalCss.includes('@media(max-height:760px)'), 'VM portal must fit phones and Chromebook-height screens');
assert(vmDesktop.includes('/vms/desktop/desktop.css?v=3') && vmDesktop.includes('class="machine-icon"'), 'Desktop viewer must load its upgraded toolbar');
assert(vmDesktopCss.includes('@media(max-width:780px)') && vmDesktopCss.includes('.mobile-dock'), 'Desktop viewer must retain dedicated mobile controls');

let depth = 0;
for (const char of staffCss.replace(/\/\*[\s\S]*?\*\//g, '')) {
  if (char === '{') depth++;
  if (char === '}') depth--;
  assert(depth >= 0, 'Staff stylesheet closes a block too early');
}
assert.equal(depth, 0, 'Staff stylesheet must have balanced blocks');

console.log('Owner, admin, moderator, homepage staff controls, and responsive design checks passed.');
