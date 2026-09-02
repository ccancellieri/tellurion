//! Deliberately narrow WKT recognition for coordinates we can serve honestly.

use std::{fs, path::Path};

/// Accepts a root WKT1 geographic CRS identified as EPSG:4326 or the canonical
/// ESRI WGS 84 definition used by Natural Earth. CRS84 is deliberately rejected:
/// its longitude/latitude axis contract differs from EPSG:4326 and the core
/// currently cannot express that distinction.
pub(crate) fn epsg(path: Option<&Path>) -> Option<i32> {
    let text = fs::read_to_string(path?).ok()?;
    let upper = text.trim().to_ascii_uppercase();
    if !upper.starts_with("GEOGCS[") {
        return None;
    }
    root_epsg_authority(&upper)
        .filter(|code| *code == 4326)
        .or_else(|| is_natural_earth_wgs84(&upper).then_some(4326))
}

fn is_natural_earth_wgs84(wkt: &str) -> bool {
    let compact = wkt
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    compact
        == "GEOGCS[\"GCS_WGS_1984\",DATUM[\"D_WGS_1984\",SPHEROID[\"WGS_1984\",6378137.0,298.257223563]],PRIMEM[\"GREENWICH\",0.0],UNIT[\"DEGREE\",0.017453292519943295]]"
}

fn root_epsg_authority(wkt: &str) -> Option<i32> {
    let mut depth = 0usize;
    let bytes = wkt.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => depth = depth.checked_sub(1)?,
            _ if depth == 1
                && (wkt[index..].starts_with("AUTHORITY[") || wkt[index..].starts_with("ID[")) =>
            {
                let tail = &wkt[index..];
                let marker = if tail.starts_with("AUTHORITY[") {
                    "AUTHORITY[\"EPSG\",\""
                } else {
                    "ID[\"EPSG\","
                };
                let value = tail.strip_prefix(marker)?;
                let digits = value
                    .trim_start_matches('"')
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>();
                return digits.parse().ok();
            }
            _ => {}
        }
        index += 1;
    }
    None
}
