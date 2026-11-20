/* Site style choice: mitch.pro (the default) or Tylerium. See docs/ui-styles.md.

   - window.MitchUiStyle.current() / .save(style) for Preferences.
   - With data-ask="1" on its <script> tag (the server adds it for signed-in
     accounts that have never chosen), this also opens the chooser: previews
     of both styles, mitch.pro selected. The answer is saved on the account
     (POST /api/me/ui-style) and the page reloads if the style changed.

   The previews are small drawings of each homepage built from each style's
   palette and fonts, so they follow light/dark mode without loading either
   page. */
(function () {
  'use strict';
  var script = document.currentScript;
  var ask = !!(script && script.dataset.ask === '1');
  var STYLES = ['mitch', 'tylerium'];

  function current() {
    return document.documentElement.getAttribute('data-ui') === 'tylerium' ? 'tylerium' : 'mitch';
  }

  function save(style) {
    if (STYLES.indexOf(style) < 0) return Promise.reject(new Error('Unknown style'));
    return fetch('/api/me/ui-style', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
      body: JSON.stringify({ style: style })
    }).then(function (response) {
      if (!response.ok) throw new Error('Could not save');
      return response.json();
    });
  }

  function status() {
    return fetch('/api/me/ui-style', { credentials: 'same-origin', cache: 'no-store' })
      .then(function (response) { return response.ok ? response.json() : null; })
      .catch(function () { return null; });
  }

  window.MitchUiStyle = { current: current, save: save, status: status };

  if (!ask || window.top !== window) return;

  var CSS = [
    '#ui-style-dialog{width:min(760px,calc(100vw - 32px));max-height:calc(100dvh - 32px);padding:0;border:1px solid var(--t-bd);border-radius:var(--t-radius,10px);background:var(--t-bg2);color:var(--t-fg);font-family:var(--t-font);box-shadow:0 18px 60px rgba(0,0,0,.35);overflow:auto}',
    '#ui-style-dialog::backdrop{background:rgba(0,0,0,.55)}',
    '#ui-style-dialog form{display:grid;gap:18px;padding:clamp(18px,3vw,28px)}',
    '#ui-style-dialog h2{margin:0;font:700 1.45rem/1.2 var(--t-font);letter-spacing:-.02em}',
    '#ui-style-dialog .ui-style-sub{margin:6px 0 0;color:var(--t-fg2);font-size:.95rem}',
    '.ui-style-options{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px;margin:0;padding:0;border:0}',
    '.ui-style-legend{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap}',
    '.ui-style-option{position:relative;display:grid;gap:10px;padding:10px;border:2px solid var(--t-bd);border-radius:var(--t-radius,10px);background:var(--t-bg);cursor:pointer}',
    '.ui-style-option:hover{border-color:var(--t-bda)}',
    '.ui-style-option:has(input:checked){border-color:var(--t-ac)}',
    '.ui-style-option:has(input:focus-visible){outline:2px solid var(--t-ac);outline-offset:3px}',
    '.ui-style-option input{position:absolute;opacity:0;pointer-events:none}',
    '.ui-style-name{display:flex;align-items:center;gap:8px;padding:2px 4px;font-weight:700}',
    '.ui-style-name::before{content:"";flex:none;width:16px;height:16px;border:2px solid var(--t-bda);border-radius:50%;box-sizing:border-box}',
    '.ui-style-option:has(input:checked) .ui-style-name::before{border:5px solid var(--t-ac)}',
    '.ui-style-name small{color:var(--t-fg2);font-weight:500}',
    '.ui-style-actions{display:flex;align-items:center;justify-content:flex-end;gap:12px}',
    '.ui-style-actions p{margin:0 auto 0 0;color:var(--t-fg2);font-size:.9rem}',
    '.ui-style-actions button{min-height:42px;padding:0 22px;border:0;border-radius:var(--t-radius-sm,7px);background:var(--t-ac);color:var(--t-ac-ink,#fff);font:700 1rem var(--t-font);cursor:pointer}',
    '.ui-style-actions button:hover{background:var(--t-ac2,var(--t-ac))}',
    '.ui-style-actions button:disabled{opacity:.6;cursor:default}',
    /* Previews: each sets its own palette, in light and dark. */
    '.uip{--b:#f6f7f3;--p:#fff;--f:#202622;--m:#657062;--a:#f75383;--l:#dce2d8;position:relative;aspect-ratio:16/10;overflow:hidden;border-radius:6px;background:var(--b);color:var(--f);font:600 9px/1.2 "DM Sans",system-ui,sans-serif;pointer-events:none;user-select:none}',
    'html:not(.theme-light) .uip{--b:#17191b;--p:#222527;--f:#f4f5f1;--m:#a6adaf;--a:#ff6f99;--l:#363a3c}',
    '.uip-ty{--b:#f3f6fa;--p:#fff;--f:#172536;--m:#485d73;--a:#245c8c;--l:#52678366;background:var(--b) url("/assets/clouds-blue.webp") center/cover;font-family:"Pixelify Sans",system-ui,sans-serif}',
    'html:not(.theme-light) .uip-ty{--b:#000;--p:#111;--f:#f0f0f0;--m:#d0d0d0;--a:#9acbfa;--l:#303030;background-image:url("/assets/clouds-dark.webp")}',
    '.uip *{box-sizing:border-box}',
    '.uip-bar{display:flex;align-items:center;gap:6%;height:13%;padding:0 5%;border-bottom:1px solid var(--l);background:var(--p)}',
    '.uip-ty .uip-bar{width:90%;height:12%;margin:4% auto 0;border:1px solid var(--l);border-radius:5px}',
    '.uip-mark{display:flex;align-items:center;gap:4px;font-weight:700;letter-spacing:-.03em}',
    '.uip-ty .uip-mark{font-family:"JetBrains Mono",monospace;font-weight:500;letter-spacing:-.06em}',
    '.uip-mark i{width:9px;height:9px;border-radius:2px;background:var(--a)}',
    '.uip-mark b{font-weight:inherit;color:var(--a)}',
    '.uip-nav{display:flex;gap:7%;flex:1;color:var(--m);font-weight:500}',
    '.uip-nav span:first-child{color:var(--a)}',
    '.uip-body{display:grid;gap:4%;padding:4% 5%;height:87%}',
    '.uip-ty .uip-body{height:auto;padding:3% 5%;gap:7px}',
    '.uip-card{border:1px solid var(--l);border-radius:5px;background:var(--p)}',
    '.uip-hello{display:flex;align-items:center;justify-content:space-between;padding:0 6%;font-size:20px;font-weight:700;letter-spacing:-.04em}',
    '.uip-hello span{font-family:cursive,"DM Sans";font-weight:400;color:var(--a)}',
    '.uip-emblem{width:30px;height:30px;border-radius:7px;background:linear-gradient(135deg,var(--a),color-mix(in srgb,var(--a) 40%,var(--p)));transform:rotate(-8deg)}',
    '.uip-tiles{display:grid;grid-template-columns:repeat(3,1fr);gap:6px}',
    '.uip-tiles .uip-card{display:grid;align-content:end;padding:6px;color:var(--m)}',
    '.uip-tiles .uip-card::before{content:"";display:block;width:12px;height:12px;margin-bottom:5px;border-radius:3px;background:color-mix(in srgb,var(--a) 70%,transparent)}',
    '.uip-classic .uip-body{grid-template-rows:1.25fr 1fr}',
    '.uip-friends{display:flex;gap:5px;align-items:center;padding:4px 6px;color:var(--m)}',
    '.uip-friends i{width:11px;height:11px;border-radius:50%;background:color-mix(in srgb,var(--a) 55%,var(--l))}',
    '.uip-welcome{display:flex;align-items:center;gap:8px;padding:8px 8px;font-size:15px}',
    '.uip-welcome .uip-emblem{width:20px;height:20px;border-radius:5px}',
    '.uip-welcome time{margin-left:auto;font-family:"JetBrains Mono",monospace;font-size:11px;color:var(--m)}',
    '.uip-games{display:grid;grid-template-columns:1.4fr 1fr 1fr 1fr;gap:5px}',
    '.uip-games .uip-card{height:34px;padding:5px;color:var(--m)}',
    '.uip-games .uip-card:first-child{color:var(--a);border-color:color-mix(in srgb,var(--a) 50%,var(--l))}',
    '@media (max-width:560px){.ui-style-options{grid-template-columns:1fr}.uip{aspect-ratio:16/9}}'
  ].join('\n');

  var PREVIEW_MITCH = '<div class="uip uip-classic" aria-hidden="true">' +
    '<div class="uip-bar"><span class="uip-mark"><i></i><span>mitch<b>.pro</b></span></span><span class="uip-nav"><span>Home</span><span>VM Lab</span><span>Chat</span><span>Games</span></span></div>' +
    '<div class="uip-body"><div class="uip-card uip-hello"><span>hello</span><span class="uip-emblem"></span></div>' +
    '<div class="uip-tiles"><div class="uip-card">Games</div><div class="uip-card">Chat</div><div class="uip-card">VM Lab</div></div></div></div>';

  var PREVIEW_TYLERIUM = '<div class="uip uip-ty" aria-hidden="true">' +
    '<div class="uip-bar"><span class="uip-mark"><span>mitch<b>.pro</b></span></span><span class="uip-nav"><span>Home</span><span>Games</span><span>Chat</span><span>Rankings</span></span></div>' +
    '<div class="uip-body"><div class="uip-card uip-friends"><i></i><i></i><i></i>Friends</div>' +
    '<div class="uip-card uip-welcome"><span class="uip-emblem"></span>hello<time>12:30</time></div>' +
    '<div class="uip-games"><div class="uip-card">Games</div><div class="uip-card">Matrix</div><div class="uip-card">Linux</div><div class="uip-card">Movies</div></div></div></div>';

  function open() {
    if (document.getElementById('ui-style-dialog') || typeof HTMLDialogElement !== 'function') return;
    var style = document.createElement('style');
    style.textContent = CSS;
    document.head.appendChild(style);

    var dialog = document.createElement('dialog');
    dialog.id = 'ui-style-dialog';
    dialog.setAttribute('aria-labelledby', 'ui-style-title');
    dialog.innerHTML =
      '<form method="dialog">' +
        '<div><h2 id="ui-style-title">Choose a site style</h2>' +
        '<p class="ui-style-sub">You can change this later in Preferences.</p></div>' +
        '<fieldset class="ui-style-options"><legend class="ui-style-legend">Site style</legend>' +
          '<label class="ui-style-option">' + PREVIEW_MITCH +
            '<input type="radio" name="ui-style" value="mitch" checked>' +
            '<span class="ui-style-name">mitch.pro <small>(recommended)</small></span></label>' +
          '<label class="ui-style-option">' + PREVIEW_TYLERIUM +
            '<input type="radio" name="ui-style" value="tylerium">' +
            '<span class="ui-style-name">Tylerium</span></label>' +
        '</fieldset>' +
        '<div class="ui-style-actions"><p id="ui-style-status" role="status"></p><button type="submit">Continue</button></div>' +
      '</form>';
    document.body.appendChild(dialog);

    var form = dialog.querySelector('form');
    var button = form.querySelector('button[type="submit"]');
    var note = dialog.querySelector('#ui-style-status');
    // A choice is required: Escape doesn't close it.
    dialog.addEventListener('cancel', function (event) { event.preventDefault(); });
    form.addEventListener('submit', function (event) {
      event.preventDefault();
      var picked = form.querySelector('input[name="ui-style"]:checked');
      var choice = picked ? picked.value : 'mitch';
      button.disabled = true;
      note.textContent = '';
      save(choice).then(function () {
        if (choice !== current()) { location.reload(); return; }
        dialog.close();
        dialog.remove();
      }).catch(function () {
        button.disabled = false;
        note.textContent = "Couldn't save your choice. Try again.";
      });
    });
    dialog.showModal();
    var checked = form.querySelector('input:checked');
    if (checked) checked.focus();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', open, { once: true });
  else open();
})();
