//! Icons embedded in the binary. GPUI draws an SVG as a mask tinted with the element's text
//! color, so one file serves every color. The icons come from Lucide (see `assets/icons/LICENSE`).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons![
    "archive",
    "arrow-up",
    "audio-lines",
    "ban",
    "banknote",
    "bell",
    "briefcase",
    "briefcase-medical",
    "bug",
    "chevron-down",
    "chevron-left",
    "chevron-right",
    "circle-help",
    "circle-x",
    "clock",
    "compass",
    "contact",
    "file-text",
    "gift",
    "globe",
    "heart-pulse",
    "house",
    "keypad",
    "languages",
    "link",
    "lock-keyhole",
    "mail",
    "message-circle",
    "message-circle-filled",
    "mic",
    "paintbrush",
    "phone",
    "phone-off",
    "plane",
    "plug",
    "plus",
    "search",
    "shield-check",
    "shield-keyhole",
    "shopping-bag",
    "sparkles",
    "ticket",
    "user-round",
    "users-round",
    "wallet",
    "x",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect())
    }
}
