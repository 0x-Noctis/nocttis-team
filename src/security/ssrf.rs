//! Perlindungan SSRF untuk tujuan provider model.
//!
//! Provider didaftarkan lewat API tanpa autentikasi (MVP lokal), jadi `base_url`-nya tidak boleh dipercaya: tanpa
//! pemeriksaan, siapa pun yang menjangkau API dapat mengarahkan server ke layanan internal (metadata cloud, jaringan privat,
//! loopback). Pemeriksaan dijalankan pada SETIAP panggilan ke provider (Lead, worker, reviewer, probe), bukan hanya saat
//! probe, dan DNS di-resolve ulang tiap kali sehingga perubahan DNS setelah pendaftaran tetap tertangkap.
use std::{env, net::IpAddr};

/// Nama environment variable berisi daftar host persis (dipisah koma) yang boleh dipakai walau alamatnya privat/loopback,
/// untuk provider internal yang tepercaya (mis. router lokal).
pub const ALLOWLIST_ENV: &str = "NOCTIS_PROVIDER_HOST_ALLOWLIST";

/// True bila `base_url` boleh dihubungi: host ada di allowlist, atau SEMUA alamat hasil resolve bersifat publik.
/// URL tidak valid, host kosong, atau resolve gagal dianggap ditolak (gagal tertutup).
pub async fn destination_allowed(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if host_allowlisted(host, env::var(ALLOWLIST_ENV).ok().as_deref()) {
        return true;
    }
    let port = url.port_or_known_default().unwrap_or(443);
    tokio::net::lookup_host((host, port))
        .await
        .is_ok_and(|addresses| addresses.map(|address| address.ip()).all(public_ip))
}

/// Cocokkan host persis terhadap daftar allowlist (tanpa wildcard, tanpa pencocokan sebagian).
pub fn host_allowlisted(host: &str, allowlist: Option<&str>) -> bool {
    allowlist.is_some_and(|value| {
        value
            .split(',')
            .map(str::trim)
            .any(|allowed| allowed == host)
    })
}

/// Alamat publik yang boleh dihubungi: bukan privat, loopback, link-local, multicast, broadcast, dokumentasi, atau unspecified.
pub fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_documentation()
                && !address.is_multicast()
                && !address.is_unspecified()
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !address.is_multicast()
                && !address.is_unspecified()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_matches_exact_hosts_only() {
        assert!(host_allowlisted("localhost", Some("127.0.0.1, localhost")));
        assert!(!host_allowlisted("local", Some("localhost")));
        assert!(!host_allowlisted("evil.localhost", Some("localhost")));
        assert!(!host_allowlisted("localhost", None));
        assert!(!host_allowlisted("localhost", Some("")));
    }

    #[test]
    fn private_loopback_and_metadata_addresses_are_not_public() {
        for blocked in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(!public_ip(blocked.parse().unwrap()), "{blocked}");
        }
        for allowed in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(allowed.parse().unwrap()), "{allowed}");
        }
    }

    #[tokio::test]
    async fn invalid_and_unresolvable_destinations_fail_closed() {
        // Operator yang mengekspor allowlist (mis. 127.0.0.1) sengaja mengizinkan host itu; asersi di bawah tidak berlaku.
        if env::var(ALLOWLIST_ENV).is_ok() {
            return;
        }
        assert!(!destination_allowed("not a url").await);
        assert!(!destination_allowed("file:///etc/passwd").await);
        assert!(!destination_allowed("http://127.0.0.1:9/v1").await);
        assert!(!destination_allowed("http://[::1]/v1").await);
        assert!(!destination_allowed("http://169.254.169.254/latest").await);
    }
}
