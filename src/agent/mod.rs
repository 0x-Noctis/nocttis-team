pub mod integrator;
pub mod lead;
pub mod reviewer;
pub mod verifier;
pub mod worker;

/// Model nyata sering membungkus JSON dengan pagar Markdown (```json) atau kalimat pengantar walau diminta JSON murni.
/// `extract` mengambil objek JSON terluar saja; validasi ketat (skema, `deny_unknown_fields`) tetap dilakukan pemanggil.
pub mod json_text {
    pub fn extract(text: &str) -> &str {
        let trimmed = text.trim();
        let unfenced = match trimmed.strip_prefix("```") {
            Some(rest) => {
                let body = rest.split_once('\n').map_or(rest, |(_, body)| body);
                body.trim_end().strip_suffix("```").unwrap_or(body).trim()
            }
            None => trimmed,
        };
        match (unfenced.find('{'), unfenced.rfind('}')) {
            (Some(start), Some(end)) if end >= start => &unfenced[start..=end],
            _ => unfenced,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::extract;

        #[test]
        fn strips_fences_and_surrounding_prose() {
            assert_eq!(extract(r#"{"a":1}"#), r#"{"a":1}"#);
            assert_eq!(extract("```json\n{\"a\":1}\n```"), r#"{"a":1}"#);
            assert_eq!(extract("```\n{\"a\":1}```"), r#"{"a":1}"#);
            assert_eq!(
                extract("Berikut hasilnya:\n{\"a\":{\"b\":2}}\nSelesai."),
                r#"{"a":{"b":2}}"#
            );
            assert_eq!(extract("tanpa objek"), "tanpa objek");
        }
    }
}
