//! The entertainment portal's catalog, served a page at a time.
//!
//! The catalog (`data/portal_catalog.json`, about 8 MB, 25k entries) lives only
//! on the server and is not in the web root: the browser never downloads it.
//! The portal page asks for just what is on screen: the featured row, a page of
//! the list, the saved or recent items it has ids for, or one random item.
//!
//! Ids are the entry's position among valid entries, the same numbering the
//! page used when it built the list itself, so saved and recent ids stay valid.
//! The URL mapping, category guess and popularity order are ports of the old
//! in-page code; the popularity order (curated from publisher rankings, see
//! `docs/game-discovery.md`) now lives in [`FEATURED_TITLES`].
//!
//! Endpoints (all GET, public):
//! - `/api/portal/summary`: `{total, categories: [{name, count}], featured}`
//! - `/api/portal/list?cat=&q=&sort=popular|alphabetical&offset=&limit=`
//! - `/api/portal/items?ids=1,2,3`: those items, in that order
//! - `/api/portal/random`: `{item}`

use axum::http::Method;
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::state::AppState;

/// Popularity order for the featured row and the "popular first" sort.
const FEATURED_TITLES: &[&str] = &[
    "Run 3",
    "Snow Rider 3D",
    "Retro Bowl",
    "Subway Surfers",
    "Drift Boss",
    "Moto X3M",
    "Retro Console Emulator",
    "Stickman Hook",
    "Eggy Car",
    "Tiny Fishing",
    "Fireboy and Watergirl 1",
    "Cookie Clicker",
    "Monkey Mart",
    "OvO",
    "Drive Mad",
    "Drift Hunters",
    "Paper.io 2",
    "Temple Run 2",
    "Papa's Pizzeria",
    "Papa's Freezeria",
    "Bloxorz",
    "Snake",
    "2048",
    "Slope",
    "Basketball Stars",
    "Basket Random",
    "Basket Bros",
    "EaglercraftX (Minecraft 1.8.8)",
    "Geometry Dash",
    "Tunnel Rush",
    "Crossy Road",
    "Duck Life",
    "Happy Wheels",
    "Vex 3",
    "Rooftop Snipers",
    "Little Alchemy",
    "Bloons Tower Defense",
    "The Impossible Quiz",
];

const CATEGORIES: &[&str] = &[
    "Arcade",
    "Action",
    "Racing",
    "Sports",
    "Multiplayer",
    "Puzzle",
    "Retro",
    "Casual",
    "Horror",
];

const DEFAULT_LIMIT: usize = 60;
const MAX_LIMIT: usize = 100;
const MAX_IDS: usize = 60;
const MAX_QUERY_CHARS: usize = 100;

pub struct Item {
    id: usize,
    title: String,
    category: &'static str,
    image: String,
    url: String,
    open_source: bool,
    external: bool,
    /// Lower-case title, description and section, for search.
    search: String,
    /// Title with everything but a-z and 0-9 removed.
    key: String,
    rank: usize,
}

/// The proxied game hosts, short names, with the host's own prefix.
const GAME_PREFIXES: &[&str] = &["/prox/luma/", "/prox/calculated2/", "/prox/gamemonetize/"];

/// Where a proxied game lives: which proxy, its directory on that host and its
/// entry file. `/prox/g/<id>/...` is served from here (see `proxy::game_proxy`).
pub struct GameDir {
    pub prefix: &'static str,
    /// Directory on the host, with a trailing slash, e.g. `snowrider3d/`.
    pub dir: String,
    /// The entry file in that directory, "" when the game opens at the directory.
    pub file: String,
    /// The query string the entry address carries, with its `?`, or "".
    pub query: String,
}

fn split_game_url(url: &str) -> Option<GameDir> {
    let prefix = GAME_PREFIXES.iter().find(|p| url.starts_with(**p))?;
    let rest = &url[prefix.len()..];
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, format!("?{q}")),
        None => (rest, String::new()),
    };
    let (dir, file) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    Some(GameDir {
        prefix,
        dir: dir.to_string(),
        file: file.to_string(),
        query,
    })
}

impl Item {
    /// What the page is told to open for this game. Proxied games are addressed
    /// by id (`/prox/g/<id>/<file>`) so the game's name is not in its address;
    /// the server maps it back ([`game_dir`]). Local and external games keep
    /// their address.
    fn game_ref(&self) -> String {
        match split_game_url(&self.url) {
            Some(g) => format!("/prox/g/{}/{}{}", self.id, g.file, g.query),
            None => self.url.clone(),
        }
    }

    /// What the page is told to load for this item's art. Art that is served
    /// from this site is addressed by id (`/prox/i/<id>`) so no game name
    /// appears in an image address; the server maps the id back (see
    /// [`image_uri`]). Art on another host is left as is.
    fn image_ref(&self) -> String {
        if self.image.starts_with('/') {
            format!("/prox/i/{}", self.id)
        } else {
            self.image.clone()
        }
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "category": self.category,
            "image": self.image_ref(),
            "url": self.game_ref(),
            "openSource": self.open_source,
            "external": self.external,
        })
    }
}

pub struct Catalog {
    items: Vec<Item>,
    /// Indexes into `items`, best first.
    popular: Vec<usize>,
    alphabetical: Vec<usize>,
    featured: Vec<usize>,
}

fn key_of(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

fn plain_text(raw: &str) -> String {
    if raw.contains('<') || raw.contains('&') {
        return super::dayboard::strip_html(raw);
    }
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn image_url(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("/img/games/") {
        return format!("/game-portal/icons/{rest}");
    }
    if let Some(rest) = raw.strip_prefix("/img/gamems/") {
        return format!("/game-portal/icons/{rest}");
    }
    if raw.starts_with("/game-portal/icons/") {
        return raw.to_string();
    }
    if let Some(rest) = raw.strip_prefix("https://img.gamemonetize.com/") {
        return format!("/prox/gm-icon/{rest}");
    }
    if let Some(rest) = raw.strip_prefix("https://lumassets.pages.dev/") {
        return format!("/prox/luma/{rest}");
    }
    if raw.to_ascii_lowercase().starts_with("https://") {
        return raw.to_string();
    }
    String::new()
}

/// Where a catalog entry's game is opened from, or "" when it can't be.
fn game_url(raw: &str) -> String {
    if raw.starts_with("/games/") || raw.starts_with("/prox/") {
        return raw.to_string();
    }
    if raw.starts_with('/') {
        return format!("/prox/calculated2{raw}");
    }
    let Ok(url) = url::Url::parse(raw) else {
        return String::new();
    };
    if url.scheme() != "https" {
        return String::new();
    }
    let rest = |url: &url::Url| {
        let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
        let hash = url.fragment().map(|f| format!("#{f}")).unwrap_or_default();
        format!("{}{query}{hash}", url.path())
    };
    match url.host_str().unwrap_or("") {
        "lumassets.pages.dev" => format!("/prox/luma{}", rest(&url)),
        "calculated2.github.io" => format!("/prox/calculated2{}", rest(&url)),
        "html5.gamemonetize.co" | "html5.gamemonetize.com" => {
            format!("/prox/gamemonetize{}", rest(&url))
        }
        _ => url.to_string(),
    }
}

fn category_for(title: &str, description: &str, url: &str) -> &'static str {
    static RULES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            (r"fnaf|backrooms|silent hill|dreader|horror", "Horror"),
            (r"multiplayer|1v1|basket bros|shell shock|smash karts|tetr\.io", "Multiplayer"),
            (r"race|racing|moto|drift|traffic|car|highway|swerve", "Racing"),
            (r"basket|football|soccer|golf|bowl|skate|punch|pool party", "Sports"),
            (r"/retro/|pokemon|mario|sonic|zelda|metroid|kirby|donkey kong|final fantasy|star fox|mega ?man|tetris", "Retro"),
            (r"puzzle|riddle|wordle|2048|bloxorz|alchemy|sort|quiz|calculator|logic", "Puzzle"),
            (r"shooter|doom|quake|gun|combat|commando|battle|fight|hobo|shark|tank|action", "Action"),
            (r"papa|idle|learn to fly|duck life|buddy|sandbox|tycoon", "Casual"),
        ]
        .into_iter()
        .filter_map(|(pattern, name)| regex::Regex::new(pattern).ok().map(|re| (re, name)))
        .collect()
    });
    let value = format!("{title} {description} {url}").to_lowercase();
    rules
        .iter()
        .find(|(re, _)| re.is_match(&value))
        .map(|(_, name)| *name)
        .unwrap_or("Arcade")
}

impl Catalog {
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            popular: Vec::new(),
            alphabetical: Vec::new(),
            featured: Vec::new(),
        }
    }

    /// Builds the catalog from the parsed JSON (`{"links": [{"title", "games": [[title, image, url, description, flag]]}]}`).
    pub fn from_json(data: &Value) -> Self {
        let ranks: std::collections::HashMap<String, usize> = FEATURED_TITLES
            .iter()
            .enumerate()
            .map(|(i, t)| (key_of(t), i))
            .collect();
        let mut items: Vec<Item> = Vec::new();
        for section in data["links"].as_array().into_iter().flatten() {
            let section_title = section["title"].as_str().unwrap_or("");
            for entry in section["games"].as_array().into_iter().flatten() {
                let field = |i: usize| entry.get(i).and_then(|v| v.as_str()).unwrap_or("");
                let title = plain_text(field(0));
                let description = plain_text(field(3));
                let url = game_url(field(2));
                if title.is_empty() || url.is_empty() {
                    continue;
                }
                let open_source = title.to_lowercase() == "2048";
                let key = key_of(&title);
                items.push(Item {
                    id: items.len(),
                    category: category_for(&title, &description, &url),
                    image: image_url(field(1)),
                    url: if open_source {
                        "/game-portal/ui/open-source/2048/".to_string()
                    } else {
                        url
                    },
                    open_source,
                    external: field(4) == "IgnoreIframe",
                    search: format!("{title} {description} {section_title}").to_lowercase(),
                    rank: ranks.get(&key).copied().unwrap_or(usize::MAX),
                    key,
                    title,
                });
            }
        }
        // Entries without art borrow the art of another entry with the same name.
        let mut art: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for item in &items {
            if !item.image.is_empty() {
                art.insert(item.key.clone(), item.image.clone());
            }
        }
        for item in &mut items {
            if item.image.is_empty() {
                item.image = art.get(&item.key).cloned().unwrap_or_default();
            }
        }

        let local = |item: &Item| -> u8 {
            if item.url.starts_with("/games/") {
                0
            } else if item.external {
                2
            } else {
                1
            }
        };
        let mut popular: Vec<usize> = (0..items.len()).collect();
        popular.sort_by(|&a, &b| {
            let (a, b) = (&items[a], &items[b]);
            a.rank
                .cmp(&b.rank)
                .then_with(|| {
                    if a.key == b.key {
                        local(a).cmp(&local(b))
                    } else {
                        a.title.to_lowercase().cmp(&b.title.to_lowercase())
                    }
                })
                .then_with(|| a.title.cmp(&b.title))
        });
        let mut alphabetical: Vec<usize> = (0..items.len()).collect();
        alphabetical.sort_by(|&a, &b| {
            let (a, b) = (&items[a], &items[b]);
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.title.cmp(&b.title))
        });
        let mut seen = std::collections::HashSet::new();
        let featured: Vec<usize> = popular
            .iter()
            .copied()
            .filter(|&i| items[i].rank != usize::MAX && seen.insert(items[i].key.clone()))
            .take(12)
            .collect();
        Self {
            items,
            popular,
            alphabetical,
            featured,
        }
    }

    fn load(base_dir: &std::path::Path) -> Self {
        let path = base_dir.join("data/portal_catalog.json");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .map(|data| Self::from_json(&data))
            .unwrap_or_else(Self::empty)
    }

    pub fn summary(&self) -> Value {
        let categories: Vec<Value> = CATEGORIES
            .iter()
            .filter_map(|name| {
                let count = self.items.iter().filter(|i| i.category == *name).count();
                (count > 0).then(|| json!({ "name": name, "count": count }))
            })
            .collect();
        json!({
            "total": self.items.len(),
            "categories": categories,
            "featured": self.featured.iter().map(|&i| self.items[i].json()).collect::<Vec<_>>(),
        })
    }

    /// One page of the list, filtered by category and search text.
    pub fn list(
        &self,
        category: &str,
        query: &str,
        alphabetical: bool,
        offset: usize,
        limit: usize,
    ) -> Value {
        let query = query.trim().to_lowercase();
        let title_query = key_of(&query);
        let order = if alphabetical {
            &self.alphabetical
        } else {
            &self.popular
        };
        let mut total = 0usize;
        let mut page: Vec<Value> = Vec::new();
        for &index in order {
            let item = &self.items[index];
            if category != "All" && item.category != category {
                continue;
            }
            let title_match = !title_query.is_empty() && item.key.contains(&title_query);
            if !query.is_empty() && !item.search.contains(&query) && !title_match {
                continue;
            }
            if total >= offset && page.len() < limit {
                page.push(item.json());
            }
            total += 1;
        }
        json!({ "total": total, "items": page })
    }

    pub fn items_by_id(&self, ids: &[usize]) -> Vec<Value> {
        ids.iter()
            .filter_map(|&id| self.items.get(id))
            .map(Item::json)
            .collect()
    }

    pub fn random(&self, pick: usize) -> Option<Value> {
        (!self.items.is_empty()).then(|| self.items[pick % self.items.len()].json())
    }
}

/// Where game `id` lives on its proxy, or `None` for an unknown id or a game
/// that is not behind one of the game proxies.
pub fn game_dir(state: &AppState, id: usize) -> Option<GameDir> {
    split_game_url(&catalog(state).items.get(id)?.url)
}

/// The real address (`/prox/gamemonetize/...`) behind a `/prox/g/<id>/...` page
/// address, for play statistics.
pub fn real_game_url(state: &AppState, page: &str) -> Option<String> {
    let id: usize = page
        .strip_prefix("/prox/g/")?
        .split('/')
        .next()?
        .parse()
        .ok()?;
    Some(catalog(state).items.get(id)?.url.clone())
}

/// Maps `/prox/i/<id>` to the real address of that item's art, so the request
/// can be served as if it had asked for that address. `None` for any other path
/// or an id with no local art.
pub fn image_uri(state: &AppState, uri: &axum::http::Uri) -> Option<axum::http::Uri> {
    let id: usize = uri
        .path()
        .strip_prefix("/prox/i/")?
        .trim_end_matches('/')
        .parse()
        .ok()?;
    let item = catalog(state).items.get(id)?;
    if !item.image.starts_with('/') {
        return None;
    }
    item.image.parse().ok()
}

fn catalog(state: &AppState) -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| Catalog::load(&state.cfg.base_dir))
}

fn param(search: &str, name: &str) -> String {
    url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn respond(value: Value) -> Response {
    let mut response = crate::errors::json_resp(200, value);
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, max-age=300"),
    );
    response
}

/// Handles `/api/portal/*`; `None` for any other path.
pub fn handle(state: &AppState, method: &Method, path: &str, search: &str) -> Option<Response> {
    let name = path.strip_prefix("/api/portal/")?;
    if *method != Method::GET {
        return Some(crate::errors::err_resp(
            405,
            Some("method not allowed"),
            None,
        ));
    }
    let catalog = catalog(state);
    match name {
        "summary" => Some(respond(catalog.summary())),
        "list" => {
            let category = param(search, "cat");
            let category = if CATEGORIES.contains(&category.as_str()) {
                category
            } else {
                "All".to_string()
            };
            let query: String = param(search, "q").chars().take(MAX_QUERY_CHARS).collect();
            let offset = param(search, "offset")
                .parse::<usize>()
                .unwrap_or(0)
                .min(1_000_000);
            let limit = param(search, "limit")
                .parse::<usize>()
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, MAX_LIMIT);
            let alphabetical = param(search, "sort") == "alphabetical";
            Some(respond(catalog.list(
                &category,
                &query,
                alphabetical,
                offset,
                limit,
            )))
        }
        "items" => {
            let ids: Vec<usize> = param(search, "ids")
                .split(',')
                .filter_map(|id| id.trim().parse().ok())
                .take(MAX_IDS)
                .collect();
            Some(respond(json!({ "items": catalog.items_by_id(&ids) })))
        }
        "random" => {
            let pick = (mitch_lib::school::now_millis() as usize)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(std::process::id() as usize);
            let mut response = respond(json!({ "item": catalog.random(pick) }));
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            Some(response)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Catalog {
        Catalog::from_json(&json!({ "links": [
            { "title": "#", "games": [
                ["Run 3", "https://img.gamemonetize.com/aaa/512x384.jpg", "https://html5.gamemonetize.co/aaa/", "", ""],
                ["Tom &amp; Jerry <b>Race</b>", "", "https://lumassets.pages.dev/tj/", "A racing game", ""],
                ["No url", "", "", "", ""],
                ["2048", "", "https://lumassets.pages.dev/2048/", "", ""],
                ["Zombie Shooter", "", "http://insecure.example/z", "", ""],
                ["Zombie Shooter", "", "https://html5.gamemonetize.co/zs/?x=1", "", "IgnoreIframe"],
                ["Run 3", "", "/games/run3/", "", ""],
            ]},
            { "title": "B", "games": [
                ["Barrel Blast", "", "https://example.org/blast", "", "IgnoreIframe"],
            ]},
        ]}))
    }

    #[test]
    fn urls_titles_and_ids_follow_the_old_in_page_rules() {
        let c = sample();
        let titles: Vec<&str> = c.items.iter().map(|i| i.title.as_str()).collect();
        // Invalid entries (no url, http) are skipped and ids are consecutive.
        assert_eq!(
            titles,
            [
                "Run 3",
                "Tom & Jerry Race",
                "2048",
                "Zombie Shooter",
                "Run 3",
                "Barrel Blast"
            ]
        );
        assert_eq!(
            c.items.iter().map(|i| i.id).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert_eq!(c.items[0].url, "/prox/gamemonetize/aaa/");
        assert_eq!(c.items[0].image, "/prox/gm-icon/aaa/512x384.jpg");
        assert_eq!(c.items[1].url, "/prox/luma/tj/");
        assert_eq!(c.items[2].url, "/game-portal/ui/open-source/2048/");
        assert!(c.items[2].open_source);
        assert_eq!(c.items[3].url, "/prox/gamemonetize/zs/?x=1");
        assert!(c.items[3].external);
        assert_eq!(c.items[5].url, "https://example.org/blast");
        assert_eq!(c.items[1].category, "Racing");
        assert_eq!(c.items[3].category, "Action");
        assert_eq!(c.items[0].category, "Arcade");
    }

    #[test]
    fn local_art_is_addressed_by_id_so_no_name_is_in_the_image_address() {
        let c = sample();
        // Proxied art becomes /prox/i/<id>; the real address stays server-side.
        assert_eq!(c.items[0].json()["image"], "/prox/i/0");
        assert_eq!(c.items[0].image, "/prox/gm-icon/aaa/512x384.jpg");
        // Art that borrows another entry's image gets an id address too.
        assert_eq!(c.items[4].json()["image"], "/prox/i/4");
        // An entry with no art and art on another host are unchanged.
        let other = Catalog::from_json(&json!({ "links": [{ "title": "#", "games": [
            ["Elsewhere", "https://cdn.example/x.png", "https://example.org/e", "", ""],
            ["Bare", "", "https://example.org/b", "", ""],
        ]}]}));
        assert_eq!(other.items[0].json()["image"], "https://cdn.example/x.png");
        assert_eq!(other.items[1].json()["image"], "");
    }

    #[test]
    fn proxied_games_are_addressed_by_id_and_other_games_are_left_alone() {
        let c = sample();
        // Luma and gamemonetize games: /prox/g/<id>/<entry file>.
        assert_eq!(c.items[0].json()["url"], "/prox/g/0/");
        assert_eq!(c.items[1].json()["url"], "/prox/g/1/");
        // A query on the entry address is kept.
        assert_eq!(c.items[3].json()["url"], "/prox/g/3/?x=1");
        // Open-source, local and external games keep their address.
        assert_eq!(
            c.items[2].json()["url"],
            "/game-portal/ui/open-source/2048/"
        );
        assert_eq!(c.items[4].json()["url"], "/games/run3/");
        assert_eq!(c.items[5].json()["url"], "https://example.org/blast");

        let g = split_game_url("/prox/luma/snowrider3d/index.html").unwrap();
        assert_eq!(
            (g.prefix, g.dir.as_str(), g.file.as_str()),
            ("/prox/luma/", "snowrider3d/", "index.html")
        );
        let g = split_game_url("/prox/gamemonetize/abc123/").unwrap();
        assert_eq!((g.dir.as_str(), g.file.as_str()), ("abc123/", ""));
        assert!(split_game_url("/games/run3/").is_none());
        assert!(split_game_url("https://example.org/x").is_none());
    }

    #[test]
    fn featured_and_popular_order_use_the_curated_list() {
        let c = sample();
        // "Run 3" is rank 0 and "2048" is in the list; both are featured, once each.
        let featured: Vec<&str> = c
            .featured
            .iter()
            .map(|&i| c.items[i].title.as_str())
            .collect();
        assert_eq!(featured, ["Run 3", "2048"]);
        // A local copy of a duplicated title sorts ahead of the proxied one.
        assert_eq!(c.items[c.popular[0]].url, "/games/run3/");
    }

    #[test]
    fn list_filters_pages_and_searches() {
        let c = sample();
        let all = c.list("All", "", false, 0, 60);
        assert_eq!(all["total"], 6);
        let page = c.list("All", "", false, 2, 2);
        assert_eq!(page["total"], 6);
        assert_eq!(page["items"].as_array().unwrap().len(), 2);
        assert_eq!(c.list("Racing", "", false, 0, 60)["total"], 1);
        // Search matches the description, and a title ignoring spaces and
        // punctuation ("tom jerry" finds "Tom & Jerry").
        assert_eq!(c.list("All", "racing game", false, 0, 60)["total"], 1);
        assert_eq!(c.list("All", "tom jerry", false, 0, 60)["total"], 1);
        assert_eq!(c.list("All", "tomjerry", false, 0, 60)["total"], 1);
        assert_eq!(c.list("All", "barrel zzz", false, 0, 60)["total"], 0);
        let alpha = c.list("All", "", true, 0, 60);
        assert_eq!(alpha["items"][0]["title"], "2048");
    }

    #[test]
    fn items_by_id_keep_order_and_skip_unknown_ids() {
        let c = sample();
        let items = c.items_by_id(&[3, 99, 0]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], 3);
        assert_eq!(items[1]["id"], 0);
        assert!(Catalog::empty().random(5).is_none());
    }

    #[test]
    fn the_real_catalog_loads_and_ids_match_the_old_page() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let c = Catalog::load(&base);
        if c.items.is_empty() {
            return; // data file not present in this checkout
        }
        assert!(c.items.len() > 20_000);
        assert!(c.featured.len() >= 10);
        assert_eq!(c.items[c.featured[0]].title, "Run 3");
    }
}
