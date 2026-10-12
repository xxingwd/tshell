extern crate gpui_kit as gpui;
pub use tshell_ui::{i18n, t};

pub mod connection;
pub use connection::{Endpoint, Target};

mod logs;
mod navigation;
pub mod terminal;

mod actions;
mod browser;
mod cli;
mod compose;
mod daemon;
mod documents;
mod images;
mod panel;
mod resource_view;
mod resources;
mod view;
pub use panel::{DockerView, Event};
pub use resources::ResourcePage;
