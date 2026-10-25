use serde::Serialize;
#[derive(Serialize)]
pub struct BookView {
    pub id: u64,
    pub title: String,
    pub target_language: String,
}
