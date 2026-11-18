(function () {
  'use strict';
  const style = document.createElement('style');
  style.textContent='.account-suggestions{position:fixed;z-index:2147483600;padding:6px;border:1px solid var(--t-bd);border-radius:12px;background:var(--t-bg2);box-shadow:0 14px 40px #0003;overflow:auto;max-height:290px}.account-suggestions[hidden]{display:none}.account-suggestions button{display:block;width:100%;padding:11px 12px;border:0!important;border-radius:8px!important;background:transparent!important;color:var(--t-fg)!important;text-align:left;font:inherit;cursor:pointer}.account-suggestions button[aria-selected=true],.account-suggestions button:hover{background:var(--t-bg3)!important}.account-suggestions strong{display:block;font-size:13px}.account-suggestions small{display:block;color:var(--t-fg2);font-size:11px;margin-top:3px;overflow-wrap:anywhere}.account-suggestions p{padding:8px;color:var(--t-fg2);font-size:12px}';
  document.head.append(style);
  let serial=0;
  async function search(query,signal) {
    const response=await fetch('/api/admin/account-search?q='+encodeURIComponent(query),{credentials:'same-origin',cache:'no-store',signal,headers:{'X-Admin-Passphrase':sessionStorage.getItem('admin_passphrase') || ''}});
    const result=await response.json();
    if(!response.ok)throw new Error([401,403].includes(response.status)?'Unlock the staff panel to search accounts.':'Account search unavailable.');
    return result.accounts || [];
  }
  function attach(input) {
    if(input.dataset.accountSearchReady)return;
    input.dataset.accountSearchReady='true';
    const matrix=input.id==='matrix-target-user';
    if(input.type==='email')input.type='text';
    if(input.type!=='search')input.placeholder='Name, username, or email';
    input.autocomplete='off';input.setAttribute('role','combobox');input.setAttribute('aria-autocomplete','list');input.setAttribute('aria-expanded','false');
    const box=document.createElement('div');box.className='account-suggestions';box.id='account-suggestions-'+(++serial);box.setAttribute('role','listbox');box.setAttribute('aria-label','Matching accounts');box.hidden=true;document.body.append(box);input.setAttribute('aria-controls',box.id);
    let controller,timer,items=[],active=-1,choosing=false;
    function close(){clearTimeout(timer);controller?.abort();box.hidden=true;input.setAttribute('aria-expanded','false');input.removeAttribute('aria-activedescendant');}
    function open(){const rect=input.getBoundingClientRect();box.style.left=Math.max(12,Math.min(rect.left,innerWidth-260))+'px';box.style.width=Math.min(Math.max(rect.width,248),innerWidth-24)+'px';box.style.top=Math.min(rect.bottom+5,innerHeight-180)+'px';box.style.maxHeight=Math.max(100,innerHeight-Math.min(rect.bottom+5,innerHeight-180)-42)+'px';box.hidden=false;input.setAttribute('aria-expanded','true');}
    function choose(index){const account=items[index];if(!account)return;choosing=true;input.value=matrix?(account.matrixUserId || account.username):account.email;input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));choosing=false;close();input.focus();}
    function highlight(index){active=index;Array.from(box.querySelectorAll('button')).forEach((button,i)=>button.setAttribute('aria-selected',String(i===index)));const option=box.querySelectorAll('button')[index];if(option){input.setAttribute('aria-activedescendant',option.id);option.scrollIntoView({block:'nearest'});}}
    async function refresh(){const query=input.value.trim();controller?.abort();if(query.length<2){close();return;}controller=new AbortController();try{items=await search(query,controller.signal);if(query!==input.value.trim() || document.activeElement!==input)return;box.replaceChildren();active=-1;items.forEach((account,index)=>{const button=document.createElement('button');button.type='button';button.id=box.id+'-'+index;button.setAttribute('role','option');button.setAttribute('aria-selected','false');button.tabIndex=-1;const title=document.createElement('strong');title.textContent=account.displayName || account.username || account.email;const detail=document.createElement('small');detail.textContent='@'+account.username+' · '+account.email;button.append(title,detail);button.addEventListener('pointerdown',event=>event.preventDefault());button.onclick=()=>choose(index);box.append(button);});if(!items.length){const empty=document.createElement('p');empty.textContent='No matching accounts.';box.append(empty);}open();}catch(error){if(error.name==='AbortError' || document.activeElement!==input)return;box.replaceChildren();const message=document.createElement('p');message.textContent=error.message;box.append(message);open();}}
    input.addEventListener('input',()=>{if(choosing)return;clearTimeout(timer);controller?.abort();timer=setTimeout(refresh,180);});
    input.addEventListener('focus',()=>{if(input.value.trim().length>=2)refresh();});
    input.addEventListener('blur',()=>setTimeout(close,120));
    input.addEventListener('keydown',event=>{if(event.key==='Escape'){close();return;}if(box.hidden||!items.length)return;if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();highlight((active+(event.key==='ArrowDown'?1:-1)+items.length)%items.length);}else if(event.key==='Enter'&&active>=0){event.preventDefault();choose(active);}});
    addEventListener('resize',close);addEventListener('scroll',event=>{if(!box.contains(event.target))close();},true);
  }
  function bind(){document.querySelectorAll('input[type=email],input[data-account-search],#mod-profile-query,#matrix-target-user,#ban-email,#ban-search,#fleet-search,#search').forEach(attach);}
  bind();new MutationObserver(bind).observe(document.body,{childList:true,subtree:true});
  window.MitchAccountSearch={search};
})();
