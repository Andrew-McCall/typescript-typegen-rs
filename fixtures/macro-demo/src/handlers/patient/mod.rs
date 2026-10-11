use axum::extract::{Json, Path};
use macro_demo_macros::api;

use crate::response::{CreatedResponse, PatientView};

#[api(Route = "/patient/{id}", Method = "GET")]
pub async fn get_patient(Path(id): Path<i64>) -> Json<PatientView> {
    Json(PatientView { id, name: String::new() })
}

#[api(Route = "/patient", Method = "POST")]
pub async fn create_patient() -> Json<CreatedResponse> {
    Json(CreatedResponse { id: 1 })
}
