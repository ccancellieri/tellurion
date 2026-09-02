use anyhow::Context;
use tokio_postgres::Client;

const QUERY: &str = "\
SELECT EXISTS (\
           SELECT 1 FROM geometry_columns \
           WHERE f_table_schema = 'public' AND f_table_name = $1 AND f_geometry_column = $2\
       ), pk.pk_columns, pk.pk_types \
FROM (\
    SELECT COALESCE(array_agg(kcu.column_name ORDER BY kcu.ordinal_position), ARRAY[]::text[]) AS pk_columns, \
           COALESCE(array_agg(c.udt_name ORDER BY kcu.ordinal_position), ARRAY[]::text[]) AS pk_types \
    FROM information_schema.table_constraints tc \
    JOIN information_schema.key_column_usage kcu \
      ON kcu.constraint_schema = tc.constraint_schema AND kcu.constraint_name = tc.constraint_name \
     AND kcu.table_schema = tc.table_schema AND kcu.table_name = tc.table_name \
    JOIN information_schema.columns c \
      ON c.table_schema = kcu.table_schema AND c.table_name = kcu.table_name \
     AND c.column_name = kcu.column_name \
    WHERE tc.table_schema = 'public' AND tc.table_name = $1 \
      AND tc.constraint_type = 'PRIMARY KEY'\
) pk";

pub(super) async fn validate_postgis_reference(
    client: &Client,
    table: &str,
    geometry: &str,
) -> anyhow::Result<String> {
    let row = client
        .query_one(QUERY, &[&table, &geometry])
        .await
        .context("checking referenced PostGIS table")?;
    interpret_postgis_reference(row.get(0), row.get(1), row.get(2))
        .with_context(|| format!("validating PostGIS table 'public.{table}'"))
}

pub(super) fn interpret_postgis_reference(
    geometry_usable: bool,
    primary_key_columns: Vec<String>,
    primary_key_types: Vec<String>,
) -> anyhow::Result<String> {
    if !geometry_usable {
        anyhow::bail!(
            "table was not found or the requested geometry column is not a usable PostGIS geometry"
        );
    }
    match primary_key_columns.as_slice() {
        [] => anyhow::bail!("table has no primary key; a single int4 or int8 key is required"),
        [column] => {
            let key_type = primary_key_types.first().ok_or_else(|| {
                anyhow::anyhow!("database returned a primary key without its type")
            })?;
            if primary_key_types.len() != 1 {
                anyhow::bail!("database returned inconsistent primary key metadata");
            }
            if key_type != "int4" && key_type != "int8" {
                anyhow::bail!(
                    "primary key column '{column}' uses '{key_type}' but must use int4 or int8"
                );
            }
            super::validate_physical_identifier("primary key column", column)?;
            Ok(column.clone())
        }
        columns => anyhow::bail!(
            "table has a composite primary key ({}) but a single int4 or int8 key is required",
            columns.join(", ")
        ),
    }
}
