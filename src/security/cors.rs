//! Validasi origin CORS dari konfigurasi. Hanya origin eksplisit `http(s)://host[:port]`; wildcard, path, query,
//! dan spasi ditolak sehingga konfigurasi tidak bisa tanpa sengaja membuka API ke semua situs.

use axum::http::HeaderValue;

pub const MAX_ORIGINS: usize = 8;

pub fn parse_origin(origin: &str) -> Result<HeaderValue, &'static str> {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .ok_or("origin harus diawali http:// atau https://")?;
    if rest.is_empty() || rest.contains('*') {
        return Err("origin tidak boleh kosong atau memakai wildcard");
    }
    if rest.contains(['/', '?', '#', '@'])
        || rest.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(
            "origin hanya boleh berupa skema://host[:port] tanpa path, query, atau kredensial",
        );
    }
    HeaderValue::from_str(origin).map_err(|_| "origin bukan nilai header yang valid")
}

pub fn parse_origins(origins: &[String]) -> Result<Vec<HeaderValue>, &'static str> {
    if origins.is_empty() || origins.len() > MAX_ORIGINS {
        return Err("jumlah origin CORS harus 1..=8");
    }
    origins.iter().map(|origin| parse_origin(origin)).collect()
}
