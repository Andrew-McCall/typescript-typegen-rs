use axum::extract::Path;
use macro_demo_macros::api;

use crate::response::{ApiError, FileResponse};

#[api(Route = "/files/{id}", Method = "GET")]
pub async fn download_file(Path(id): Path<i64>) -> Result<FileResponse, ApiError> {
    let _ = id;
    Ok(FileResponse { bytes: Vec::new() })
}

#[api(Route = "/export", Method = "GET", Response = "Vec<PatientView>")]
pub async fn export_patients() -> Result<impl axum::response::IntoResponse, ApiError> {
    Ok(axum::http::StatusCode::OK)
}
