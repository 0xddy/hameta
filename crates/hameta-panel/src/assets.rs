use axum::Json;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use serde_json::json;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
pub struct WebAssets;

pub async fn serve_spa(uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    if raw_path == "api" || raw_path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": "NOT_FOUND",
                "message": "接口不存在"
            })),
        )
            .into_response();
    }

    if !raw_path.is_empty()
        && raw_path != ".gitkeep"
        && let Some(response) = asset_response(raw_path)
    {
        return response;
    }

    if let Some(response) = asset_response("index.html") {
        return response;
    }

    (
        StatusCode::SERVICE_UNAVAILABLE,
        "前端静态资源尚未构建，请先执行 `pnpm --dir web build`",
    )
        .into_response()
}

fn asset_response(path: &str) -> Option<Response> {
    let file = WebAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = (StatusCode::OK, file.data).into_response();
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    Some(response)
}
