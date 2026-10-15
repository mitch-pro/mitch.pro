//! Per-host PWA manifests — verbatim from server.js /manifest.json routes
//! (serialized with serde_json's preserve_order to match JS.stringify order).

pub fn pickle_manifest() -> serde_json::Value {
    serde_json::json!({
        "name": "Sexy Pickle Club",
        "short_name": "Pickle Club",
        "description": "The premier pickle community & cellar chat — installable and works offline.",
        "id": "/",
        "start_url": "/?utm_source=pwa",
        "scope": "/",
        "display": "standalone",
        "display_override": ["standalone", "minimal-ui"],
        "background_color": "#171918",
        "theme_color": "#171918",
        "orientation": "any",
        "categories": ["social", "games", "productivity"],
        "icons": [
            { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
            { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
            { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
        ],
        "shortcuts": [
            { "name": "Matrix Chat", "short_name": "Matrix", "description": "Open encrypted Matrix chat", "url": "/matrix/?utm_source=pwa-shortcut", "icons": [{ "src": "/icon-192.png", "sizes": "192x192" }] },
            { "name": "The Barrel", "short_name": "Barrel", "description": "Live pickle lounge", "url": "/barrel/?utm_source=pwa-shortcut", "icons": [{ "src": "/icon-192.png", "sizes": "192x192" }] },
            { "name": "Bulletin", "short_name": "Bulletin", "description": "Official announcements", "url": "/bulletin/?utm_source=pwa-shortcut", "icons": [{ "src": "/icon-192.png", "sizes": "192x192" }] }
        ]
    })
}

pub fn rjuhsd_manifest() -> serde_json::Value {
    serde_json::json!({
        "name": "RJUHSD Hub",
        "short_name": "RJUHSD",
        "description": "The Roseville Joint Union High School District hub: bell schedules, news, and student tools.",
        "id": "/",
        "start_url": "/?utm_source=pwa",
        "scope": "/",
        "display": "standalone",
        "display_override": ["standalone", "minimal-ui"],
        "background_color": "#0c0809",
        "theme_color": "#0c0809",
        "orientation": "any",
        "categories": ["education", "productivity"],
        "icons": [
            { "src": "/rjuhsd-assets/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
            { "src": "/rjuhsd-assets/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
            { "src": "/rjuhsd-assets/maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
        ],
        "shortcuts": [
            { "name": "Bell Schedule", "short_name": "Bells", "description": "Live RJUHSD bell schedules", "url": "/?utm_source=pwa-shortcut#schedule-panel", "icons": [{ "src": "/rjuhsd-assets/icon-192.png", "sizes": "192x192" }] },
            { "name": "Student Tools", "short_name": "Tools", "description": "Study timer, finals countdown, and more", "url": "/?utm_source=pwa-shortcut#tools", "icons": [{ "src": "/rjuhsd-assets/icon-192.png", "sizes": "192x192" }] }
        ]
    })
}
