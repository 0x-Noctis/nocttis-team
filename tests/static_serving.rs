// M5-007: WebApp statis disajikan Axum: SPA fallback, 404 JSON untuk /api, header keamanan, tanpa traversal/listing.
use std::{fs, path::PathBuf};

use ai_team::api;
use axum::{Router, routing::get};
use reqwest::{Client, StatusCode, redirect::Policy};
use tokio::net::TcpListener;
use uuid::Uuid;

struct Site {
    root: PathBuf,
    base: String,
    client: Client,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn site() -> Site {
    let root = std::env::temp_dir().join(format!("noctis-web-{}", Uuid::new_v4()));
    fs::create_dir_all(root.join("_app/immutable")).unwrap();
    fs::write(
        root.join("index.html"),
        "<!doctype html><title>Noctis</title><div id=app></div>",
    )
    .unwrap();
    fs::write(root.join("_app/immutable/app.js"), "console.log('app')").unwrap();
    fs::write(
        root.join("favicon.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    // Berkas di luar web_root yang tidak boleh bocor lewat traversal.
    fs::write(
        root.parent().unwrap().join(format!(
            "{}-secret.txt",
            root.file_name().unwrap().to_string_lossy()
        )),
        "TOP-SECRET",
    )
    .unwrap();
    api::web::validate_root(&root).unwrap();
    let router = Router::new()
        .route("/api/v1/ping", get(|| async { "pong" }))
        .fallback(api::error::not_found);
    let app = api::web::attach(router, &root).layer(axum::middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Site {
        root,
        base,
        client: Client::builder().redirect(Policy::none()).build().unwrap(),
    }
}

#[tokio::test]
async fn serves_files_and_falls_back_to_index_for_client_side_routes() {
    let site = site().await;
    let get = |path: &str| site.client.get(format!("{}{path}", site.base)).send();

    let root = get("/").await.unwrap();
    assert_eq!(root.status(), StatusCode::OK);
    assert!(root.text().await.unwrap().contains("<title>Noctis</title>"));

    let asset = get("/_app/immutable/app.js").await.unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    assert!(
        asset.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    assert_eq!(asset.text().await.unwrap(), "console.log('app')");

    // Rute SPA (dimuat ulang langsung): index.html dengan status 200, bukan 404.
    for route in [
        "/operations",
        "/runs/0b0d2d6e-aaaa-4bbb-8ccc-123456789abc",
        "/projects/x/y",
    ] {
        let response = get(route).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("<title>Noctis</title>"),
            "{route}"
        );
    }
}

#[tokio::test]
async fn api_paths_never_fall_back_to_html() {
    let site = site().await;
    let known = site
        .client
        .get(format!("{}/api/v1/ping", site.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        (known.status(), known.text().await.unwrap().as_str()),
        (StatusCode::OK, "pong")
    );

    for path in ["/api/v1/nope", "/api/", "/api/v1/ping/extra"] {
        let response = site
            .client
            .get(format!("{}{path}", site.base))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let body: serde_json::Value = response.json().await.expect("harus JSON, bukan HTML");
        assert_eq!(body["error"]["code"], "NOT_FOUND", "{path}");
    }
}

#[tokio::test]
async fn security_headers_are_set_and_traversal_is_not_served() {
    let site = site().await;
    let response = site
        .client
        .get(format!("{}/", site.base))
        .send()
        .await
        .unwrap();
    let headers = response.headers();
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let csp = headers["content-security-policy"].to_str().unwrap();
    for directive in [
        "default-src 'self'",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "connect-src 'self'",
    ] {
        assert!(csp.contains(directive), "{directive}");
    }
    // Header juga ada pada respons API dan 404.
    let missing = site
        .client
        .get(format!("{}/api/v1/nope", site.base))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.headers()["x-content-type-options"], "nosniff");

    // Traversal (mentah dan ter-encode) tidak boleh membaca berkas di luar root.
    let secret = format!(
        "{}-secret.txt",
        site.root.file_name().unwrap().to_string_lossy()
    );
    for path in [
        format!("/../{secret}"),
        format!("/%2e%2e/{secret}"),
        format!("/_app/../../{secret}"),
        format!("/..%2f{secret}"),
    ] {
        let response = site
            .client
            .get(format!("{}{path}", site.base))
            .send()
            .await
            .unwrap();
        let body = response.text().await.unwrap();
        assert!(
            !body.contains("TOP-SECRET"),
            "{path} membocorkan berkas di luar root"
        );
    }
    // Direktori tidak di-list.
    let listing = site
        .client
        .get(format!("{}/_app/immutable/", site.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !listing.contains("app.js"),
        "listing direktori tidak boleh ada"
    );
    let _ = fs::remove_file(site.root.parent().unwrap().join(secret));
}

#[test]
fn web_root_must_be_a_directory_with_index_html() {
    let dir = std::env::temp_dir().join(format!("noctis-web-bad-{}", Uuid::new_v4()));
    assert!(api::web::validate_root(&dir).is_err(), "tidak ada");
    fs::create_dir_all(&dir).unwrap();
    assert!(
        api::web::validate_root(&dir)
            .unwrap_err()
            .contains("index.html")
    );
    fs::write(dir.join("index.html"), "x").unwrap();
    assert!(api::web::validate_root(&dir).is_ok());
    let _ = fs::remove_dir_all(&dir);
}
