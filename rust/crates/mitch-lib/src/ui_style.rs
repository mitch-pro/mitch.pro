//! Each account's site style: mitch.pro (the default) or Tylerium.
//!
//! The choice lives in `ui_style_choices`, one row per normalized email. No
//! row means the account has never chosen: pages render in mitch.pro and the
//! server asks once with the chooser (`webserver/ui-style.js`). The server
//! decides per page whether the choice applies at all (public pages and
//! rjuhsd.school always use mitch.pro); see `mitch-server/src/ui_style.rs` and
//! `docs/ui-styles.md`.

use crate::data::DataStore;
use rusqlite::params;

/// A site style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiStyle {
    /// The original mitch.pro look. Default for everyone who hasn't chosen.
    #[default]
    Mitch,
    /// Tylerium's charcoal/blue look with the cloud background.
    Tylerium,
}

impl UiStyle {
    /// The stored and API value.
    pub fn as_str(self) -> &'static str {
        match self {
            UiStyle::Mitch => "mitch",
            UiStyle::Tylerium => "tylerium",
        }
    }

    /// Parses an API value; anything else is rejected.
    pub fn parse(value: &str) -> Option<UiStyle> {
        match value {
            "mitch" => Some(UiStyle::Mitch),
            "tylerium" => Some(UiStyle::Tylerium),
            _ => None,
        }
    }
}

/// The account's saved style, or `None` if it has never chosen.
pub fn get(store: &DataStore, email: &str) -> Option<UiStyle> {
    store
        .conn()
        .query_row(
            "SELECT style FROM ui_style_choices WHERE email = ?1",
            [email],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| UiStyle::parse(&value))
}

/// Saves (or replaces) the account's style. Returns false if the write failed.
pub fn set(store: &DataStore, email: &str, style: UiStyle) -> bool {
    store
        .conn()
        .execute(
            "INSERT INTO ui_style_choices (email, style, chosen_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(email) DO UPDATE SET style = excluded.style, chosen_at = excluded.chosen_at",
            params![email, style.as_str(), crate::school::now_millis()],
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-ui-style-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn unset_until_chosen_then_replaceable() {
        let s = store();
        let me = "test.student1@student.rjuhsd.us";
        assert_eq!(get(&s, me), None);
        assert!(set(&s, me, UiStyle::Tylerium));
        assert_eq!(get(&s, me), Some(UiStyle::Tylerium));
        assert!(set(&s, me, UiStyle::Mitch));
        assert_eq!(get(&s, me), Some(UiStyle::Mitch));
        assert_eq!(get(&s, "someone.else@x.test"), None);
    }

    #[test]
    fn only_known_values_parse() {
        assert_eq!(UiStyle::parse("mitch"), Some(UiStyle::Mitch));
        assert_eq!(UiStyle::parse("tylerium"), Some(UiStyle::Tylerium));
        assert_eq!(UiStyle::parse("Tylerium"), None);
        assert_eq!(UiStyle::parse(""), None);
        assert_eq!(UiStyle::default(), UiStyle::Mitch);
    }
}
