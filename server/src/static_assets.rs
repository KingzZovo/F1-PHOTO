//! Embedded SPA static assets.
//!
//! At release time the Vue 3 build output (`web/dist`) is embedded into the
//! binary via `include_dir`. The fallback handler below serves embedded
//! files for any non-`/api`, non-`/healthz`, non-`/readyz` path. If the
//! path does not match an embedded file (e.g. `/projects/abc`), `index.html`
//! is returned so the SPA router can take over.
//!
//! When `web/dist/` is missing or empty during local development the embed
//! is empty and the fallback returns `404 Not Found` for non-API routes —
//! the API itself still works, and the dev server should be hit through
//! `vite dev`.

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use include_dir::{Dir, File, include_dir};

static WEB_DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../web/dist");

/// Attach an SPA fallback to `router` that serves embedded `web/dist` files.
///
/// The fallback only kicks in for unmatched routes — actual API routes
/// continue to win because they are matched first.
pub fn attach_spa_fallback(router: Router) -> Router {
    router.fallback(spa_fallback)
}

async fn spa_fallback(req: Request) -> Response {
    let path = req.uri().path();

    // Don't shadow API / health endpoints. If the API router didn't match, it
    // really is a 404 — clients shouldn't get HTML back for `/api/...`.
    if path.starts_with("/api/") || path == "/healthz" || path == "/readyz" {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    // Try the literal path (strip leading `/`).
    let trimmed = path.trim_start_matches('/');
    if !trimmed.is_empty() {
        if let Some(resp) = serve_embedded(trimmed) {
            return resp;
        }
    }

    // SPA fallback: serve index.html so the Vue router can render.
    if let Some(resp) = serve_embedded("index.html") {
        return resp;
    }

    // No embedded bundle at all (e.g. dev build without `web/dist`).
    (StatusCode::NOT_FOUND, "static bundle not embedded").into_response()
}

fn serve_embedded(rel_path: &str) -> Option<Response> {
    let file: &File = WEB_DIST.get_file(rel_path)?;
    let mime = mime_for(rel_path);
    let body = Body::from(file.contents());
    let mut resp = Response::new(body);
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    // Cache hashed asset chunks aggressively, but never cache index.html.
    let cache = if rel_path.ends_with("index.html") || rel_path.is_empty() {
        "no-cache"
    } else if rel_path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Some(resp)
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Best-effort summary used during boot logging.
pub fn embed_summary() -> EmbedSummary {
    let mut count = 0usize;
    let mut bytes = 0u64;
    let has_index = WEB_DIST.get_file("index.html").is_some();
    walk(&WEB_DIST, &mut count, &mut bytes);
    EmbedSummary {
        file_count: count,
        total_bytes: bytes,
        has_index,
    }
}

fn walk(dir: &Dir<'_>, count: &mut usize, bytes: &mut u64) {
    for f in dir.files() {
        *count += 1;
        *bytes += f.contents().len() as u64;
    }
    for sub in dir.dirs() {
        walk(sub, count, bytes);
    }
}

pub struct EmbedSummary {
    pub file_count: usize,
    pub total_bytes: u64,
    pub has_index: bool,
}
