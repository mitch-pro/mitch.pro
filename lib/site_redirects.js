export const RJUHSD_ORIGIN = 'https://rjuhsd.school';

export function bellScheduleRedirect(url, method = 'GET') {
  if (method !== 'GET' && method !== 'HEAD') return null;
  if (!/^\/(?:rjuhsd\/)?bell(?:\.html?|\/(?:index(?:\.html?)?\/?)?)?$/i.test(url.pathname)) return null;
  return RJUHSD_ORIGIN + '/' + url.search;
}
