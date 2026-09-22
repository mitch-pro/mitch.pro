import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const server = readFileSync('server.js', 'utf8');
const preferences = readFileSync('webserver/preferences/index.html', 'utf8');
const shell = readFileSync('webserver/app-shell.js', 'utf8');
const profile = readFileSync('webserver/profile/index.html', 'utf8');
const home = readFileSync('webserver/index.html', 'utf8');

assert(server.includes("'Cache-Control': 'private, no-cache, no-store, must-revalidate'"), 'JSON responses must not be cached');
assert(server.includes('writeDocument(PROFILES_FILE, profiles)'), 'Profile saves must expose persistence errors');
assert(server.includes("type: 'profile_updated'"), 'Successful profile saves must notify connected pages');
assert(server.includes('profile: {'), 'Profile saves must return persisted fields');
const ownProfileRoutes = server.match(/if \(path === '\/api\/profile'[^]*?\n    }/g) || [];
assert(ownProfileRoutes.length >= 2, 'Own-profile read and write routes must exist');
assert(ownProfileRoutes.every(route => route.includes("cookies['studentId'] || cookies['id'] || ''")), 'Own-profile reads and writes must accept both supported session cookies');
assert(server.includes('const publicName = p.displayName || p.nickname || username;'), 'Saved display names must take precedence on member surfaces');
assert(server.includes("displayName: safeProfile.displayName || ''"), 'The editor must receive the stored display name instead of a nickname fallback');
assert(preferences.includes("cache: 'no-store'"), 'Profile editor must bypass browser caches');
assert(preferences.includes('applyAccountProfile(d.profile)'), 'Profile editor must apply confirmed server state');
assert(preferences.includes("CustomEvent('mitch-profile-updated'"), 'Profile editor must update the current browser immediately');
assert(shell.includes("event.detail.type === 'profile_updated'"), 'Shared shell must handle server profile updates');
assert(shell.includes("typeof window.loadMembers === 'function'"), 'Member surfaces must refresh after profile changes');
assert(profile.includes('load(true)'), 'Open profile pages must refresh after profile changes');
assert(server.includes("if (!u.hostname || u.username || u.password) return '';"), 'Extensionless HTTPS avatar URLs must be accepted without permitting URL credentials');
assert(!server.includes('const pathname = u.pathname.toLowerCase();'), 'Profile images must not require a filename extension');
assert(home.includes("image.classList.contains('member-presence-avatar')"), 'People rail must replace failed images with initials');
assert(home.includes('referrerpolicy="no-referrer"'), 'People rail images must avoid hotlink referrer failures');

console.log('Immediate, persisted profile update checks passed.');
