export const RJUHSD_ORIGIN = 'https://rjuhsd.school';
export const BLOOKET_BOT_ORIGIN = 'https://woodcreek.site';

export function bellScheduleRedirect(url, method = 'GET') {
  if (method !== 'GET' && method !== 'HEAD') return null;
  if (!/^\/(?:rjuhsd\/)?bell(?:\.html?|\/(?:index(?:\.html?)?\/?)?)?$/i.test(url.pathname)) return null;
  return RJUHSD_ORIGIN + '/' + url.search;
}

export function blooketBotRedirect(url, method = 'GET') {
  if (method !== 'GET' && method !== 'HEAD') return null;
  if (url.pathname === '/blooket-bot/') return null;
  if (!/^\/blooket-bot(?:\.html?|\/(?:index(?:\.html?)?\/?)?)?$/i.test(url.pathname)) return null;
  return '/blooket-bot/' + url.search;
}
