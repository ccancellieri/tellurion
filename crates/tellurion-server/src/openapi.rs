//! `/api`: a hand-written OpenAPI 3.0 document per protocol root (`#39`) —
//! `openapi_features.json`, `openapi_tiles.json`, `openapi_styles.json`,
//! `openapi_threedtiles.json`, `openapi_stac.json`, `openapi_records.json`,
//! `openapi_processes.json`, one per `Protocol`,
//! covering exactly the resources that root serves. Each is embedded at
//! compile time so it always ships with the binary and never depends on a
//! runtime file path; which one a request sees comes from the
//! `Extension<Protocol>` `app::build` layers onto that root's sub-router.
//!
//! "Exactly the resources that root serves" used to be an intention these
//! documents drifted away from (`#225`: whole verbs, whole resources, and a
//! handful of honoured query parameters served but never described, while
//! every root advertises the `oas30` conformance class and OGC API Features
//! Part 1 Requirement 8 makes a server's own API definition the authority on
//! which query parameters it may honour). It is now enforced: the tests at
//! the bottom of this module read the routes each protocol crate's `router()`
//! actually registers and require the matching document to describe those
//! paths and methods, and only those. A new endpoint therefore lands with its
//! documentation or it doesn't land — see those tests' own doc for how they
//! read the router and what they deliberately leave to review.

use axum::extract::Extension;
use axum::http::header;
use axum::response::IntoResponse;

use crate::protocol::Protocol;

const OPENAPI_MEDIA_TYPE: &str = "application/vnd.oai.openapi+json;version=3.0";

const FEATURES_OPENAPI_JSON: &str = include_str!("../openapi_features.json");
const TILES_OPENAPI_JSON: &str = include_str!("../openapi_tiles.json");
const STYLES_OPENAPI_JSON: &str = include_str!("../openapi_styles.json");
const THREEDTILES_OPENAPI_JSON: &str = include_str!("../openapi_threedtiles.json");
const STAC_OPENAPI_JSON: &str = include_str!("../openapi_stac.json");
const RECORDS_OPENAPI_JSON: &str = include_str!("../openapi_records.json");
const PROCESSES_OPENAPI_JSON: &str = include_str!("../openapi_processes.json");

fn document_for(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Features => FEATURES_OPENAPI_JSON,
        Protocol::Tiles => TILES_OPENAPI_JSON,
        Protocol::Styles => STYLES_OPENAPI_JSON,
        Protocol::ThreeDTiles => THREEDTILES_OPENAPI_JSON,
        Protocol::Stac => STAC_OPENAPI_JSON,
        Protocol::Records => RECORDS_OPENAPI_JSON,
        Protocol::Processes => PROCESSES_OPENAPI_JSON,
    }
}

pub async fn api_doc(Extension(protocol): Extension<Protocol>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, OPENAPI_MEDIA_TYPE)],
        document_for(protocol),
    )
}

/// `#225`'s structural safety net: the documents above are hand-written, and
/// they have already drifted once (whole verbs and whole resources served but
/// never described, while all five roots advertise the `oas30` conformance
/// class and Features Part 1 Requirement 8 says a server SHALL reject query
/// parameters *its own API definition* doesn't name). These tests make that
/// drift a build failure instead of a discovery.
///
/// ## Where the truth comes from
///
/// Not from a second hand-written list — from the live `axum::Router` each
/// protocol crate returns. Axum exposes no public route iterator, but its
/// `Debug` impl renders the whole path router, including
/// `node: Node { paths: {RouteId(n): "/collections/{cid}/items", ...} }` and,
/// per route, the `allow_header` axum itself would answer a `405` with
/// (`Bytes(b"GET,HEAD,POST")`). Parsing that rendering is the one seam that
/// reaches real registration data without either duplicating the route list
/// or standing up an `AppContext` to make live requests. It is deliberately
/// defensive: if a future axum renders it differently, [`registered_routes`]
/// panics naming this module rather than silently asserting over an empty
/// set.
///
/// ## What is compared
///
/// Path *shape* and *method set*, per protocol root, in both directions: every
/// registered route must appear in that root's document, and the document must
/// promise nothing that isn't registered. Path parameter *names* are erased
/// (`{cid}` and `{collectionId}` both normalize to `{}`) because the router's
/// capture names are internal and the document's are the published OGC ones;
/// a literal segment never normalizes to a parameter, so replacing a
/// parameterized path with a hardcoded one (the `WebMercatorQuad` case `#225`
/// also fixes) still fails here.
///
/// ## What is NOT covered
///
/// Query parameters, headers, request bodies, response codes and media types
/// are still checked by review alone: axum parses those inside each handler's
/// own extractors (`Query<T>`, `HeaderMap`, an untyped `HashMap` in several
/// handlers), so there is no registration-time artifact to compare a document
/// against, and inventing one would mean the second hand-written list this
/// test exists to avoid. Same for the `/{tenant}/{protocol}/catalogs/{catalog}`
/// mount prefix `app::build` adds: each document describes one protocol root
/// relative to its own mount, which is what OGC API expects, so the prefix is
/// intentionally outside the comparison.
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use axum::Router;
    use serde_json::Value;

    use super::{document_for, Protocol};

    /// One route's published shape: the path as registered, and the methods
    /// it answers.
    type RouteTable = BTreeMap<String, BTreeSet<String>>;

    /// Methods axum synthesizes rather than a route ever declaring them —
    /// `HEAD` off every `GET`, `OPTIONS` off every route — dropped from both
    /// sides so a document is never expected to enumerate them.
    const SYNTHETIC_METHODS: [&str; 2] = ["HEAD", "OPTIONS"];

    /// The OGC API Common resources `app::protocol_root`/`app::stac_root` add
    /// to every protocol root, outside any protocol crate's own `router()`.
    const PROTOCOL_ROOT_PATHS: [&str; 3] = ["/", "/conformance", "/api"];

    /// Whichever of axum's `Debug` renderings this parser depends on breaking
    /// should say so out loud, at this module, rather than degrade into an
    /// empty comparison that passes.
    fn drift_panic(what: &str) -> ! {
        panic!(
            "axum's Router Debug rendering no longer exposes {what}; \
             the openapi.rs route-coverage tests need updating to match it"
        )
    }

    /// The section of a `Router`'s `Debug` rendering describing the routes it
    /// serves — deliberately not the `fallback_router` that follows it, which
    /// carries axum's own internal `/{*__private__axum_fallback}` entry.
    fn path_router_section(dump: &str) -> &str {
        match dump.split_once(", fallback_router:") {
            Some((section, _)) => section,
            None => drift_panic("a `fallback_router` field to cut the path router at"),
        }
    }

    /// `RouteId(4): "/collections/{cid}/items"` pairs out of the `node`
    /// sub-section.
    fn route_id_paths(section: &str) -> BTreeMap<u32, String> {
        let Some((_, node)) = section.split_once("node: Node { paths: {") else {
            drift_panic("a `node: Node { paths: ... }` route-id-to-path map")
        };
        let mut out = BTreeMap::new();
        let mut rest = node;
        while let Some((id, after)) = next_route_id(rest) {
            let Some((_, quoted)) = after.split_once('"') else {
                drift_panic("quoted path strings in its route-id-to-path map")
            };
            let Some((path, tail)) = quoted.split_once('"') else {
                drift_panic("terminated path strings in its route-id-to-path map")
            };
            out.insert(id, path.to_string());
            rest = tail;
        }
        out
    }

    /// The method set of each route in the `routes` sub-section, read off the
    /// same `allow_header` axum itself answers a `405` with.
    fn route_id_methods(section: &str) -> BTreeMap<u32, BTreeSet<String>> {
        let Some((_, routes)) = section.split_once("routes: {") else {
            drift_panic("a `routes: { ... }` endpoint map")
        };
        let routes = routes.split("node: Node").next().unwrap_or_default();
        let mut out = BTreeMap::new();
        let mut rest = routes;
        while let Some((id, after)) = next_route_id(rest) {
            let Some((_, allow)) = after.split_once("allow_header: ") else {
                drift_panic("an `allow_header` on each of its method routers")
            };
            let methods = match allow.strip_prefix("Bytes(b\"") {
                Some(value) => value
                    .split('"')
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|method| !method.is_empty())
                    .filter(|method| !SYNTHETIC_METHODS.contains(method))
                    .map(str::to_string)
                    .collect(),
                // `AllowHeader::None`/`Skip`: a route that declares no method
                // filter at all. No route in this workspace is built that way,
                // and an empty method set below reports it as a mismatch
                // rather than passing silently.
                None => BTreeSet::new(),
            };
            out.insert(id, methods);
            rest = allow;
        }
        out
    }

    /// `RouteId(7): ` -> `(7, rest-after-the-colon)`.
    fn next_route_id(haystack: &str) -> Option<(u32, &str)> {
        let start = haystack.find("RouteId(")?;
        let after = &haystack[start + "RouteId(".len()..];
        let (digits, rest) = after.split_once(')')?;
        let id = digits
            .parse()
            .unwrap_or_else(|_| drift_panic("numeric route ids"));
        Some((id, rest))
    }

    /// Every path one protocol crate's `router()` actually serves, with the
    /// methods each answers.
    fn registered_routes<S>(router: &Router<S>) -> RouteTable {
        let dump = format!("{router:?}");
        let section = path_router_section(&dump);
        let paths = route_id_paths(section);
        let methods = route_id_methods(section);
        if paths.is_empty() {
            drift_panic("any registered path at all");
        }
        paths
            .into_iter()
            .map(|(id, path)| {
                let methods = methods.get(&id).cloned().unwrap_or_default();
                assert!(
                    !methods.is_empty(),
                    "route '{path}' registered no methods; either axum's Debug \
                     rendering changed or this route really answers nothing"
                );
                (erase_parameter_names(&path), methods)
            })
            .collect()
    }

    /// Every path one document promises, with the methods it promises on each.
    fn documented_routes(document: &str) -> RouteTable {
        let parsed: Value = serde_json::from_str(document).expect("document is valid JSON");
        let paths = parsed["paths"]
            .as_object()
            .expect("document has a paths object");
        paths
            .iter()
            .map(|(path, item)| {
                let methods: BTreeSet<String> = item
                    .as_object()
                    .expect("each path item is an object")
                    .keys()
                    .map(|key| key.to_ascii_uppercase())
                    .filter(|method| !SYNTHETIC_METHODS.contains(&method.as_str()))
                    .filter(|method| {
                        ["GET", "PUT", "POST", "PATCH", "DELETE", "TRACE"]
                            .contains(&method.as_str())
                    })
                    .collect();
                assert!(
                    !methods.is_empty(),
                    "documented path '{path}' declares no operation"
                );
                (erase_parameter_names(path), methods)
            })
            .collect()
    }

    /// `/collections/{cid}/items/{fid}` and `/collections/{collectionId}/items/
    /// {featureId}` are the same shape under different capture names; a
    /// literal segment is never erased, so `/tiles/WebMercatorQuad` and
    /// `/tiles/{tileMatrixSetId}` stay different shapes.
    fn erase_parameter_names(path: &str) -> String {
        path.split('/')
            .map(|segment| {
                if segment.starts_with('{') && segment.ends_with('}') {
                    "{}"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The routes a protocol crate's own `router()` serves, plus the Common
    /// landing/conformance/API resources its root gains from `app.rs`.
    fn served_routes<S>(router: &Router<S>) -> RouteTable {
        let mut table = registered_routes(router);
        for path in PROTOCOL_ROOT_PATHS {
            table.insert(path.to_string(), BTreeSet::from(["GET".to_string()]));
        }
        table
    }

    fn describe(table: &RouteTable) -> Vec<String> {
        table
            .iter()
            .map(|(path, methods)| {
                format!(
                    "{path} [{}]",
                    methods.iter().cloned().collect::<Vec<_>>().join(",")
                )
            })
            .collect()
    }

    fn assert_document_covers<S>(protocol: Protocol, router: Router<S>) {
        let served = served_routes(&router);
        let documented = documented_routes(document_for(protocol));

        let missing: Vec<String> = describe(&served)
            .into_iter()
            .filter(|entry| !describe(&documented).contains(entry))
            .collect();
        let invented: Vec<String> = describe(&documented)
            .into_iter()
            .filter(|entry| !describe(&served).contains(entry))
            .collect();

        assert!(
            missing.is_empty() && invented.is_empty(),
            "{protocol:?}'s OpenAPI document no longer matches what its root serves.\n\
             served but undocumented (or documented with the wrong methods):\n  {}\n\
             documented but not served (or serving different methods):\n  {}",
            if missing.is_empty() {
                "-".to_string()
            } else {
                missing.join("\n  ")
            },
            if invented.is_empty() {
                "-".to_string()
            } else {
                invented.join("\n  ")
            },
        );
    }

    #[test]
    fn features_document_covers_every_served_route() {
        assert_document_covers(Protocol::Features, tellurion_features::router());
    }

    #[test]
    fn tiles_document_covers_every_served_route() {
        assert_document_covers(Protocol::Tiles, tellurion_tiles::router());
    }

    #[test]
    fn styles_document_covers_every_served_route() {
        assert_document_covers(Protocol::Styles, tellurion_styles::router());
    }

    #[test]
    fn threedtiles_document_covers_every_served_route() {
        assert_document_covers(Protocol::ThreeDTiles, tellurion_places::router());
    }

    #[test]
    fn stac_document_covers_every_served_route() {
        assert_document_covers(Protocol::Stac, tellurion_stac::router());
    }

    #[test]
    fn records_document_covers_every_served_route() {
        assert_document_covers(Protocol::Records, tellurion_records::router());
    }

    #[test]
    fn processes_document_covers_every_served_route() {
        assert_document_covers(Protocol::Processes, tellurion_processes::router());
    }

    /// Every embedded document is valid JSON, is an OpenAPI 3.0 document, and
    /// resolves every `#/components/...` reference it makes — a hand-written
    /// document's cheapest way to rot is a `$ref` to a component someone
    /// renamed.
    #[test]
    fn every_document_is_valid_and_self_contained() {
        for protocol in Protocol::ALL {
            let document = document_for(protocol);
            let parsed: Value = serde_json::from_str(document).unwrap_or_else(|error| {
                panic!("{protocol:?}'s document is not valid JSON: {error}")
            });
            let version = parsed["openapi"].as_str().unwrap_or_default();
            assert!(
                version.starts_with("3.0"),
                "{protocol:?}'s document declares OpenAPI version '{version}', \
                 but /api is served as application/vnd.oai.openapi+json;version=3.0"
            );
            for reference in component_references(&parsed) {
                let mut cursor = &parsed;
                for segment in reference.trim_start_matches("#/").split('/') {
                    cursor = &cursor[segment];
                }
                assert!(
                    !cursor.is_null(),
                    "{protocol:?}'s document references '{reference}', which it does not define"
                );
            }
        }
    }

    fn component_references(value: &Value) -> Vec<String> {
        match value {
            Value::Object(map) => map
                .iter()
                .flat_map(|(key, child)| match (key.as_str(), child.as_str()) {
                    ("$ref", Some(target)) => vec![target.to_string()],
                    _ => component_references(child),
                })
                .collect(),
            Value::Array(items) => items.iter().flat_map(component_references).collect(),
            _ => Vec::new(),
        }
    }
}
