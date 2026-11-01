(() => {
  const $ = id => document.getElementById(id);
  const headers = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
  const errors = {
    too_soon: 'A code was just sent. Wait a minute before asking for another.',
    wrong_code: 'That code isn\'t right.',
    too_many_attempts: 'Too many wrong codes. Send a new code.',
    expired: 'That code expired. Send a new one.',
    no_request: 'No code is waiting. Send a new one.',
    passphrase_too_short: 'Use at least 8 characters.',
    no_email: 'This account has no email address.'
  };

  function say(id, message, isError) {
    const el = $(id);
    el.textContent = message || '';
    el.dataset.error = isError ? 'true' : 'false';
  }

  async function post(path, body) {
    const response = await fetch(path, { method: 'POST', credentials: 'same-origin', headers, body: JSON.stringify(body || {}) });
    const data = await response.json().catch(() => ({}));
    return { ok: response.ok, status: response.status, data };
  }

  async function sendCode() {
    const button = $('send-code');
    button.disabled = true;
    say('request-status', 'Sending…');
    try {
      const { ok, status, data } = await post('/api/admin/passphrase-reset/request');
      if (ok) {
        say('request-status', 'Code sent to ' + data.sentTo + '. It works once for 15 minutes.');
        $('step-confirm').hidden = false;
        $('reset-code').focus();
      } else if (status === 401 || status === 403) {
        say('request-status', 'Sign in with an admin account first.', true);
      } else {
        say('request-status', errors[data.error] || 'Could not send the code. Try again.', true);
      }
    } catch (_) {
      say('request-status', 'Could not send the code. Try again.', true);
    }
    button.disabled = false;
  }

  $('send-code').addEventListener('click', sendCode);
  $('resend').addEventListener('click', sendCode);

  $('confirm-form').addEventListener('submit', async event => {
    event.preventDefault();
    const passphrase = $('reset-passphrase').value;
    if (passphrase !== $('reset-passphrase-again').value) {
      say('confirm-status', 'The two passphrases don\'t match.', true);
      return;
    }
    const button = $('confirm-button');
    button.disabled = true;
    say('confirm-status', 'Saving…');
    try {
      const { ok, data } = await post('/api/admin/passphrase-reset/confirm', { code: $('reset-code').value, passphrase });
      if (ok) {
        // Drop any stale passphrase cached by other admin pages.
        try { sessionStorage.clear(); } catch (_) {}
        say('confirm-status', 'Passphrase updated. You can unlock the admin tools with it now.');
        $('confirm-form').reset();
      } else {
        say('confirm-status', errors[data.error] || 'Could not reset the passphrase. Try again.', true);
      }
    } catch (_) {
      say('confirm-status', 'Could not reset the passphrase. Try again.', true);
    }
    button.disabled = false;
  });
})();
