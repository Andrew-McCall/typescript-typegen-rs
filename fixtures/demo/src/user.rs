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

/// Serialises as a bare number, not an object.
#[derive(Serialize, Ts)]
#[serde(transparent)]
pub struct Meters(pub u32);

/// Immutable payload: every field is `readonly` in TypeScript.
#[derive(Serialize, Ts)]
#[ts(readonly)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub actor_id: u64,
    /// Spliced inline rather than imported.
    #[ts(inline)]
    pub window: Span,
    pub distance: Meters,
}

#[derive(Serialize, Ts)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Per-variant rename_all, and a deserialize-only rename that must be ignored.
#[derive(Serialize, Ts)]
#[serde(tag = "op")]
pub enum Command {
    #[serde(rename_all = "camelCase")]
    Resize { new_width: u32 },
    #[serde(rename(serialize = "quit", deserialize = "exit"))]
    Stop,
}
