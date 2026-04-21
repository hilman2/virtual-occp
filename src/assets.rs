use axum::{
    body::Body,
    extract::Path,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

use crate::station::Handle;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

pub fn router() -> Router<Handle> {
    Router::new()
        .route("/", get(index))
        .route("/*file", get(static_file))
}

async fn index() -> impl IntoResponse {
    serve("index.html")
}

async fn static_file(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        return serve("index.html");
    }
    serve(path)
}

fn serve(path: &str) -> Response {
    // Fall back to index.html for unknown paths (SPA-like behavior).
    let (file, actual) = match Assets::get(path) {
        Some(f) => (f, path.to_string()),
        None => match Assets::get("index.html") {
            Some(f) => (f, "index.html".to_string()),
            None => {
                return (StatusCode::NOT_FOUND, "not found").into_response();
            }
        },
    };
    let mime = mime_guess::from_path(&actual).first_or_octet_stream();
    let body = Body::from(file.data.into_owned());
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(body)
        .unwrap()
}

// Kept around for future use.
#[allow(dead_code)]
async fn by_path(Path(p): Path<String>) -> impl IntoResponse {
    serve(&p)
}
