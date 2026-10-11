use ts_typegen::Ts;

#[derive(serde::Serialize, Ts)]
pub struct PatientView {
    pub id: i64,
    pub name: String,
}

#[derive(serde::Serialize, Ts)]
pub struct CreatedResponse {
    pub id: i64,
}

#[derive(serde::Serialize, Ts)]
pub struct UpdatedResponse {
    pub id: i64,
}

/// Carries no `Ts` derive -- exercises the build's `.map_response("FileResponse", "Blob")`
/// mapping for an `IntoResponse` type outside this crate's generated TypeScript surface.
pub struct FileResponse {
    pub bytes: Vec<u8>,
}

impl axum::response::IntoResponse for FileResponse {
    fn into_response(self) -> axum::response::Response {
        self.bytes.into_response()
    }
}

pub struct ApiError;

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}
