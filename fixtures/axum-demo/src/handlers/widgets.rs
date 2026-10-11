use axum::extract::Path;
use axum::Json;

use crate::response::WidgetView;

/// Exercises a tuple `Path` extractor.
pub async fn get_widget(Path((a, b)): Path<(i64, i64)>) -> Json<WidgetView> {
    Json(WidgetView { a, b })
}
