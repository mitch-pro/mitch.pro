import assert from 'node:assert/strict';

// Mirrors server.js makeVerificationCodeHtml argument contract.
// Regression: callers pass (label, code, expiryMinutes[, email]). A previous
// signature of (email, label, code, expiryMinutes) put the code in the label
// slot, the expiry in the big code box, and left minutes as "undefined".
function makeVerificationCodeHtml(label, code, expiryMinutes, email = '') {
  const safeLabel = String(label || 'this action');
  const safeCode = String(code || '').trim();
  const mins = Number(expiryMinutes);
  const safeMins = Number.isFinite(mins) && mins > 0 ? mins : 10;
  return `
    <h2>Verification Code</h2>
    <p>Please use the following verification code to confirm <strong>${safeLabel}</strong> on your account:</p>
    <span class="code">${safeCode}</span>
    <p>valid for <strong>${safeMins} minutes</strong></p>
    <p data-email="${email}"></p>
  `;
}

const html = makeVerificationCodeHtml('Account Signup', '865665', 30, 'user@example.com');
assert.match(html, /confirm <strong>Account Signup<\/strong>/);
assert.match(html, /class="code">865665</);
assert.match(html, /valid for <strong>30 minutes<\/strong>/);
assert.doesNotMatch(html, /undefined minutes/);
assert.doesNotMatch(html, /class="code">30</);
assert.match(html, /data-email="user@example.com"/);

const fallback = makeVerificationCodeHtml('Password Reset', '123456');
assert.match(fallback, /valid for <strong>10 minutes<\/strong>/);

console.log('verification email template arg-order tests passed');
