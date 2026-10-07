//! Redaksi rahasia dari teks (tanpa regex agar tidak menambah dependensi).
//!
//! Yang dikenali: blok kunci privat PEM, nilai rahasia yang didaftarkan (mis. API key provider), token berformat
//! dikenal (`sk-`, `ghp_`, `AKIA…`, `xox…-`, `AIza…`, JWT), nilai setelah `Bearer`, dan penugasan `nama=nilai` /
//! `"nama": "nilai"` untuk nama yang mengandung password/secret/token/api_key dan sejenisnya.
//!
//! Sengaja konservatif terhadap false positive: nilai penugasan harus ≥ 8 karakter (jadi `max_tokens: 1000`
//! tidak tersentuh) dan string heksadesimal/base64 polos TIDAK diredaksi (hash commit adalah data sah).
//! Ini mengurangi kebocoran, bukan pengganti menjaga rahasia keluar dari repository.

use std::{
    borrow::Cow,
    sync::{PoisonError, RwLock},
};

pub const REDACTED: &str = "[REDACTED]";
const MIN_VALUE: usize = 8;
const MAX_REGISTERED: usize = 64;

static REGISTERED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Daftarkan nilai rahasia konkret (mis. API key yang dibaca dari environment) agar selalu diredaksi di mana pun muncul.
/// Nilai pendek diabaikan karena redaksinya akan merusak teks biasa.
pub fn register_secret(value: &str) {
    if value.len() < MIN_VALUE {
        return;
    }
    let mut registered = REGISTERED.write().unwrap_or_else(PoisonError::into_inner);
    if registered.len() < MAX_REGISTERED && !registered.iter().any(|known| known == value) {
        registered.push(value.to_owned());
    }
}

pub fn redact(input: &str) -> Cow<'_, str> {
    let mut text = redact_private_keys(input);
    text = redact_registered(text);
    text = redact_known_tokens(text);
    text = redact_assignments(text);
    if text == input {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(text)
    }
}

pub fn redact_bytes(input: &[u8]) -> Cow<'_, [u8]> {
    match std::str::from_utf8(input) {
        Ok(text) => match redact(text) {
            Cow::Borrowed(_) => Cow::Borrowed(input),
            Cow::Owned(text) => Cow::Owned(text.into_bytes()),
        },
        // Bukan UTF-8 (biner): tidak ada yang bisa dikenali dengan aman; biarkan apa adanya.
        Err(_) => Cow::Borrowed(input),
    }
}

fn redact_private_keys(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("-----BEGIN ") {
        let after = &rest[start..];
        let Some(header_end) = after
            .find("-----\n")
            .or_else(|| after[11..].find("-----").map(|i| i + 11))
        else {
            break;
        };
        if !after[..header_end].contains("PRIVATE KEY") {
            output.push_str(&rest[..start + 11]);
            rest = &rest[start + 11..];
            continue;
        }
        output.push_str(&rest[..start]);
        output.push_str("[REDACTED PRIVATE KEY]");
        rest = match after.find("-----END ") {
            Some(end) => {
                let tail = &after[end + 9..];
                // Lewati sisa baris penutup `...PRIVATE KEY-----`.
                &tail[tail.find("-----").map_or(tail.len(), |i| i + 5)..]
            }
            None => "",
        };
    }
    output.push_str(rest);
    output
}

fn redact_registered(text: String) -> String {
    let registered = REGISTERED.read().unwrap_or_else(PoisonError::into_inner);
    registered
        .iter()
        .fold(text, |text, secret| text.replace(secret.as_str(), REDACTED))
}

fn is_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '+' | '/')
}

fn is_known_token(word: &str) -> bool {
    let long = |prefix: &str, min: usize| word.starts_with(prefix) && word.len() >= min;
    long("sk-", 20)
        || long("sk_live_", 16)
        || long("sk_test_", 16)
        || ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
            .iter()
            .any(|p| long(p, 30))
        || long("github_pat_", 30)
        || ["xoxb-", "xoxp-", "xoxa-", "xoxs-"]
            .iter()
            .any(|p| long(p, 20))
        || (word.starts_with("AIza") && word.len() == 39)
        || ((word.starts_with("AKIA") || word.starts_with("ASIA"))
            && word.len() == 20
            && word
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
        || (word.starts_with("eyJ") && word.len() >= 30 && word.matches('.').count() == 2)
}

fn redact_known_tokens(text: String) -> String {
    let mut output = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, output: &mut String| {
        if is_known_token(word) {
            output.push_str(REDACTED);
        } else {
            output.push_str(word);
        }
        word.clear();
    };
    for character in text.chars() {
        if is_word(character) {
            word.push(character);
        } else {
            flush(&mut word, &mut output);
            output.push(character);
        }
    }
    flush(&mut word, &mut output);
    output
}

const NAME_MARKERS: [&str; 9] = [
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "password",
    "passwd",
    "token",
    "private_key",
    "access_key",
];

fn value_end(text: &str, from: usize) -> usize {
    text[from..]
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '}' | ')' | '`'))
        .map_or(text.len(), |i| from + i)
}

fn redact_assignments(text: String) -> String {
    let lower = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        // Kandidat berikutnya: nama penanda rahasia atau kata `bearer`.
        let next = NAME_MARKERS
            .iter()
            .chain(std::iter::once(&"bearer"))
            .filter_map(|marker| lower[cursor..].find(marker).map(|i| (cursor + i, *marker)))
            .min_by_key(|(position, _)| *position);
        let Some((position, marker)) = next else {
            break;
        };
        let bytes = text.as_bytes();
        let mut index = position + marker.len();
        if marker == "bearer" {
            if bytes.get(index) != Some(&b' ') {
                output.push_str(&text[cursor..index]);
                cursor = index;
                continue;
            }
            index += 1;
        } else {
            // Habiskan sisa nama (mis. `_value`), kutip penutup, spasi, lalu wajib `:` atau `=`.
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-'))
            {
                index += 1;
            }
            if matches!(bytes.get(index), Some(b'"' | b'\'')) {
                index += 1;
            }
            while bytes.get(index) == Some(&b' ') {
                index += 1;
            }
            if !matches!(bytes.get(index), Some(b':' | b'=')) {
                output.push_str(&text[cursor..index.min(text.len())]);
                cursor = index.min(text.len());
                continue;
            }
            index += 1;
            while bytes.get(index) == Some(&b' ') {
                index += 1;
            }
            if matches!(bytes.get(index), Some(b'"' | b'\'')) {
                index += 1;
            }
        }
        let end = value_end(&text, index);
        if end - index >= MIN_VALUE
            && &text[index..end] != REDACTED
            && !text[index..end].starts_with("[REDACTED")
        {
            output.push_str(&text[cursor..index]);
            output.push_str(REDACTED);
            cursor = end;
        } else {
            output.push_str(&text[cursor..index.max(cursor)]);
            cursor = index.max(cursor);
        }
    }
    output.push_str(&text[cursor..]);
    output
}
