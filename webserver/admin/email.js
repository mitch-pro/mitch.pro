let emailPreviewVersion = 0;
let emailPreviewTemplate = '';
let emailTestSending = false;

async function loadEmailTools() {
  if (!currentUser?.isAdmin || document.body.classList.contains('admin-locked')) return;
  const recipient = document.getElementById('email-test-recipient');
  if (recipient && !recipient.value && currentUser.email) {
    recipient.value = currentUser.email;
  }
  document.getElementById('email-connection').textContent = 'Checking mitch-mail-rs…';
  document.getElementById('email-senders').textContent = '';
  const preview = previewEmailTemplate();
  try {
    const response = await adminFetch('/api/admin/email/status');
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || 'Connection check failed.');
    document.getElementById('email-connection').textContent = data.connected
      ? 'Connected to mitch-mail-rs.' : 'Cannot reach mitch-mail-rs status. Check the service deployment.';
    if (data.connected) {
      document.getElementById('email-senders').textContent = Object.entries(data.senders)
        .map(([name, configured]) => `${name}: ${configured ? 'configured' : 'not configured'}`).join(' · ')
        + '. A test email checks SMTP acceptance.';
    }
  } catch (error) {
    document.getElementById('email-connection').textContent = error.message;
  }
  await preview;
}

async function previewEmailTemplate() {
  if (!currentUser?.isAdmin) return;
  const version = ++emailPreviewVersion;
  const template = document.getElementById('email-template').value;
  const button = document.getElementById('email-test-send');
  emailPreviewTemplate = '';
  button.disabled = true;
  document.getElementById('email-preview').srcdoc = '';
  document.getElementById('email-subject').textContent = '';
  if (!emailTestSending) setStatus('email-test-status', 'Loading preview…');
  try {
    const data = await postAdmin('/api/admin/email/preview', { template });
    if (version !== emailPreviewVersion) return;
    document.getElementById('email-preview').srcdoc = data.html;
    document.getElementById('email-subject').textContent = data.subject;
    emailPreviewTemplate = template;
    if (!emailTestSending) setStatus('email-test-status', '');
  } catch (error) {
    if (version === emailPreviewVersion) setStatus('email-test-status', error.message, 'err');
  } finally {
    if (version === emailPreviewVersion) button.disabled = emailTestSending || !emailPreviewTemplate;
  }
}

async function sendEmailTest() {
  if (emailTestSending || !emailPreviewTemplate || !currentUser?.isAdmin) return;
  const recipient = document.getElementById('email-test-recipient');
  const to = recipient ? recipient.value.trim() : '';
  if (!to) {
    setStatus('email-test-status', 'Enter a recipient email address.', 'err');
    recipient?.focus();
    return;
  }
  emailTestSending = true;
  const template = emailPreviewTemplate;
  const button = document.getElementById('email-test-send');
  button.disabled = true;
  setStatus('email-test-status', 'Sending test email…');
  try {
    const data = await postAdmin('/api/admin/email/test', { template, to });
    setStatus('email-test-status', 'SMTP accepted the test email for ' + data.to + '. Check the inbox or spam folder.', 'ok');
  } catch (error) {
    setStatus('email-test-status', error.message, 'err');
  } finally {
    emailTestSending = false;
    button.disabled = !emailPreviewTemplate;
  }
}
