use serde::Serialize;
use ts_typegen::Ts;

/// A user of the system.
#[derive(Serialize, Ts)]
#[serde(rename_all = "camelCase")]
pub struct User {
    /// Stable identifier.
    pub user_id: u64,
    pub name: String,
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(skip)]
    pub secret: String,
}

#[derive(Serialize, Ts)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Guest,
}

#[derive(Serialize, Ts)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Event {
    Click { x: i32, y: i32 },
    KeyPress { code: String },
}

#[derive(Serialize, Ts)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}
