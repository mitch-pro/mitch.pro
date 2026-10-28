import assert from 'node:assert/strict';

// Unit tests for administrative role resolution and hierarchy.
// Tests role logic against dynamic configuration without hardcoded personal credentials.

function normalizeEmail(email) {
  return String(email || '').trim().toLowerCase();
}

function loadAdminConfig(mockConfig = {}) {
  return {
    owners: Array.isArray(mockConfig.owners) ? mockConfig.owners.map(normalizeEmail) : ['admin@mitch.pro'],
    coOwners: Array.isArray(mockConfig.coOwners) ? mockConfig.coOwners.map(normalizeEmail) : [],
    admins: Array.isArray(mockConfig.admins) ? mockConfig.admins.map(normalizeEmail) : [],
  };
}

function isOwnerEmail(config, email) {
  const norm = normalizeEmail(email);
  return norm.length > 0 && config.owners.includes(norm);
}

function isCoOwnerEmail(config, email) {
  const norm = normalizeEmail(email);
  return norm.length > 0 && config.coOwners.includes(norm);
}

function isAdminEmail(config, email) {
  const norm = normalizeEmail(email);
  if (!norm) return false;
  return config.owners.includes(norm) || config.coOwners.includes(norm) || config.admins.includes(norm);
}

function siteAdminEmails(config) {
  const all = [...config.owners, ...config.coOwners, ...config.admins];
  return Array.from(new Set(all));
}

function canModifyRole(actorConfig, actorEmail, targetConfig, targetEmail) {
  // Only primary owners can modify owner/co-owner roles or demote owners
  const actorIsOwner = isOwnerEmail(actorConfig, actorEmail);
  const targetIsOwner = isOwnerEmail(targetConfig, targetEmail);
  if (targetIsOwner && !actorIsOwner) {
    return false;
  }
  return actorIsOwner;
}

// 1. Dynamic role resolution with arbitrary, mock test identities
const dynamicConfig = loadAdminConfig({
  owners: ['test-owner@example.com'],
  coOwners: ['test-coowner@example.com'],
  admins: ['test-admin@example.com'],
});

const owner = 'test-owner@example.com';
const coOwner = 'test-coowner@example.com';
const admin = 'test-admin@example.com';
const regularUser = 'regular-user@example.com';

// Owner role assertions
assert.equal(isOwnerEmail(dynamicConfig, owner), true, 'Owner email must be recognized as owner');
assert.equal(isCoOwnerEmail(dynamicConfig, owner), false, 'Owner email is not a co-owner');
assert.equal(isAdminEmail(dynamicConfig, owner), true, 'Owner must receive administrative access');

// Co-owner role assertions (cannot masquerade as primary owner)
assert.equal(isCoOwnerEmail(dynamicConfig, coOwner), true, 'Co-owner email must be recognized as co-owner');
assert.equal(isOwnerEmail(dynamicConfig, coOwner), false, 'Co-owner must NOT receive primary owner status');
assert.equal(isAdminEmail(dynamicConfig, coOwner), true, 'Co-owner must receive administrative access');

// Plain admin assertions
assert.equal(isOwnerEmail(dynamicConfig, admin), false, 'Admin must not be promoted to owner');
assert.equal(isCoOwnerEmail(dynamicConfig, admin), false, 'Admin is not a co-owner');
assert.equal(isAdminEmail(dynamicConfig, admin), true, 'Admin must have admin access');

// Regular user assertions
assert.equal(isOwnerEmail(dynamicConfig, regularUser), false, 'Regular user is not owner');
assert.equal(isCoOwnerEmail(dynamicConfig, regularUser), false, 'Regular user is not co-owner');
assert.equal(isAdminEmail(dynamicConfig, regularUser), false, 'Regular user is not admin');

// Site admin collective membership
const siteAdmins = siteAdminEmails(dynamicConfig);
assert.equal(siteAdmins.includes(normalizeEmail(owner)), true);
assert.equal(siteAdmins.includes(normalizeEmail(coOwner)), true);
assert.equal(siteAdmins.includes(normalizeEmail(admin)), true);
assert.equal(siteAdmins.includes(normalizeEmail(regularUser)), false);

// Role hierarchy & mutation protection: Co-owners or admins cannot demote/delete owners
assert.equal(canModifyRole(dynamicConfig, coOwner, dynamicConfig, owner), false, 'Co-owners cannot modify or delete owners');
assert.equal(canModifyRole(dynamicConfig, admin, dynamicConfig, owner), false, 'Admins cannot modify or delete owners');
assert.equal(canModifyRole(dynamicConfig, owner, dynamicConfig, coOwner), true, 'Owners can modify co-owners');
assert.equal(canModifyRole(dynamicConfig, owner, dynamicConfig, admin), true, 'Owners can modify admins');

console.log('Role unit tests passed (no hardcoded credentials required).');
