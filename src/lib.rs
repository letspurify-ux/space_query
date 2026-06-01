#![allow(
    clippy::arc_with_non_send_sync,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod app;
pub mod app_icon;
pub mod db;
pub(crate) mod sql_delimiter;
pub mod sql_format;
pub mod sql_parser_engine;
pub mod sql_text;
pub mod ui;
pub mod utils;
pub mod version;
