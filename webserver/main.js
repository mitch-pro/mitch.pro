const publicVapidKey = "BIuXi_VdGolze2Q3DgfsCjSVKWaxDMi2nkS6NkybXAOLuNu-8-LyeCMqC-daOxUsd2nnvr3QRfKgn-wSNNYs7iw";

function urlBase64ToUint8Array(base64String) {
  const padding = "=".repeat((4 - base64String.length % 4) % 4);
  const base64 = (base64String + padding)
    .replace(/-/g, "+")
    .replace(/_/g, "/");
  const rawData = window.atob(base64);
  return Uint8Array.from([...rawData].map(char => char.charCodeAt(0)));
}

(async () => {
  if ("serviceWorker" in navigator && "PushManager" in window) {
    // When a freshly deployed service worker takes over mid-session
    // (skipWaiting + claim), reload once so the page refetches assets through
    // the new worker — users never need Ctrl+Shift+R. Keyed by script URL so
    // each deploy reloads exactly once per tab.
    let seenSW;
    try { seenSW = JSON.parse(sessionStorage.getItem("sw-scripts") || "{}"); } catch (_) { seenSW = {}; }
    if (navigator.serviceWorker.controller) seenSW[navigator.serviceWorker.controller.scriptURL] = 1;
    navigator.serviceWorker.addEventListener("controllerchange", () => {
      const s = navigator.serviceWorker.controller;
      if (!s || seenSW[s.scriptURL]) return;
      seenSW[s.scriptURL] = 1;
      try { sessionStorage.setItem("sw-scripts", JSON.stringify(seenSW)); } catch (_) {}
      location.reload();
    });

    // ?v=11 busts any stale copy of the worker script itself; keep this in
    // sync with the app-shell.js registration so pages don't flip-flop workers.
    const sw = await navigator.serviceWorker.register("/sw.js?v=48", { scope: "/", updateViaCache: "none" });
    console.log("Service Worker registered");

    let subscription = await sw.pushManager.getSubscription();
    if (!subscription) {
      subscription = await sw.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlBase64ToUint8Array(publicVapidKey),
      });
      console.log("Subscribed for push");
    } else {
      console.log("Already subscribed");
    }

    await fetch("/subscribe", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(subscription),
    });
    console.log("Subscription sent to server");
  } else {
    console.warn("Push messaging isn't supported");
  }
})();

