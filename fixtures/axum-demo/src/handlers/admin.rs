use axum::Json;

use crate::response::StatusView;

/// Exercises an `any` route.
pub async fn health_check() -> Json<StatusView> {
    Json(StatusView { ok: true })
}
