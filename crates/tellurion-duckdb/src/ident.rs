//! Whitelist validation + quoting for identifiers spliced directly into SQL
//! text — DuckDB, like PostgreSQL and SQLite, uses ANSI double quotes for a
//! delimited identifier, so this mirrors `tellurion-geopackage::ident`
//! exactly. Table/column names originate from operator-authored config or
//! this driver's own catalog introspection, not request input — but every
//! value that ends up inside a query string (rather than a bound `?`
//! parameter) still passes this whitelist first, the same defense-in-depth
//! every SQL builder in this workspace applies.

use crate::error::{DuckdbDriverError, Result};

/// DuckDB has no hard identifier length ceiling; 63 is kept anyway so a
/// config typo or overlong operator-supplied name fails the same way it
/// would against any other driver in this workspace, rather than silently
/// working differently here.
const MAX_IDENT_LEN: usize = 63;

fn validate_charset(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_IDENT_LEN {
        return Err(DuckdbDriverError::InvalidIdentifier(value.to_string()));
    }
    let mut chars = value.chars();
    let first = chars.next().expect("checked non-empty above");
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(DuckdbDriverError::InvalidIdentifier(value.to_string()));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(DuckdbDriverError::InvalidIdentifier(value.to_string()));
    }
    Ok(())
}

/// Double-quotes a value for use as a table/column reference. The whitelist
/// above excludes `"`, so no escaping step is needed on top of it.
pub(crate) fn quote_ident(value: &str) -> Result<String> {
    validate_charset(value)?;
    Ok(format!("\"{value}\""))
}

/// Single-quotes a value for use as a DuckDB string literal (the `table_name`
/// argument `pragma_table_info`/`duckdb_constraints()` filters take a real
/// SQL string, not a bound parameter position DuckDB's table functions
/// accept). Applies the identical whitelist as [`quote_ident`] first — the
/// value is always a table name that already passed identifier validation,
/// never arbitrary text — so no embedded-quote escaping step can ever be
/// reached in practice; this exists only so a literal is built the same
/// validated way an identifier is, not as a general string-literal escaper.
pub(crate) fn quote_literal(value: &str) -> Result<String> {
    validate_charset(value)?;
    Ok(format!("'{value}'"))
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
    fn literal_quotes_a_plain_table_name() {
        assert_eq!(quote_literal("demo").unwrap(), "'demo'");
    }

    #[test]
    fn literal_rejects_the_same_invalid_input_as_ident() {
        assert!(quote_literal("1table").is_err());
    }
}
