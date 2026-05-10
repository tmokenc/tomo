//! Tomo admin SPA — Yew client.
//!
//! Mounts on `#app-root` of `index.html`. All data comes from `/api/*` REST
//! endpoints served by the tomo-admin backend. The first request is always
//! `GET /api/me`; if that returns 401 we send the user to `/login`.

pub mod api;
pub mod components;
pub mod routes;
pub mod types;

use wasm_bindgen::prelude::*;
use yew::Renderer;

use crate::components::app::App;

#[wasm_bindgen(start)]
pub fn start() {
    wasm_logger::init(wasm_logger::Config::new(log::Level::Info));

    let root = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("app-root"))
        .expect("missing #app-root in index.html");

    Renderer::<App>::with_root(root).render();
}
