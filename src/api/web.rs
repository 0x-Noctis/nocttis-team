//! Penyajian WebApp statis dari Axum (M5-007): satu proses dan satu origin untuk UI dan API.
//!
//! - File di bawah `web_root` dilayani apa adanya (`ServeDir`: tanpa listing direktori, path `..` ditolak).
//! - Path lain yang bukan `/api/...` jatuh ke `index.html` (SPA, status 200) supaya muat-ulang di `/operations` dst. berfungsi.
//! - `/api/...` yang tidak dikenal tetap 404 berbentuk envelope JSON, bukan HTML.
//! - Header keamanan dipasang pada semua respons.
//!
//! CSP memakai `'unsafe-inline'` untuk script dan style karena SvelteKit (adapter-static) menyisipkan skrip bootstrap inline;
//! penggantinya adalah `kit.csp` mode hash/nonce. Batas ini dicatat di `docs/deployment.md`.

use std::path::Path;

use axum::{
    Router,
    http::{HeaderName, HeaderValue},
    routing::any,
};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// Memeriksa `web_root` saat start: harus direktori dengan `index.html`. Pesan error menyebut penyebabnya tanpa path penuh.
pub fn validate_root(root: &Path) -> Result<(), &'static str> {
    if !root.is_dir() {
        return Err("server.web_root bukan direktori");
    }
    if !root.join("index.html").is_file() {
        return Err("server.web_root tidak berisi index.html (jalankan `npm run build` di web/)");
    }
    Ok(())
}

/// Tambahkan penyajian statis ke router API yang sudah lengkap (panggil SEBELUM layer lain agar header ikut terpasang).
pub fn attach(router: Router, root: &Path) -> Router {
    let files = ServeDir::new(root).fallback(ServeFile::new(root.join("index.html")));
    let header = |name: &'static str, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        )
    };
    router
        // Route spesifik menang atas wildcard ini; yang tersisa adalah /api/... yang memang tidak ada.
        .route("/api", any(super::error::not_found))
        .route("/api/", any(super::error::not_found))
        .route("/api/{*rest}", any(super::error::not_found))
        .fallback_service(files)
        .layer(header("content-security-policy", CSP))
        .layer(header("x-content-type-options", "nosniff"))
        .layer(header("x-frame-options", "DENY"))
        .layer(header("referrer-policy", "no-referrer"))
        .layer(header(
            "permissions-policy",
            "geolocation=(), microphone=(), camera=()",
        ))
}
