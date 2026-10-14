//! Native YTM: a lightweight YouTube Music client for Windows, in the shape
//! of Spotifast. egui draws the interface; the InnerTube API gives the
//! catalogue; a Rust audio engine streams, decodes and plays.

pub mod api;
pub mod app;
pub mod backend;
pub mod cache;
pub mod discord;
pub mod images;
pub mod lastfm;
pub mod local;
pub mod model;
pub mod net;
pub mod platform;
pub mod player;
pub mod queue;
pub mod readings;
pub mod settings;
pub mod stream;
pub mod theme;
pub mod ui;
pub mod update;
