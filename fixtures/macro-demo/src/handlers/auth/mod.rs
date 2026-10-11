use axum::extract::Json;
use macro_demo_macros::api;

use crate::response::UpdatedResponse;

#[api(Route = "/auth/refresh", Method = "POST")]
pub async fn refresh() -> Json<UpdatedResponse> {
    Json(UpdatedResponse { id: 1 })
}
