let emailPreviewVersion = 0;

async function loadEmailTools() {
  if (!currentUser?.isAdmin || document.body.classList.contains('admin-locked')) return;
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
        + '. SMTP delivery is checked when you send.';
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
  document.getElementById('email-preview').srcdoc = '';
  document.getElementById('email-subject').textContent = '';
  setStatus('email-test-status', 'Loading preview…');
  try {
    const data = await postAdmin('/api/admin/email/preview', { template });
    if (version !== emailPreviewVersion) return;
    document.getElementById('email-preview').srcdoc = data.html;
    document.getElementById('email-subject').textContent = data.subject;
    setStatus('email-test-status', '');
  } catch (error) {
    if (version === emailPreviewVersion) setStatus('email-test-status', error.message, 'err');
  }
}
