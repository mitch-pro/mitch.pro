// Draws the page's text on <canvas> elements instead of keeping it in the DOM.
// Text nodes are swapped for canvases (as they appear, too), and the strings are
// kept in a WeakMap rather than in any attribute. Font and colour come from the
// canvas's computed style, so the stylesheet and theme still control them. Form
// controls (the search box, the sort menu) can't hold canvases and stay as text.
(() => {
  'use strict';
  const texts = new WeakMap();
  // Repaint a canvas whenever its box changes size (layout settling, rotation,
  // a neighbour resizing). Without this a label painted while the layout was
  // still narrow stayed clipped until some unrelated update repainted everything.
  const sizeWatcher = new ResizeObserver(entries => entries.forEach(entry => paint(entry.target)));
  const measure = document.createElement('canvas').getContext('2d');
  const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'CANVAS', 'INPUT', 'TEXTAREA', 'SELECT', 'OPTION', 'IFRAME', 'TITLE', 'svg', 'SVG']);

  const fontOf = style => `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;

  // options.block: fill the container's width (names that get cut off with an
  // ellipsis); otherwise the canvas is as wide as its text.
  // options.wrap: a paragraph that wraps onto several lines at the container's width.
  function create(text, options = {}) {
    const canvas = document.createElement('canvas');
    canvas.className = options.wrap ? 'wrap-canvas' : options.block ? 'title-canvas' : 'text-canvas';
    canvas.setAttribute('aria-hidden', 'true');
    texts.set(canvas, String(text));
    sizeWatcher.observe(canvas);
    queue();
    return canvas;
  }

  function layout(canvas) {
    if (!canvas.classList.contains('text-canvas')) return;
    const style = getComputedStyle(canvas);
    measure.font = fontOf(style);
    const lineHeight = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.3;
    canvas.style.width = Math.ceil(measure.measureText(texts.get(canvas)).width) + 1 + 'px';
    canvas.style.height = Math.ceil(lineHeight) + 'px';
  }

  function wrapLines(ctx, text, width) {
    const lines = [];
    let line = '';
    for (const word of text.split(' ')) {
      const next = line ? line + ' ' + word : word;
      if (line && ctx.measureText(next).width > width) { lines.push(line); line = word; } else line = next;
    }
    if (line) lines.push(line);
    return lines;
  }

  function paintWrapped(canvas, text) {
    const style = getComputedStyle(canvas);
    const lineHeight = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.4;
    measure.font = fontOf(style);
    const width = canvas.getBoundingClientRect().width;
    if (!width) return;
    const lines = wrapLines(measure, text, width);
    canvas.style.height = Math.ceil(lines.length * lineHeight) + 'px';
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(lines.length * lineHeight * ratio);
    const ctx = canvas.getContext('2d');
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.font = fontOf(style);
    ctx.fillStyle = style.color;
    ctx.textBaseline = 'middle';
    const align = ['center', 'right'].includes(style.textAlign) ? style.textAlign : 'left';
    ctx.textAlign = align;
    const x = align === 'center' ? width / 2 : align === 'right' ? width : 0;
    lines.forEach((line, i) => ctx.fillText(line, x, (i + 0.5) * lineHeight));
  }

  function paint(canvas) {
    const text = texts.get(canvas);
    if (text !== undefined && canvas.classList.contains('wrap-canvas')) { paintWrapped(canvas, text); return; }
    layout(canvas);
    const rect = canvas.getBoundingClientRect();
    if (text === undefined || !rect.width || !rect.height) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(rect.width * ratio);
    canvas.height = Math.round(rect.height * ratio);
    const ctx = canvas.getContext('2d');
    const style = getComputedStyle(canvas);
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.font = fontOf(style);
    ctx.fillStyle = style.color;
    ctx.textBaseline = 'middle';
    let shown = text;
    if (ctx.measureText(shown).width > rect.width + 1) {
      let low = 0, high = text.length;
      while (low < high) {
        const mid = Math.ceil((low + high) / 2);
        if (ctx.measureText(text.slice(0, mid).trimEnd() + '…').width <= rect.width) low = mid; else high = mid - 1;
      }
      shown = text.slice(0, low).trimEnd() + '…';
    }
    ctx.fillText(shown, 0, rect.height / 2);
  }

  let queued = false;
  function queue() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      document.querySelectorAll('canvas.title-canvas, canvas.text-canvas, canvas.wrap-canvas').forEach(paint);
    });
  }

  function skipped(node) {
    for (let el = node.parentElement; el; el = el.parentElement) {
      if (SKIP.has(el.tagName) || el.hasAttribute('data-plain-text')) return true;
    }
    return false;
  }

  function convertNode(node) {
    const raw = node.nodeValue;
    if (!raw || !raw.trim() || skipped(node)) return;
    const parts = [];
    if (/^\s/.test(raw)) parts.push(document.createTextNode(' '));
    parts.push(create(raw.replace(/\s+/g, ' ').trim(), { wrap: node.parentElement?.tagName === 'P' }));
    if (/\s$/.test(raw)) parts.push(document.createTextNode(' '));
    node.replaceWith(...parts);
  }

  function convert(root) {
    if (root.nodeType === Node.TEXT_NODE) { convertNode(root); return; }
    if (root.nodeType !== Node.ELEMENT_NODE) return;
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    nodes.forEach(convertNode);
  }

  window.MitchCanvasText = { create };

  const start = () => {
    convert(document.body);
    new MutationObserver(records => {
      for (const record of records) {
        record.addedNodes.forEach(convert);
        if (record.type === 'characterData') convertNode(record.target);
      }
    }).observe(document.body, { childList: true, subtree: true, characterData: true });
  };
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start, { once: true });

  window.addEventListener('resize', queue);
  window.addEventListener('themechange', queue);
  new MutationObserver(queue).observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-theme', 'style'] });
  if (document.fonts?.ready) document.fonts.ready.then(queue);
  // A font that finishes loading later (a theme font, say) changes text widths.
  document.fonts?.addEventListener?.('loadingdone', queue);
})();
