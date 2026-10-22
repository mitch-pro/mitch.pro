(() => {
  'use strict';
  // Curated from publisher rankings, not collected player activity. Source
  // dates and the selection method are documented in docs/game-discovery.md.
  const titles = [
    'Run 3', 'Snow Rider 3D', 'Retro Bowl', 'Subway Surfers', 'Drift Boss',
    'Moto X3M', 'Stickman Hook', 'Eggy Car', 'Tiny Fishing',
    'Fireboy and Watergirl 1', 'Cookie Clicker', 'Monkey Mart', 'OvO',
    'Drive Mad', 'Drift Hunters', 'Paper.io 2', 'Temple Run 2',
    "Papa's Pizzeria", "Papa's Freezeria", 'Bloxorz', 'Snake', '2048',
    'Slope', 'Basketball Stars', 'Basket Random', 'Basket Bros',
    'EaglercraftX (Minecraft 1.8.8)', 'Geometry Dash', 'Tunnel Rush',
    'Crossy Road', 'Duck Life', 'Happy Wheels', 'Vex 3', 'Rooftop Snipers',
    'Little Alchemy', 'Bloons Tower Defense', 'The Impossible Quiz'
  ];
  const key = title => String(title || '').toLowerCase().replace(/[^a-z0-9]/g, '');
  const ranks = new Map(titles.map((title, index) => [key(title), index]));
  function rank(title) { return ranks.get(key(title)) ?? Number.MAX_SAFE_INTEGER; }
  function compare(a, b) {
    const difference = rank(a.title) - rank(b.title);
    if (difference) return difference;
    if (key(a.title) === key(b.title)) {
      const local = game => String(game.url || '').startsWith('/games/') ? 0 : game.external ? 2 : 1;
      return local(a) - local(b);
    }
    return a.title.localeCompare(b.title);
  }
  globalThis.MitchGameDiscovery = Object.freeze({ key, rank, compare, titles: Object.freeze(titles) });
})();
