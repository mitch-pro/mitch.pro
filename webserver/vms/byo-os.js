// Bring Your Own OS — client-side chunked upload + VM creation. Kept as its
// own file rather than folded into portal.js: this is a self-contained
// upload flow (file picker, chunking, progress, resume) that doesn't touch
// portal.js's own computer-list rendering, and a full page reload after
// successful creation is simplest way to pick that list back up.
(function () {
  const CHUNK_SIZE = 16 * 1024 * 1024; // 16MB — server's hard ceiling is 24MB.
  const MAX_ISO_BYTES = 6 * 1024 * 1024 * 1024; // 6GB
  const SELF_REFUND_MAX_BYTES = 128 * 1024 * 1024; // matches byo_os.rs's SELF_REFUND_MAX_BYTES

  const $ = (id) => document.getElementById(id);
  const dialog = () => $('byo-os-dialog');

  function fmtBytes(n) {
    if (!n) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB'];
    let i = 0;
    let v = n;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
  }

  function setStatus(text, isError) {
    const el = $('byo-os-status');
    if (!el) return;
    el.textContent = text || '';
    el.style.color = isError ? '#f87171' : '#94a3b8';
  }

  async function api(url, opts) {
    const res = await fetch(url, { credentials: 'same-origin', ...opts });
    let data = null;
    try {
      data = await res.json();
    } catch (_) {
      /* non-JSON response body, fall through with data = null */
    }
    if (!res.ok) {
      const err = new Error((data && data.error) || `Request failed (${res.status})`);
      err.data = data;
      throw err;
    }
    return data;
  }

  // Tracked so the create button can warn before permanently deleting an
  // existing computer — /api/vm/byo-os/create itself now deletes whatever
  // computer the user already has (template or BYO-OS) before building the
  // new one, the same way the template path's "Delete & Recreate" does.
  let hasExistingComputer = false;

  async function refreshIsoState() {
    try {
      const [isoData, computersData] = await Promise.all([
        api('/api/vm/byo-os/iso', { method: 'GET' }),
        api('/api/vm/computers', { method: 'GET' }).catch(() => null),
      ]);
      const has = isoData && isoData.iso;
      $('byo-os-no-iso').classList.toggle('is-hidden', !!has);
      $('byo-os-has-iso').classList.toggle('is-hidden', !has);
      if (has) {
        $('byo-os-iso-name').textContent = isoData.iso.filename;
        $('byo-os-iso-meta').textContent =
          `${fmtBytes(isoData.iso.sizeBytes)} · uploaded ${new Date(isoData.iso.uploadedAt).toLocaleString()}`;
        const undersized = Number(isoData.iso.sizeBytes) > 0 && Number(isoData.iso.sizeBytes) < SELF_REFUND_MAX_BYTES;
        $('byo-os-refund-btn').classList.toggle('is-hidden', !undersized);
        const reportBtn = $('byo-os-report-btn');
        if (reportBtn) {
          reportBtn.disabled = !!isoData.iso.reported;
          reportBtn.textContent = isoData.iso.reported ? 'Already reported' : 'Report an issue';
        }
      }
      hasExistingComputer = !!(
        computersData && Array.isArray(computersData.computers) && computersData.computers.length > 0
      );
      const createBtn = $('byo-os-create-btn');
      if (createBtn) {
        createBtn.textContent = hasExistingComputer
          ? 'Delete current computer & create from this ISO →'
          : 'Create computer from this ISO →';
      }
    } catch (e) {
      setStatus(e.message, true);
    }
  }

  let uploading = false;
  let cancelRequested = false;

  async function uploadChunkWithRetry(uploadId, index, blob) {
    let lastErr = null;
    for (let attempt = 1; attempt <= 3; attempt++) {
      try {
        const res = await fetch(
          `/api/vm/byo-os/iso/chunk?uploadId=${encodeURIComponent(uploadId)}&index=${index}`,
          {
            method: 'POST',
            credentials: 'same-origin',
            headers: { 'Content-Type': 'application/octet-stream', 'X-Mitch-Requested-With': '1' },
            body: blob,
          },
        );
        if (!res.ok) throw new Error(`chunk ${index} failed (HTTP ${res.status})`);
        return;
      } catch (err) {
        lastErr = err;
        if (attempt < 3) await new Promise((r) => setTimeout(r, 1000 * attempt));
      }
    }
    throw lastErr;
  }

  async function uploadFile(file) {
    if (uploading || !file) return;
    if (file.size <= 0 || file.size > MAX_ISO_BYTES) {
      setStatus('ISO must be larger than 0 bytes and no more than 6GB.', true);
      return;
    }
    uploading = true;
    cancelRequested = false;
    const progressWrap = $('byo-os-progress-wrap');
    const progressBar = $('byo-os-progress-bar');
    const progressText = $('byo-os-progress-text');
    progressWrap.classList.remove('is-hidden');
    $('byo-os-upload-btn').disabled = true;
    try {
      setStatus('Starting upload…');
      const start = await api('/api/vm/byo-os/iso/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ filename: file.name, totalSizeBytes: file.size, chunkSizeBytes: CHUNK_SIZE }),
      });
      const uploadId = start.uploadId;
      const totalChunks = start.totalChunks;
      localStorage.setItem('byo_os_upload_id', uploadId);

      // Resume-aware: if this uploadId already has chunks on disk (a retry
      // after a failed chunk, or the same file re-selected after a page
      // reload with the uploadId still in localStorage), skip re-sending
      // them instead of starting over.
      let alreadyReceived = new Set();
      try {
        const status = await api(
          `/api/vm/byo-os/iso/status?uploadId=${encodeURIComponent(uploadId)}`,
          { method: 'GET' },
        );
        alreadyReceived = new Set(status.receivedChunks || []);
      } catch (_) {
        /* fresh upload, nothing to resume */
      }

      for (let i = 0; i < totalChunks; i++) {
        if (cancelRequested) {
          setStatus('Upload cancelled.');
          return;
        }
        if (!alreadyReceived.has(i)) {
          const startByte = i * CHUNK_SIZE;
          const endByte = Math.min(startByte + CHUNK_SIZE, file.size);
          await uploadChunkWithRetry(uploadId, i, file.slice(startByte, endByte));
        }
        const pct = Math.round(((i + 1) / totalChunks) * 100);
        progressBar.style.width = `${pct}%`;
        progressText.textContent =
          `Uploading… ${pct}% (${fmtBytes(Math.min((i + 1) * CHUNK_SIZE, file.size))} / ${fmtBytes(file.size)})`;
      }

      setStatus('Handing off to the computer service… large ISOs can take a minute or two here.');
      await api('/api/vm/byo-os/iso/complete', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ uploadId }),
      });
      localStorage.removeItem('byo_os_upload_id');
      setStatus('ISO uploaded.');
      progressWrap.classList.add('is-hidden');
      await refreshIsoState();
    } catch (e) {
      setStatus(e.message, true);
    } finally {
      uploading = false;
      $('byo-os-upload-btn').disabled = false;
    }
  }

  // Unlike uploadFile, there's no chunking/progress here — the server
  // relays the download straight into Proxmox in one request, so this is
  // just a single long-running fetch from the browser's perspective.
  async function fetchFromUrl(rawUrl) {
    const urlBtn = $('byo-os-url-btn');
    urlBtn.disabled = true;
    setStatus('Fetching that ISO and handing it to the computer service… large files can take a while here.');
    try {
      await api('/api/vm/byo-os/iso/from-url', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ url: rawUrl }),
      });
      setStatus('ISO fetched.');
      await refreshIsoState();
    } catch (e) {
      setStatus(e.message, true);
    } finally {
      urlBtn.disabled = !$('byo-os-url-input').value.trim();
    }
  }

  function init() {
    const openBtns = [$('byo-os-open-btn'), $('byo-os-open-btn-header')].filter(Boolean);
    if (!openBtns.length || !dialog()) return;
    const closeBtn = $('byo-os-close-btn');
    const fileInput = $('byo-os-file-input');
    const uploadBtn = $('byo-os-upload-btn');
    const urlInput = $('byo-os-url-input');
    const urlBtn = $('byo-os-url-btn');
    const deleteBtn = $('byo-os-delete-btn');
    const createBtn = $('byo-os-create-btn');
    const refundBtn = $('byo-os-refund-btn');
    const reportBtn = $('byo-os-report-btn');

    openBtns.forEach((btn) => {
      btn.addEventListener('click', () => {
        setStatus('');
        refreshIsoState();
        dialog().showModal();
      });
    });
    closeBtn.addEventListener('click', () => {
      cancelRequested = true;
      dialog().close();
    });
    fileInput.addEventListener('change', () => {
      uploadBtn.disabled = !fileInput.files || !fileInput.files[0];
    });
    uploadBtn.addEventListener('click', () => {
      const file = fileInput.files && fileInput.files[0];
      if (file) uploadFile(file);
    });
    urlInput.addEventListener('input', () => {
      urlBtn.disabled = !urlInput.value.trim();
    });
    urlBtn.addEventListener('click', () => {
      const rawUrl = urlInput.value.trim();
      if (rawUrl) fetchFromUrl(rawUrl);
    });
    deleteBtn.addEventListener('click', async () => {
      if (!confirm('Remove your stored ISO? You will need to upload (and pay) again to use BYO-OS.')) return;
      try {
        await api('/api/vm/byo-os/iso/delete', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
          body: '{}',
        });
        await refreshIsoState();
      } catch (e) {
        setStatus(e.message, true);
      }
    });
    refundBtn.addEventListener('click', async () => {
      if (!confirm('Refund this ISO as a wrong link? It will be removed and your coins returned.')) return;
      refundBtn.disabled = true;
      try {
        const data = await api('/api/vm/byo-os/iso/refund-undersized', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
          body: '{}',
        });
        setStatus(`Refunded ${data.refunded} coins.`);
        await refreshIsoState();
      } catch (e) {
        setStatus(e.message, true);
      } finally {
        refundBtn.disabled = false;
      }
    });
    reportBtn.addEventListener('click', async () => {
      const note = prompt('Optional: what went wrong? (leave blank to just flag it)') || '';
      reportBtn.disabled = true;
      try {
        const data = await api('/api/vm/byo-os/iso/report-issue', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
          body: JSON.stringify({ note }),
        });
        setStatus(data.message || 'Reported.');
        await refreshIsoState();
      } catch (e) {
        setStatus(e.message, true);
        reportBtn.disabled = false;
      }
    });
    createBtn.addEventListener('click', async () => {
      if (hasExistingComputer) {
        const ok = confirm(
          'This will permanently delete your current computer and all its files, then create a new one from this ISO. This cannot be undone. Continue?',
        );
        if (!ok) return;
      }
      createBtn.disabled = true;
      setStatus(
        hasExistingComputer
          ? 'Deleting your current computer and creating a new one… this can take a minute.'
          : 'Creating your computer… this can take a minute.',
      );
      try {
        await api('/api/vm/byo-os/create', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
          body: '{}',
        });
        setStatus('Computer created — reloading…');
        setTimeout(() => location.reload(), 1200);
      } catch (e) {
        setStatus(e.message, true);
        createBtn.disabled = false;
      }
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init, { once: true });
  } else {
    init();
  }
})();
