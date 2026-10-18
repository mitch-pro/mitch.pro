import { PLAYER_URL } from "./blooket-connection.mjs";

/**
 * FlareSolverr Client & Clearance Manager
 * Connects to the local loopback FlareSolverr service (default: http://127.0.0.1:8191/v1)
 * to solve Cloudflare Turnstile/Managed Challenges and retrieve clearance cookies & User-Agents.
 */

export const BLOOKET_PLAYER_URL = PLAYER_URL;

function sameSiteValue(val) {
  const s = String(val || "").toLowerCase();
  if (s === "strict") return "Strict";
  if (s === "none" || s === "no_restriction") return "None";
  return "Lax";
}

export function toPlaywrightCookies(rawCookies, targetHost = "play.blooket.com") {
  if (!Array.isArray(rawCookies)) return [];
  return rawCookies.flatMap((c) => {
    const name = String(c?.name || "").trim();
    const value = String(c?.value || "").trim();
    if (!name) return [];

    let domain = String(c?.domain || targetHost).trim().toLowerCase();
    // Keep cookies scoped to blooket.com or play.blooket.com
    const host = domain.replace(/^\./, "");
    if (host !== "blooket.com" && !host.endsWith(".blooket.com")) return [];

    const expires = Number(c?.expires || c?.expiry);
    const cookie = {
      name,
      value,
      domain,
      path: String(c?.path || "/").startsWith("/") ? String(c?.path || "/") : "/",
      httpOnly: Boolean(c?.httpOnly),
      secure: c?.secure !== false,
      sameSite: sameSiteValue(c?.sameSite)
    };

    if (Number.isFinite(expires) && expires > 0) {
      cookie.expires = expires;
    }

    return [cookie];
  });
}

export class FlareSolverrClient {
  constructor({
    endpoint = process.env.FLARESOLVERR_URL || "http://127.0.0.1:8191/v1",
    targetUrl = BLOOKET_PLAYER_URL,
    ttlMs = 25 * 60 * 1000 // Cache for 25 minutes
  } = {}) {
    this.endpoint = endpoint;
    this.targetUrl = targetUrl;
    this.ttlMs = ttlMs;

    this.cachedSession = null;
    this.solvingPromise = null;
    this.status = "idle"; // "idle" | "solving" | "ready" | "error" | "fallback"
    this.lastError = null;
    this.enabled = true;
  }

  getCachedClearance() {
    if (!this.cachedSession) return null;
    const age = Date.now() - this.cachedSession.solvedAt;
    if (age > this.ttlMs) {
      return null;
    }
    return this.cachedSession;
  }

  async getClearance({ forceRefresh = false, maxTimeout = 45000 } = {}) {
    if (!this.enabled) return null;

    if (!forceRefresh) {
      const existing = this.getCachedClearance();
      if (existing) return existing;
    }

    // Single-flight deduplication: if already solving, wait for the active promise
    if (this.solvingPromise) {
      return this.solvingPromise;
    }

    this.solvingPromise = (async () => {
      this.status = "solving";
      this.lastError = null;
      const startTime = Date.now();

      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), maxTimeout + 5000);

      try {
        const res = await fetch(this.endpoint, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            cmd: "request.get",
            url: this.targetUrl,
            maxTimeout: Math.max(15000, maxTimeout),
            waitInSeconds: 2
          }),
          signal: controller.signal
        });

        if (!res.ok) {
          throw new Error(`FlareSolverr returned HTTP ${res.status}`);
        }

        const data = await res.json().catch(() => ({}));
        if (data?.status !== "ok" || !data?.solution) {
          throw new Error(data?.message || "FlareSolverr did not return a valid solution.");
        }

        const cookies = toPlaywrightCookies(data.solution.cookies || []);
        const userAgent = String(data.solution.userAgent || "").trim();
        const latencyMs = Date.now() - startTime;
        const version = String(data.version || "3.5.0");

        if (!cookies.length) {
          throw new Error("FlareSolverr returned no cookies for Blooket.");
        }

        this.cachedSession = {
          cookies,
          userAgent,
          solvedAt: Date.now(),
          latencyMs,
          version,
          status: Number(data.solution.status) || 200,
          url: data.solution.url || this.targetUrl
        };

        this.status = "ready";
        this.lastError = null;
        return this.cachedSession;
      } catch (err) {
        this.status = "fallback";
        this.lastError = err.message;
        // Don't blow up the entire caller - return null so callers gracefully fallback to direct browser launch
        return null;
      } finally {
        clearTimeout(timeoutId);
        this.solvingPromise = null;
      }
    })();

    return this.solvingPromise;
  }

  getStatus() {
    const cached = this.getCachedClearance();
    const ageSeconds = cached ? Math.round((Date.now() - cached.solvedAt) / 1000) : null;

    return {
      enabled: this.enabled,
      status: this.status,
      endpoint: this.endpoint,
      cached: Boolean(cached),
      cookiesCount: cached?.cookies?.length || 0,
      cookieNames: cached?.cookies?.map(c => c.name) || [],
      ageSeconds,
      ttlSeconds: Math.round(this.ttlMs / 1000),
      latencyMs: cached?.latencyMs || 0,
      version: cached?.version || "3.5.0",
      userAgent: cached?.userAgent ? "Synced" : "Default",
      lastError: this.lastError
    };
  }
}
