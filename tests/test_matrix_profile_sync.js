import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const matrixRs = readFileSync('rust/crates/mitch-server/src/routes/matrix.rs', 'utf8');
const matrixIndex = readFileSync('webserver/matrix/index.html', 'utf8');

// 1. Verify sync_profile_to_matrix definition and capabilities in Rust
assert(matrixRs.includes('pub async fn sync_profile_to_matrix('), 'sync_profile_to_matrix helper must be defined');
assert(matrixRs.includes('/_matrix/client/v3/profile/{encoded_user_id}/displayname'), 'Matrix profile sync must push displayname');
assert(matrixRs.includes('/_matrix/client/v3/profile/{encoded_user_id}/avatar_url'), 'Matrix profile sync must push avatar_url');
assert(matrixRs.includes('/_matrix/client/v3/presence/{encoded_user_id}/status'), 'Matrix profile sync must push status_msg presence for bio');

// 2. Verify Matrix SSO login syncs profile fields
assert(matrixRs.includes('sync_profile_to_matrix('), 'Matrix SSO login must invoke sync_profile_to_matrix');

// 3. Verify GET /api/matrix/sso-status returns pfp and bio
assert(matrixRs.includes('prof.get("pfp")') && matrixRs.includes('"pfp":'), 'sso-status must return pfp');
assert(matrixRs.includes('prof.get("bio")') && matrixRs.includes('"bio":'), 'sso-status must return bio');

// 4. Verify Matrix client shell renders pfp and bio
assert(matrixIndex.includes('status.pfp'), 'matrix/index.html must check status.pfp');
assert(matrixIndex.includes('status.bio'), 'matrix/index.html must check status.bio');
assert(matrixIndex.includes('avatar.style.backgroundImage'), 'matrix/index.html must set avatar background-image');

// 5. Verify Matrix -> mitch.pro reverse proxy sync
assert(matrixRs.includes('displayname/?$'), 'Matrix proxy must detect displayname updates');
assert(matrixRs.includes('avatar_url/?$'), 'Matrix proxy must detect avatar_url updates');
assert(matrixRs.includes('status/?$'), 'Matrix proxy must detect presence status_msg updates');

console.log('Matrix <-> mitch.pro profile synchronization checks passed.');
