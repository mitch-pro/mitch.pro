  (function () {
  'use strict';
  if (window.self !== window.top) return;

  function getCook(n) {
    var m = document.cookie.match(new RegExp('(?:^|; )' + n.replace(/[.*+?^${}()|[\]\\]/g,'\\$&') + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : null;
  }

  var SID = getCook('studentId') || getCook('id');
  if (!SID) return;

  var _aiPref = {};
  try { _aiPref = JSON.parse(localStorage.getItem('_prefAI') || '{}'); } catch(e) {}
  if (_aiPref.enabled === false) return;
  var _personality = _aiPref.personality || 'friendly';

  var _open = false, _busy = false, _history = [];

  var ST = document.createElement('style');
  ST.textContent = [
    '#_ab{z-index:2147483630;width:28px;height:28px;',
    'border-radius:50%;border:1px solid rgba(255,255,255,0.1);cursor:pointer;opacity:0.7;',
    'background:rgba(10,10,10,0.4);color:#fff;box-shadow:none;',
    'font-size:10px;font-weight:900;letter-spacing:.02em;line-height:1;',
    'display:flex;align-items:center;justify-content:center;flex-shrink:0;',
    'transition:transform .15s,opacity .15s,background .15s;font-family:system-ui,sans-serif;}',
    '#_ab:hover{transform:scale(1.1);opacity:1;background:var(--t-ac, #8b5cf6);}',
    '#_ap{position:fixed;top:50px;right:12px;z-index:2147483629;width:310px;',
    'max-height:440px;display:flex;flex-direction:column;background:rgba(20,20,25,0.96);',
    'border:1px solid var(--t-bd, rgba(255,255,255,0.1));border-radius:14px;box-shadow:0 8px 40px rgba(0,0,0,.65);',
    'overflow:hidden;transform:translateY(10px) scale(.97);opacity:0;pointer-events:none;',
    'transition:transform .18s ease,opacity .18s ease;}',
    '#_ap.vis{transform:none;opacity:1;pointer-events:all;}',
    '#_ah{display:flex;align-items:center;padding:9px 13px;border-bottom:1px solid var(--t-bd, rgba(255,255,255,0.1));',
    'font-family:system-ui,sans-serif;font-size:.8rem;font-weight:700;color:var(--t-fg, #e8e6e3);gap:6px;}',
    '#_ah .logo{color:var(--t-ac, #8b5cf6);font-size:.75rem;font-weight:900;}',
    '#_ah .title{flex:1;}',
    '#_ahx{background:none;border:none;color:var(--t-fg2, #555);cursor:pointer;font-size:.9rem;',
    'padding:2px 5px;border-radius:4px;line-height:1;}#_ahx:hover{color:var(--t-fg, #fff);}',
    '#_am{flex:1;overflow-y:auto;padding:9px 11px;display:flex;flex-direction:column;',
    'gap:7px;font-family:system-ui,sans-serif;font-size:.79rem;line-height:1.55;min-height:60px;}',
    '._mb{max-width:88%;padding:6px 10px;border-radius:10px;word-break:break-word;}',
    '._mu{background:var(--t-bg2, #302e2b);color:var(--t-fg, #e8e6e3);align-self:flex-end;border-radius:10px 10px 2px 10px;white-space:pre-wrap;}',
    '._ma{background:var(--t-bg3, #272522);color:var(--t-fg2, #bababa);align-self:flex-start;border-radius:10px 10px 10px 2px;}',
    '._ma p{margin:0 0 .4em;}._ma p:last-child{margin:0;}',
    '._ma strong{color:var(--t-fg, #e8e6e3);font-weight:700;}',
    '._ma em{font-style:italic;}',
    '._ma code{background:rgba(0,0,0,0.3);color:var(--t-ac, #8b5cf6);padding:1px 4px;border-radius:3px;font-size:.88em;font-family:monospace;}',
    '._ma pre{background:rgba(0,0,0,0.3);padding:7px 9px;border-radius:6px;overflow-x:auto;margin:.3em 0;}',
    '._ma pre code{background:none;padding:0;color:var(--t-fg, #c8c5c0);font-size:.85em;}',
    '._ma ul,._ma ol{margin:.2em 0 .2em 1.2em;padding:0;}',
    '._ma li{margin:.1em 0;}',
    '._ma h1,._ma h2,._ma h3{color:var(--t-fg, #e8e6e3);font-weight:700;margin:.3em 0 .15em;line-height:1.2;}',
    '._ma h1{font-size:1em;}._ma h2{font-size:.95em;}._ma h3{font-size:.9em;}',
    '._ma a{color:var(--t-ac, #8b5cf6);text-decoration:underline;}',
    '._ma hr{border:none;border-top:1px solid var(--t-bd, #3d3a37);margin:.4em 0;}',
    '._merr{color:#f87171;font-size:.73rem;align-self:center;padding:3px 8px;',
    'background:rgba(239,68,68,.12);border-radius:6px;}',
    '._mnotice{color:#fbbf24;font-size:.69rem;align-self:center;padding:3px 8px;',
    'background:rgba(245,158,11,.08);border-radius:6px;}',
    '#_ai{display:flex;gap:6px;padding:8px 10px;border-top:1px solid var(--t-bd, #3d3a37);align-items:flex-end;}',
    '#_at{flex:1;resize:none;background:var(--t-bg3, #272522);border:1px solid var(--t-bd, #3d3a37);color:var(--t-fg, #e8e6e3);',
    'border-radius:8px;padding:6px 8px;font-size:.79rem;font-family:system-ui,sans-serif;',
    'outline:none;line-height:1.4;max-height:80px;overflow-y:auto;}',
    '#_at:focus{border-color:var(--t-ac, #8b5cf6);}',
    '#_as{background:var(--t-ac, #8b5cf6);color:#000;border:none;border-radius:8px;',
    'padding:6px 11px;font-size:.79rem;font-weight:800;cursor:pointer;flex-shrink:0;}',
    '#_as:disabled{opacity:.35;cursor:default;}'
  ].join('');
  document.head.appendChild(ST);

  var btn = document.createElement('button');
  btn.id = '_ab'; btn.title = 'mitch.pro Assistant'; btn.textContent = 'AI';
  btn.onclick = togglePanel;
  // Inject AI button into shared topbar, falling back to body
  (function injectAIBtn() {
    var topbar = document.getElementById('site-topbar');
    if (topbar) {
      topbar.insertBefore(btn, topbar.firstChild);
    } else {
      // topbar not ready yet — wait briefly for broadcast.js to create it
      setTimeout(function() {
        var tb = document.getElementById('site-topbar');
        if (tb) tb.insertBefore(btn, tb.firstChild);
        else document.body.appendChild(btn);
      }, 100);
    }
  })();
  var panel = document.createElement('div');
  panel.id = '_ap';
  panel.innerHTML =
    '<div id="_ah"><span class="logo">AI</span><span class="title">mitch.pro Assistant</span>' +
    '<button id="_ahx">x</button></div>' +
    '<div id="_am"></div>' +
    '<div id="_ai"><textarea id="_at" rows="1" placeholder="Ask anything..."></textarea>' +
    '<button id="_as">Send</button></div>';
  document.body.appendChild(panel);

      var msgs = panel.querySelector('#_am');
    var input = panel.querySelector('#_at');
    var sendBtn = panel.querySelector('#_as');
    var closeBtn = panel.querySelector('#_ahx');

    sendBtn.onclick = send;
    closeBtn.onclick = function() { _open = false; panel.classList.remove('vis'); };

  input.addEventListener('keydown', function(e) {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); e.stopPropagation(); send(); }
  });
  input.addEventListener('input', function() {
    this.style.height = 'auto';
    this.style.height = Math.min(this.scrollHeight, 80) + 'px';
  });

  function togglePanel() {
    _open = !_open;
    panel.classList.toggle('vis', _open);
    if (_open) setTimeout(function() { input.focus(); }, 180);
  }

  function _esc(s) {
    return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
  }

  function _md(text) {
    var lines = text.split('\n');
    var out = '', inCode = false, codeLang = '', codeBuf = '', inList = false, listType = '';

    function flushList() {
      if (!inList) return;
      out += '</' + listType + '>';
      inList = false; listType = '';
    }

    function inlineFormat(s) {
      s = s.replace(/`([^`]+)`/g, function(_,c){ return '<code>' + _esc(c) + '</code>'; });
      s = s.replace(/\*\*\*(.+?)\*\*\*/g, '<strong><em>$1</em></strong>');
      s = s.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
      s = s.replace(/\*(.+?)\*/g, '<em>$1</em>');
      s = s.replace(/~~(.+?)~~/g, '<del>$1</del>');
      s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
      return s;
    }

    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];
      var fence = line.match(/^```(\w*)$/);
      if (fence) {
        if (!inCode) {
          flushList();
          inCode = true; codeLang = fence[1]; codeBuf = '';
        } else {
          out += '<pre><code>' + _esc(codeBuf.replace(/\n$/, '')) + '</code></pre>';
          inCode = false; codeBuf = '';
        }
        continue;
      }
      if (inCode) { codeBuf += line + '\n'; continue; }

      var hm = line.match(/^(#{1,3})\s+(.+)/);
      if (hm) {
        flushList();
        var hl = hm[1].length;
        out += '<h' + hl + '>' + inlineFormat(_esc(hm[2])) + '</h' + hl + '>';
        continue;
      }
      if (/^---+$/.test(line.trim())) { flushList(); out += '<hr>'; continue; }

      var ulm = line.match(/^[\*\-]\s+(.+)/);
      if (ulm) {
        if (!inList || listType !== 'ul') { flushList(); out += '<ul>'; inList = true; listType = 'ul'; }
        out += '<li>' + inlineFormat(_esc(ulm[1])) + '</li>';
        continue;
      }
      var olm = line.match(/^\d+\.\s+(.+)/);
      if (olm) {
        if (!inList || listType !== 'ol') { flushList(); out += '<ol>'; inList = true; listType = 'ol'; }
        out += '<li>' + inlineFormat(_esc(olm[1])) + '</li>';
        continue;
      }

      flushList();
      if (line.trim() === '') { out += '<p>'; continue; }
      out += '<p>' + inlineFormat(_esc(line)) + '</p>';
    }
    if (inCode) out += '<pre><code>' + _esc(codeBuf) + '</code></pre>';
    flushList();
    out = out.replace(/(<p>)+/g, '');
    return out;
  }

  function addMsg(text, cls) {
    var d = document.createElement('div');
    d.className = '_mb ' + cls;
    if (cls === '_ma') {
      d.innerHTML = _md(text);
    } else {
      d.textContent = text;
    }
    msgs.appendChild(d);
    msgs.scrollTop = msgs.scrollHeight;
    return d;
  }

  async function send() {
    var text = input.value.trim();
    if (!text || _busy) return;
    input.value = ''; input.style.height = '';
    addMsg(text, '_mu');
    _history.push({role: 'user', text: text});
    _busy = true; sendBtn.disabled = true;
    var thinking = addMsg('...', '_ma');
    try {
      var r = await fetch('/api/ai', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        credentials: 'include',
        body: JSON.stringify({
          studentId: SID,
          prompt: text,
          history: _history.slice(-8),
          type: 'assistant',
          personality: _personality,
          privacy: !!_aiPref.privacy,
          page: window.location.href,
          pageHtml: (function() {
            try {
              var c = document.documentElement.cloneNode(true);
              c.querySelectorAll('script,style,link[rel="stylesheet"]').forEach(function(el){el.remove();});
              return c.outerHTML.slice(0, 20000);
            } catch(e) { return ''; }
          })()
        })
      });
      var d = await r.json();
      thinking.remove();
      if (!r.ok || d.error) {
        addMsg(d.error || 'Something went wrong.', '_merr');
        _history.pop();
      } else {
        if (d.degraded && d.notice) addMsg(d.notice, '_mnotice');
        addMsg(d.response, '_ma');
        _history.push({role: 'model', text: d.response});
        if (_history.length > 20) _history = _history.slice(-20);
      }
    } catch(e) {
      thinking.remove();
      addMsg('Network error.', '_merr');
      _history.pop();
    }
    _busy = false; sendBtn.disabled = false;
    input.focus();
  }
})();
