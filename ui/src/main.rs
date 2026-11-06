//! The ethos interface. Native access goes through `bridge`; page content comes from the engine.

mod app;
mod bridge;
mod chrome;
mod controls;
mod icons;
mod page;
mod sidebar;
mod store;

fn main() {
    let root = leptos::prelude::document()
        .get_element_by_id("root")
        .and_then(|root| wasm_bindgen::JsCast::dyn_into(root).ok())
        .expect("index.html has a root element");
    leptos::mount::mount_to(root, app::App).forget();
}
