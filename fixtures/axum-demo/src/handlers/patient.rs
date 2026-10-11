use axum::extract::Path;
use axum::Json;

use crate::response::PatientView;

/// Exercises the `{id}`-style capture syntax.
pub async fn get_patient(Path(id): Path<i64>) -> Json<PatientView> {
    Json(PatientView { id, name: String::new() })
}
