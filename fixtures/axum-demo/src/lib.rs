pub mod handlers;
pub mod response;

use axum::routing::{any, get, post};
use axum::Router;

/// The real entry point: no `#[ts_route]`, no foreign attribute, nothing
/// annotated anywhere in this crate. Every route below is discovered purely
/// by walking this builder chain and the sub-routers it nests and merges.
pub fn app() -> Router {
    Router::new()
        .merge(patient_routes())
        .merge(note_routes())
        .nest("/widgets", widget_routes())
        .nest("/orders", order_routes())
        .route("/health", any(handlers::admin::health_check))
}

fn patient_routes() -> Router {
    Router::new().route("/patients/{id}", get(handlers::patient::get_patient))
}

fn note_routes() -> Router {
    Router::new()
        .route("/notes/:id", get(handlers::notes::get_note))
        .route("/notes", post(handlers::notes::add_note))
}

fn widget_routes() -> Router {
    Router::new().route("/{a}/{b}", get(handlers::widgets::get_widget))
}

fn order_routes() -> Router {
    Router::new().route("/{order_id}/{line_id}", get(handlers::orders::get_order_line))
}
