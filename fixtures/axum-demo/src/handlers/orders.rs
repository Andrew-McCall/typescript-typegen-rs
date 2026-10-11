use axum::extract::Path;
use axum::Json;
use ts_typegen::Ts;

use crate::response::OrderLineView;

/// A struct `Path` extractor. `#[derive(Ts)]` is needed here even though
/// this struct is never a response body -- route discovery resolves a
/// struct-shaped `Path<T>`'s field names by looking `T` up in the same
/// registry a response type comes from, so it has to be in that registry
/// too, or discovery can't zip its fields against the template's captures.
#[derive(serde::Deserialize, Ts)]
pub struct OrderLineParams {
    pub order_id: i64,
    pub line_id: i64,
}

/// Exercises a struct `Path` extractor.
pub async fn get_order_line(Path(params): Path<OrderLineParams>) -> Json<OrderLineView> {
    Json(OrderLineView { order_id: params.order_id, line_id: params.line_id })
}
