use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("collector: {0}")]
    Collector(String),

    #[error("database: {0}")]
    Database(#[from] sqlx::Error),

    #[error("journal: {0}")]
    Journal(String),

    #[error("web: {0}")]
    Web(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),

    #[error("bad request: {0}")]
    BadRequest(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match &self {
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}
