//! Postgres identifier sanitization shared by table and column naming. Every
//! physical name ingest hands to the database is derived from operator- or
//! dataset-supplied text (collection ids, OGR field names) that may contain
//! characters Postgres identifiers cannot.

/// Postgres truncates identifiers at `NAMEDATALEN - 1` bytes (63 by default).
const MAX_IDENTIFIER_LEN: usize = 63;

/// Lowercases, replaces any non `[a-z0-9_]` byte with `_`, guarantees the
/// result starts with a letter or underscore, and falls back to
/// `"collection"` if nothing usable survives.
pub fn sanitize_identifier(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|ch| {
            let lower = ch.to_ascii_lowercase();
            if lower.is_ascii_alphanumeric() || lower == '_' {
                lower
            } else {
                '_'
            }
        })
        .collect();

    if out.is_empty() {
        out.push_str("collection");
    }

    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }

    out.truncate(MAX_IDENTIFIER_LEN);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_replaces_invalid_chars() {
        assert_eq!(sanitize_identifier("My Cool-Data!23"), "my_cool_data_23");
    }

    #[test]
    fn prefixes_when_leading_char_is_digit() {
        assert_eq!(sanitize_identifier("123abc"), "_123abc");
    }

    #[test]
    fn falls_back_to_collection_when_empty() {
        assert_eq!(sanitize_identifier(""), "collection");
        assert_eq!(sanitize_identifier("!!!"), "___");
    }

    #[test]
    fn truncates_to_postgres_identifier_limit() {
        let long = "a".repeat(100);
        let sanitized = sanitize_identifier(&long);
        assert_eq!(sanitized.len(), MAX_IDENTIFIER_LEN);
    }

    #[test]
    fn already_valid_identifier_is_unchanged() {
        assert_eq!(sanitize_identifier("demo_collection"), "demo_collection");
    }
}
