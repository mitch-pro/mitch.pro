import { describe, test, expect } from 'bun:test';
import { readFileSync, existsSync } from 'fs';
import { join } from 'path';

describe('RJUHSD Bell Schedule Overrides Integration', () => {
  const dataDir = join(__dirname, '..', 'data');
  const overridesPath = join(dataDir, 'bell_overrides.json');

  test('bell_overrides.json exists and is valid JSON', () => {
    expect(existsSync(overridesPath)).toBe(true);
    const content = readFileSync(overridesPath, 'utf8');
    const data = JSON.parse(content);
    expect(typeof data).toBe('object');
    expect(data.date).toBeDefined();
    expect(data.schedule).toBeDefined();
    expect(Array.isArray(data.schedule.lunch1)).toBe(true);
    expect(Array.isArray(data.schedule.lunch2)).toBe(true);
    expect(data.schedule.lunch1.length).toBeGreaterThan(0);
    expect(data.schedule.lunch2.length).toBeGreaterThan(0);
  });

  test('calendar.js resolve correctly applies bell_overrides.json', () => {
    // Load schedule data & calendar resolver
    const scheduleJs = readFileSync(join(__dirname, '..', 'webserver/bell/schedule.js'), 'utf8');
    const calendarJs = readFileSync(join(__dirname, '..', 'webserver/rjuhsd-assets/calendar.js'), 'utf8');
    const override = JSON.parse(readFileSync(overridesPath, 'utf8'));

    const sandbox = { window: {}, document: { documentElement: { classList: { contains: () => false } } } };
    const runInSandbox = (code) => new Function('window', 'g', code)(sandbox.window, sandbox.window);

    runInSandbox(scheduleJs);
    runInSandbox(calendarJs);

    const resolve = sandbox.window.RJUHSD_CALENDAR.resolve;
    expect(typeof resolve).toBe('function');

    // Test resolving with override for Woodcreek on the override date
    const targetDate = override.date;
    const resolvedOverride = resolve('woodcreek', targetDate, [], 'auto', false, override);

    expect(resolvedOverride.inSession).toBe(true);
    expect(resolvedOverride.title).toBe(override.name || 'Special schedule');
    expect(resolvedOverride.lunch1.length).toBe(override.schedule.lunch1.length);
    expect(resolvedOverride.lunch1[0].name).toBe(override.schedule.lunch1[0].name);
    expect(resolvedOverride.lunch1[0].start).toBe(override.schedule.lunch1[0].start);
    expect(resolvedOverride.lunch1[0].end).toBe(override.schedule.lunch1[0].end);

    // Test non-override date falls back to regular calendar
    const otherDate = '2026-09-15'; // Tuesday
    const resolvedNormal = resolve('woodcreek', otherDate, [], 'auto', false, override);
    expect(resolvedNormal.title).toBe('Regular schedule');
    expect(resolvedNormal.lunch1[0].name).toBe('Period 1');
  });
});
