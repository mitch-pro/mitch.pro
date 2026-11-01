import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const CATALOG_PATH = join(process.cwd(), 'webserver', 'game-portal', 'games.json');

export async function fetchAndImportGames(amount = 500) {
  const url = `https://gamemonetize.com/rssfeed.php?format=json&category=All&amount=${amount}`;
  console.log(`Fetching ${amount} games with icons from GameMonetize...`);
  
  const res = await fetch(url, {
    headers: {
      'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36',
      'Accept': 'application/json'
    }
  });

  if (!res.ok) throw new Error(`HTTP ${res.status}: ${res.statusText}`);
  const data = await res.json();
  if (!Array.isArray(data)) throw new Error('Invalid JSON response: expected an array');

  console.log(`Received ${data.length} games. Merging into portal catalog...`);
  const catalog = JSON.parse(readFileSync(CATALOG_PATH, 'utf8'));
  const seenUrls = new Set();
  const seenTitles = new Set();

  catalog.links.forEach(section => {
    (section.games || []).forEach(g => {
      seenTitles.add(String(g[0]).trim().toLowerCase());
      if (g[2]) seenUrls.add(String(g[2]).trim());
    });
  });

  let added = 0;
  const sectionMap = new Map();
  catalog.links.forEach(s => sectionMap.set(s.title, s));

  data.forEach(item => {
    if (!item.url || !item.title) return;
    const cleanUrl = String(item.url).trim();
    const cleanTitle = String(item.title).replace(/\s*\bunblocked\b/gi, '').replace(/\s+:/g, ':').trim();
    const titleKey = cleanTitle.toLowerCase();

    if (seenUrls.has(cleanUrl) || seenTitles.has(titleKey)) return;
    seenUrls.add(cleanUrl);
    seenTitles.add(titleKey);

    const firstLetter = cleanTitle.charAt(0).toUpperCase();
    const sectionTitle = /^[A-Z]$/.test(firstLetter) ? firstLetter : '#';

    let section = sectionMap.get(sectionTitle);
    if (!section) {
      section = { title: sectionTitle, games: [] };
      sectionMap.set(sectionTitle, section);
      catalog.links.push(section);
    }

    const description = String(item.description || '').slice(0, 160).replace(/<[^>]*>/g, '').trim();
    const thumbUrl = item.thumb ? String(item.thumb).trim() : '';

    section.games.push([
      cleanTitle,
      thumbUrl,
      cleanUrl,
      description,
      ''
    ]);
    added++;
  });

  catalog.links.sort((a, b) => a.title.localeCompare(b.title));
  catalog.links.forEach(s => {
    s.games.sort((a, b) => a[0].localeCompare(b[0]));
  });

  writeFileSync(CATALOG_PATH, JSON.stringify(catalog, null, 4));
  console.log(`Successfully added ${added} new games with icons to ${CATALOG_PATH}`);
  return added;
}

if (process.argv[1]?.endsWith('import_games_with_icons.js')) {
  const amount = parseInt(process.argv[2], 10) || 500;
  fetchAndImportGames(amount).catch(err => {
    console.error('Failed to import games:', err.message);
    process.exit(1);
  });
}
