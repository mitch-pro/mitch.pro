(function () {
  'use strict';
  var emblem = document.querySelector('.hello-emblem');
  if (!emblem) return;
  var inView = false;
  var reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  function updateMotion() {
    emblem.dataset.motion = inView && !document.hidden && !reducedMotion.matches ? 'active' : 'paused';
  }
  if ('IntersectionObserver' in window) {
    var observer = new IntersectionObserver(function (entries) {
      inView = entries[0].isIntersecting;
      updateMotion();
    });
    observer.observe(emblem);
  }
  document.addEventListener('visibilitychange', updateMotion);
  reducedMotion.addEventListener('change', updateMotion);
})();
