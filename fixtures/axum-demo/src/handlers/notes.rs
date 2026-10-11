use axum::extract::Path;
use axum::Json;
use ts_typegen::Ts;

use crate::response::NoteView;

#[derive(serde::Deserialize, Ts)]
pub struct NewNote {
    pub body: String,
}

/// Exercises the `:id`-style capture syntax.
pub async fn get_note(Path(id): Path<i64>) -> Json<NoteView> {
    Json(NoteView { id, body: String::new() })
}

/// Exercises an optional request body.
pub async fn add_note(body: Option<Json<NewNote>>) -> Json<NoteView> {
    let body = body.map(|Json(n)| n.body).unwrap_or_default();
    Json(NoteView { id: 0, body })
}
