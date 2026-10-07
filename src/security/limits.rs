//! Batas ukuran body request. Router yang butuh batas lebih ketat memasang sendiri (mis. 64 KiB untuk task/project);
//! batas global ini menjadi pagar bagi route lain yang tidak memasangnya (bawaan axum adalah 2 MiB).

use axum::extract::DefaultBodyLimit;

pub const MAX_BODY_BYTES: usize = 256 * 1024;

pub fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_BODY_BYTES)
}
