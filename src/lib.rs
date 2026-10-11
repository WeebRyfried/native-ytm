//! Native YTM: a lightweight YouTube Music client for Windows, in the shape
//! of Spotifast. egui draws the interface; the InnerTube API gives the
//! catalogue; a Rust audio engine streams, decodes and plays.

pub mod api;
pub mod app;
pub mod backend;
pub mod discord;
pub mod images;
pub mod lastfm;
pub mod model;
pub mod platform;
pub mod player;
pub mod queue;
pub mod settings;
pub mod stream;
pub mod theme;
pub mod ui;
