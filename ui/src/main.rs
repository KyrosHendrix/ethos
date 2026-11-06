//! The ethos interface. Native access goes through `bridge`; page content comes from the engine.

mod app;
mod bridge;
mod chrome;
mod controls;
mod icons;
mod page;
mod sidebar;
mod store;
mod tools;

fn main() {
    // Without this a panic reaches the console as a bare "unreachable".
    std::panic::set_hook(Box::new(|panic| {
        web_sys::console::error_1(&panic.to_string().into());
    }));
    let root = leptos::prelude::document()
        .get_element_by_id("root")
        .and_then(|root| wasm_bindgen::JsCast::dyn_into(root).ok())
        .expect("index.html has a root element");
    leptos::mount::mount_to(root, app::App).forget();
}
