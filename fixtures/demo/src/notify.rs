use serde::Serialize;
use ts_typegen::Ts;

/// A support ticket. `id` exercises `Preset::Uuid`, wired in `build.rs`.
#[derive(Serialize, Ts)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: uuid::Uuid,
    pub subject: String,
    /// Raw file bytes. serde_json writes this as a JSON number array, so the
    /// default binding is `number[]`; `.bytes(Bytes::Str)` would switch it to
    /// `string` for a base64-style codec instead.
    pub attachment: Vec<u8>,
}

/// Adjacently tagged: the discriminant and payload sit in separate keys,
/// e.g. `{ type: "alert", data: { message, level } }`.
#[derive(Serialize, Ts)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Notification {
    Info(String),
    Alert { message: String, level: u8 },
}

/// Untagged: serde tries each variant's own shape in order with no
/// discriminant at all, so the binding is a plain union (`number | string`).
#[derive(Serialize, Ts)]
#[serde(untagged)]
pub enum Id {
    Numeric(u64),
    Text(String),
}
