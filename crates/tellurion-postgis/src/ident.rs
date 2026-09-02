//! Whitelist validation + quoting for identifiers and literals spliced
//! directly into SQL text. Table/column names originate from operator-
//! authored config, not request input — but every value that ends up inside
//! a query string (rather than a bound parameter) still passes this
//! whitelist first. This is defense in depth against more than malice: a
//! config typo that would otherwise produce a syntactically valid but wrong
//! identifier fails loudly at query-build time instead of a confusing SQL
//! error mid-request.

use crate::error::{PostgisError, Result};

/// `NAMEDATALEN` on a stock PostgreSQL build is 64, leaving 63 usable bytes.
const MAX_IDENT_LEN: usize = 63;

fn validate_charset(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_IDENT_LEN {
        return Err(PostgisError::InvalidIdentifier(value.to_string()));
    }
    let mut chars = value.chars();
    let first = chars.next().expect("checked non-empty above");
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(PostgisError::InvalidIdentifier(value.to_string()));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(PostgisError::InvalidIdentifier(value.to_string()));
    }
    Ok(())
}

/// Double-quotes a value for use as a table/column reference. The whitelist
/// above excludes `"`, so no escaping step is needed on top of it.
pub(crate) fn quote_ident(value: &str) -> Result<String> {
    validate_charset(value)?;
    Ok(format!("\"{value}\""))
}

/// Single-quotes a value for use as a SQL string literal (a jsonb key in a
/// `-` delete-key expression). Same whitelist as [`quote_ident`] — it
/// excludes `'`, so no escaping is needed. Deliberately identifier-charset
/// restricted: every caller of this function passes a real column name
/// (`resolved_pk`/`resolved_geometry`), which is always identifier-shaped
/// already — see [`quote_sql_string`] for a value with no such shape.
pub(crate) fn quote_literal(value: &str) -> Result<String> {
    validate_charset(value)?;
    Ok(format!("'{value}'"))
}

/// Single-quotes and escapes `value` for safe use as an arbitrary SQL string
/// literal (an `ST_AsMVT` layer name — a collection's `external_id`, which,
/// unlike an identifier, is free-form config text with no charset
/// restriction: hyphens, spaces, anything `AppConfig::validate` doesn't
/// reject, `#49`). Doubles any embedded `'`, the standard SQL string-literal
/// escape — the correct way to make an arbitrary string literal-safe,
/// unlike [`quote_literal`]'s identifier-charset whitelist, which would
/// wrongly reject an ordinary external id like `"public-demo"`. Never fails:
/// any string is escapable.
pub(crate) fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_a_plain_identifier() {
        assert_eq!(quote_ident("demo_table").unwrap(), "\"demo_table\"");
    }

    #[test]
    fn rejects_embedded_quote_attempt() {
        assert!(quote_ident("demo\"; DROP TABLE x; --").is_err());
    }

    #[test]
    fn rejects_leading_digit() {
        assert!(quote_ident("1table").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(quote_ident("").is_err());
    }

    #[test]
    fn rejects_too_long() {
        let long = "a".repeat(MAX_IDENT_LEN + 1);
        assert!(quote_ident(&long).is_err());
    }

    #[test]
    fn accepts_leading_underscore() {
        assert!(quote_ident("_private").is_ok());
    }

    #[test]
    fn quotes_a_literal() {
        assert_eq!(quote_literal("geom").unwrap(), "'geom'");
    }

    #[test]
    fn rejects_literal_with_embedded_quote_attempt() {
        assert!(quote_literal("geom'; --").is_err());
    }

    #[test]
    fn quote_sql_string_accepts_a_hyphenated_external_id() {
        assert_eq!(quote_sql_string("public-demo"), "'public-demo'");
    }

    #[test]
    fn quote_sql_string_escapes_an_embedded_quote_instead_of_rejecting_it() {
        assert_eq!(quote_sql_string("o'brien"), "'o''brien'");
    }

    #[test]
    fn quote_sql_string_accepts_an_empty_value() {
        assert_eq!(quote_sql_string(""), "''");
    }
}
