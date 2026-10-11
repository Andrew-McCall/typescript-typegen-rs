use axum::extract::{Json, Path, Query};
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

/// `#[derive(Ts)]` like any other query type needs to be -- query extractors
/// have no `request`-style override, so this exercises the generated client's
/// query argument on its own: a concrete type with no index signature, cast
/// onto `RouteRequest.query: Record<string, unknown>`.
#[derive(serde::Deserialize, ts_typegen::Ts)]
pub struct SearchQuery {
    pub name: String,
}

#[api(Route = "/patient/search", Method = "GET", Response = "Vec<PatientView>")]
pub async fn search_patients(Query(q): Query<SearchQuery>) -> Json<Vec<PatientView>> {
    let _ = q;
    Json(Vec::new())
}

/// Not `#[derive(Ts)]` either -- stands in for a validation-friendly wire form
/// a macro generates at expansion time, invisible to the source scan. The
/// `Request = "PatientView"` override below must be what the client actually
/// uses, not this literal extractor type.
#[derive(serde::Deserialize)]
pub struct UpdatePatientWire {
    pub name: Option<String>,
}

#[api(Route = "/patient/{id}", Method = "PUT", Request = "PatientView")]
pub async fn update_patient(Path(id): Path<i64>, Json(body): Json<UpdatePatientWire>) -> Json<PatientView> {
    let _ = body;
    Json(PatientView { id, name: String::new() })
}
