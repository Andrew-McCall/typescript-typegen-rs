use ts_typegen::Ts;

#[derive(serde::Serialize, Ts)]
pub struct PatientView {
    pub id: i64,
    pub name: String,
}

#[derive(serde::Serialize, Ts)]
pub struct NoteView {
    pub id: i64,
    pub body: String,
}

#[derive(serde::Serialize, Ts)]
pub struct WidgetView {
    pub a: i64,
    pub b: i64,
}

#[derive(serde::Serialize, Ts)]
pub struct OrderLineView {
    pub order_id: i64,
    pub line_id: i64,
}

#[derive(serde::Serialize, Ts)]
pub struct StatusView {
    pub ok: bool,
}
