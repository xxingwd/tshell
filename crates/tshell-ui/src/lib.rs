//! Shared native interface primitives and the application's locale catalog.
extern crate gpui_kit as gpui;
pub mod appearance;
pub mod controls;
pub mod i18n;
rust_i18n::i18n!("../../locales", fallback = "en");

#[macro_export]
macro_rules! t {
    ($key:expr, locale = $locale:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::translate(&($key), Some($locale), &[$((stringify!($name), ($value).to_string())),*])
    };
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::translate(&($key), None, &[$((stringify!($name), ($value).to_string())),*])
    };
}

pub mod editor;
