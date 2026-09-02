//! The `load` path: shells out to the host's `ogr2ogr` binary. `ogr2ogr`
//! owns its own DDL (creates the table); this module is only responsible
//! for invoking it correctly, `ANALYZE`ing the result so
//! `pg_class.reltuples` (the fast-path estimate `tellurion-postgis` reports
//! as `numberMatched`) is populated instead of left at Postgres's "never
//! analyzed" sentinel, and surfacing a clear error if `ogr2ogr` is missing.

use std::path::Path;
use std::process::Command;

pub async fn load(
    path: &Path,
    table: &str,
    db_url: &str,
    layer: Option<&str>,
) -> anyhow::Result<()> {
    let pg_dsn = format!("PG:{}", to_libpq_keyword_value(db_url)?);

    let mut cmd = Command::new("ogr2ogr");
    cmd.arg("-f")
        .arg("PostgreSQL")
        .arg(&pg_dsn)
        .arg(path)
        .arg("-nln")
        .arg(table)
        .arg("-lco")
        .arg("GEOMETRY_NAME=geom")
        .arg("-nlt")
        .arg("PROMOTE_TO_MULTI")
        .arg("--config")
        .arg("PG_USE_COPY")
        .arg("YES")
        .arg("-progress");

    if let Some(layer) = layer {
        cmd.arg(layer);
    }

    tracing::info!(table, source = %path.display(), "loading dataset via ogr2ogr");

    let status = cmd.status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "'ogr2ogr' not found on PATH. Install GDAL (it ships the `ogr2ogr` \
                 command-line tool) and make sure it's on PATH, then retry."
            )
        } else {
            anyhow::anyhow!("failed to run 'ogr2ogr': {err}")
        }
    })?;

    if !status.success() {
        anyhow::bail!(
            "ogr2ogr exited with {status} while loading '{}' into table '{table}'",
            path.display()
        );
    }

    let client = crate::db::connect_url(db_url).await?;
    client
        .batch_execute(&format!("ANALYZE \"{table}\""))
        .await
        .map_err(|err| anyhow::anyhow!("analyzing table '{table}' after ogr2ogr load: {err}"))?;

    Ok(())
}

/// Rewrites a `postgres://user:pass@host:port/dbname?param=value` URL into
/// libpq keyword/value form (`key='value' ...`). GDAL's PostgreSQL driver
/// appends its own `application_name='GDAL x.y.z'` to whatever connection
/// string it is given; that concatenation only produces a valid libpq
/// conninfo when the input is already keyword/value — appended onto a URI
/// it breaks parsing ("unexpected spaces found in ..."). Keyword/value form
/// is immune to that because it is itself just space-separated `key=value`
/// pairs.
fn to_libpq_keyword_value(url: &str) -> anyhow::Result<String> {
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .ok_or_else(|| anyhow::anyhow!("'{url}' is not a postgres:// or postgresql:// URL"))?;

    let (authority_and_path, query) = match rest.split_once('?') {
        Some((left, right)) => (left, Some(right)),
        None => (rest, None),
    };

    let (userinfo, host_and_path) = match authority_and_path.split_once('@') {
        Some((left, right)) => (Some(left), right),
        None => (None, authority_and_path),
    };

    let (host_port, db_path) = host_and_path.split_once('/').unwrap_or((host_and_path, ""));
    let dbname = db_path.trim_start_matches('/');

    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (host_port, None),
    };

    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(userinfo) = userinfo {
        let (user, password) = match userinfo.split_once(':') {
            Some((u, p)) => (u, Some(p)),
            None => (userinfo, None),
        };
        if !user.is_empty() {
            pairs.push(("user".to_string(), percent_decode(user)));
        }
        if let Some(password) = password {
            pairs.push(("password".to_string(), percent_decode(password)));
        }
    }
    if !host.is_empty() {
        pairs.push(("host".to_string(), percent_decode(host)));
    }
    if let Some(port) = port {
        pairs.push(("port".to_string(), port.to_string()));
    }
    if !dbname.is_empty() {
        pairs.push(("dbname".to_string(), percent_decode(dbname)));
    }
    if let Some(query) = query {
        for param in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = param.split_once('=').unwrap_or((param, ""));
            pairs.push((percent_decode(key), percent_decode(value)));
        }
    }

    Ok(pairs
        .into_iter()
        .map(|(k, v)| format!("{k}='{}'", escape_libpq_value(&v)))
        .collect::<Vec<_>>()
        .join(" "))
}

/// Escapes a value for libpq's single-quoted keyword/value syntax: a
/// backslash escapes the following character, so literal backslashes and
/// single quotes both need escaping (in that order).
fn escape_libpq_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Minimal percent-decoding for the URL components libpq DSNs commonly
/// carry (no external dependency needed for this narrow, self-contained use).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_full_url_to_keyword_value_form() {
        let dsn = to_libpq_keyword_value("postgres://tellurion:tellurion@localhost:5433/tellurion")
            .unwrap();
        assert_eq!(
            dsn,
            "user='tellurion' password='tellurion' host='localhost' port='5433' dbname='tellurion'"
        );
    }

    #[test]
    fn works_without_credentials() {
        let dsn = to_libpq_keyword_value("postgres://localhost/tellurion").unwrap();
        assert_eq!(dsn, "host='localhost' dbname='tellurion'");
    }

    #[test]
    fn accepts_postgresql_scheme() {
        let dsn = to_libpq_keyword_value("postgresql://localhost:5432/db").unwrap();
        assert_eq!(dsn, "host='localhost' port='5432' dbname='db'");
    }

    #[test]
    fn carries_query_params_through() {
        let dsn = to_libpq_keyword_value("postgres://localhost/db?sslmode=require").unwrap();
        assert_eq!(dsn, "host='localhost' dbname='db' sslmode='require'");
    }

    #[test]
    fn percent_decodes_credentials() {
        let dsn = to_libpq_keyword_value("postgres://us%40er:p%40ss@localhost/db").unwrap();
        assert_eq!(
            dsn,
            "user='us@er' password='p@ss' host='localhost' dbname='db'"
        );
    }

    #[test]
    fn escapes_embedded_quotes_and_backslashes() {
        let dsn = to_libpq_keyword_value("postgres://us'er:p%5Css@localhost/db").unwrap();
        assert_eq!(
            dsn,
            r"user='us\'er' password='p\\ss' host='localhost' dbname='db'"
        );
    }

    #[test]
    fn rejects_non_postgres_scheme() {
        assert!(to_libpq_keyword_value("mysql://localhost/db").is_err());
    }
}
