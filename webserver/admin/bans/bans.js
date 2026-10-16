(function () {
  'use strict';
  const $ = id => document.getElementById(id);
  let bans = [], busy = false, setup = false, timedBansReady = false, pendingBan = null;
  function headers() {
    const result = {'Content-Type':'application/json','X-Mitch-Requested-With':'1'};
    const pass = sessionStorage.getItem('admin_passphrase');
    if (pass) result['X-Admin-Passphrase'] = pass;
    return result;
  }
  async function request(url, options = {}) {
    const response = await fetch(url, {credentials:'same-origin',cache:'no-store',...options,headers:headers()});
    const data = await response.json();
    if (!response.ok) {
      const error = new Error(data.error || 'Request failed. Try again.');
      error.code = data.error;
      throw error;
    }
    return data;
  }
  function status(id, text, error = false) {
    $(id).textContent = text;
    $(id).dataset.error = String(error);
  }
  function setBusy(value) {
    busy = value;
    $('bans-workspace').querySelectorAll('button').forEach(button => {button.disabled=value;});
    $('ban-form').querySelector('button').disabled=value||!timedBansReady;
  }
  function showLock() {
    $('bans-workspace').hidden = true;
    $('passphrase-lock').hidden = false;
    $('lock-heading').textContent = setup ? 'Set your admin passphrase' : 'Unlock ban management';
    $('unlock-button').textContent = setup ? 'Save and unlock' : 'Unlock';
    $('admin-passphrase').autocomplete = setup ? 'new-password' : 'current-password';
    status('access-status', 'Admin access verified. Enter your passphrase to continue.');
  }
  function render() {
    const query = $('ban-search').value.trim().toLowerCase();
    const filtered = bans.filter(ban => [ban.email,ban.reason,ban.by].some(value => String(value || '').toLowerCase().includes(query)));
    $('bans-summary').textContent = query ? filtered.length+' of '+bans.length+' accounts' : bans.length+' banned '+(bans.length===1?'account':'accounts');
    const list = $('ban-list');
    list.replaceChildren();
    if (!filtered.length) {
      const empty = document.createElement('p');
      empty.className = 'bans-empty';
      empty.textContent = query ? 'No bans match that search.' : 'No banned accounts.';
      list.append(empty);
    }
    filtered.forEach(ban => {
      const row = document.createElement('article');row.className='ban-entry';row.setAttribute('role','listitem');
      const copy = document.createElement('div');copy.className='ban-entry-copy';
      const email = document.createElement('strong');email.textContent=ban.email;
      const reason = document.createElement('p');reason.textContent=ban.reason || 'No reason recorded.';
      const meta = document.createElement('small');
      const date = new Date(ban.bannedAt);
      meta.textContent = [ban.by ? 'By '+ban.by : '', Number.isFinite(date.getTime()) ? date.toLocaleString() : ''].filter(Boolean).join(' · ');
      const expiry = document.createElement('small');
      expiry.className = 'ban-expiry';
      expiry.textContent = ban.expiresAt ? 'Ends '+new Date(ban.expiresAt).toLocaleString() : 'Permanent ban';
      copy.append(email,reason,meta,expiry);
      if (ban.cleanup) {
        const cleanup=document.createElement('small');
        cleanup.textContent=ban.cleanup.complete ? 'Chat messages removed; uploaded media blocked.' : ban.cleanup.error ? 'Chat history hidden. Server cleanup is retrying.' : 'Chat history hidden. Server cleanup is queued.';
        copy.append(cleanup);
      }
      const restore = document.createElement('button');restore.type='button';restore.textContent='Restore access';restore.disabled=busy;
      restore.addEventListener('click',()=>changeBan(false,ban.email,'','list-status'));
      row.append(copy,restore);list.append(row);
    });
  }
  async function loadBans() {
    const data = await request('/api/admin/bans');
    bans = (data.bans || []).map(ban => ({...ban,cleanup:data.chatCleanup?.[ban.email]}));
    const wasReady = timedBansReady;
    timedBansReady = data.supportsTimedBans === true && data.supportsChatCleanup === true;
    $('ban-form').querySelector('button').disabled = busy || !timedBansReady;
    if (!timedBansReady) status('ban-status','Ban management is updating. Use Refresh in a moment to continue.');
    else if (!wasReady) status('ban-status','');
    render();
    $('passphrase-lock').hidden = true;
    $('bans-workspace').hidden = false;
    status('access-status','Admin access verified.');
  }
  async function changeBan(ban, email, reason, output, duration) {
    if (busy || (ban && !timedBansReady)) return;
    setBusy(true);status(output,ban?'Saving ban…':'Restoring access…');
    try {
      const data = await request(ban?'/api/admin/ban-account':'/api/admin/unban-account',{method:'POST',body:JSON.stringify({email,reason,...(ban?{duration}:{})})});
      if (data.pending) throw new Error('This action requires admin approval.');
      status(output,ban?'Banned '+data.targetEmail+'.':(data.existed?'Restored access for ':'No account ban found for ')+data.targetEmail+'.');
      if (ban) $('ban-form').reset();
      try { await loadBans(); } catch (_) { status('list-status','The action succeeded, but the list could not refresh. Use Refresh to try again.',true); }
    } catch (error) {
      status(output,error.message,true);
      if (['invalid_passphrase','passphrase_not_configured'].includes(error.code)) {setup=error.code==='passphrase_not_configured';showLock();}
    } finally {setBusy(false);}
  }
  $('ban-form').addEventListener('submit',event=>{
    event.preventDefault();
    if (busy || !timedBansReady) return;
    const email=$('ban-email').value.trim(),reason=$('ban-reason').value.trim(),duration=$('ban-duration').value;
    if (!reason) {status('ban-status','Enter a reason for the ban.',true);$('ban-reason').focus();return;}
    pendingBan={email,reason,duration};
    $('confirm-email').textContent=email;
    $('confirm-duration').textContent=$('ban-duration').selectedOptions[0].textContent;
    $('confirm-reason').textContent=reason;
    $('ban-confirm').returnValue='cancel';
    $('ban-confirm').showModal();
  });
  $('ban-confirm').addEventListener('close',()=>{
    const ban=pendingBan;pendingBan=null;
    if ($('ban-confirm').returnValue==='ban'&&ban) changeBan(true,ban.email,ban.reason,'ban-status',ban.duration);
  });
  $('restore-form').addEventListener('submit',event=>{event.preventDefault();changeBan(false,$('restore-email').value.trim(),'','restore-status');});
  $('ban-search').addEventListener('input',render);
  $('refresh-bans').addEventListener('click',async()=>{
    if (busy) return;
    setBusy(true);status('list-status','Refreshing…');
    try {await loadBans();status('list-status','');} catch(error) {status('list-status',error.message,true);if(['invalid_passphrase','passphrase_not_configured'].includes(error.code)){setup=error.code==='passphrase_not_configured';showLock();}}
    finally {setBusy(false);}
  });
  $('passphrase-form').addEventListener('submit',async event=>{
    event.preventDefault();$('unlock-button').disabled=true;status('unlock-status','Checking…');
    const pass=$('admin-passphrase').value;
    try {
      const response=await fetch('/api/admin/passphrase-status',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','X-Mitch-Requested-With':'1','X-Admin-Passphrase':pass},body:JSON.stringify({passphrase:pass})});
      const data=await response.json();
      if(!response.ok)throw new Error(data.error==='invalid_passphrase'?'Incorrect passphrase.':data.error || 'Could not unlock ban management.');
      sessionStorage.setItem('admin_passphrase',pass);$('admin-passphrase').value='';
      await loadBans();status('unlock-status','');
    } catch(error) {status('unlock-status',error.message,true);} finally {$('unlock-button').disabled=false;}
  });
  (async()=>{
    try {
      const me=await request('/api/me');
      if(!me.isAdmin&&!me.isOwner&&!me.isCoOwner){status('access-status','Admin access is required to manage bans.',true);return;}
      const config=await request('/api/admin/passphrase-status');setup=!config.set;
      if(!setup&&sessionStorage.getItem('admin_passphrase')) {
        try {await loadBans();return;} catch(error) {if(!['invalid_passphrase','passphrase_not_configured'].includes(error.code))throw error;}
      }
      showLock();
    } catch(error) {status('access-status',error.message,true);}
  })();
}());
