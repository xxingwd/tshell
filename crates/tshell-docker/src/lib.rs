extern crate gpui_kit as gpui;
pub use tshell_ui::{i18n, t};

pub mod connection;
pub use connection::{Endpoint, Target};

mod logs;
pub mod terminal;

mod panel;
mod view;
pub use panel::{DockerView, Event};
