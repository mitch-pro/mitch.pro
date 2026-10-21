(() => {
  const id = new URLSearchParams(location.search).get('message');
  if (!id) return;
  let done = false;
  const observer = new MutationObserver(() => {
    if (done) return;
    const message = document.querySelector('.msg[data-id="' + CSS.escape(id) + '"]');
    if (!message) return;
    done = true; observer.disconnect();
    message.scrollIntoView({block:'center',behavior:'instant'});
    message.animate([{outline:'2px solid var(--t-ac)',outlineOffset:'3px'},{outline:'2px solid transparent',outlineOffset:'3px'}],{duration:2500});
  });
  observer.observe(document.documentElement,{childList:true,subtree:true});
  setTimeout(() => observer.disconnect(),30000);
})();
