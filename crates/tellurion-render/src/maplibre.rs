//! MapLibre Style JSON -> [`LayerPaint`] conversion. This is the one place
//! in the workspace that knows MapLibre paint-property names and value
//! syntax; every crate that wants a styled tile calls
//! [`resolve_layer_paints`] rather than re-parsing style JSON itself. Pure,
//! like the rest of this crate: no I/O, just `serde_json::Value` in,
//! [`LayerPaint`]s out.
//!
//! Scope (v1, matching the design doc): `fill-color`, `fill-opacity`,
//! `line-color`, `line-width`, `circle-color`, `circle-radius`. Other paint
//! properties (`fill-outline-color`, `circle-opacity`, `line-opacity`, text
//! styling, ...) are not read. Layer types other than `fill`/`line`/`circle`
//! (`background`, `symbol`, `raster`, `heatmap`, ...) are registered with an
//! unstyled entry (so a later style layer on the same `source-layer` still
//! has something to merge onto) but contribute no paint of their own.
//!
//! Any of those six properties may be a zoom-driven `step`/`interpolate`
//! expression rather than a literal; [`resolve_layer_paints`] takes the
//! zoom it is resolving for and evaluates them there ([`eval_zoom_expr`]).
//! Feature-driven expressions are refused, not approximated — a paint is
//! resolved per MVT source-layer, so no feature exists to evaluate one
//! against.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::style::parse_css_hex_color;
use crate::styled::LayerPaint;

/// MapLibre's own spec default for `line-width` when the property is
/// present on a `paint` object of the wrong shape is irrelevant here — this
/// is the value an entry starts at before any style layer sets a real
/// `line-width`, matching the Mapbox/MapLibre Style Spec's documented
/// default (verified 2026-07 against docs.mapbox.com/style-spec/reference).
const DEFAULT_STROKE_WIDTH: f32 = 1.0;
/// MapLibre spec default for `circle-radius` (verified 2026-07).
const DEFAULT_POINT_RADIUS: f32 = 5.0;

/// Starting point for every `source-layer` entry: fully transparent fill
/// and stroke (an unstyled property must draw nothing, not an arbitrary
/// guessed color) with the spec-default width/radius so a layer that only
/// sets `line-color` still strokes at a sane width.
const UNSTYLED: LayerPaint = LayerPaint {
    fill_rgba: [0, 0, 0, 0],
    stroke_rgba: [0, 0, 0, 0],
    stroke_width: DEFAULT_STROKE_WIDTH,
    point_radius: DEFAULT_POINT_RADIUS,
};

/// Every `source-layer` this style document's layers name — the exact set of
/// keys [`resolve_layer_paints`] would produce, read without building the
/// paints (`#220`, `#245`).
///
/// Lives beside `resolve_layer_paints` rather than in a caller because this
/// crate is the one place that knows the style document's shape, and because
/// "which MVT layers does this style paint" must answer identically for the
/// renderer and for whoever decides to advertise the style at all.
pub fn source_layers(style_doc: &Value) -> impl Iterator<Item = &str> {
    style_doc
        .get("layers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|layer| layer.get("source-layer").and_then(Value::as_str))
}

/// Whether `style_doc` paints anything at all on a tileset made of
/// `layer_names` — the single definition of style applicability (`#245`).
///
/// A MapLibre style keys every layer's paint by `source-layer`
/// ([`resolve_layer_paints`]), so a style naming no layer this tileset
/// carries renders a blank tile. Advertising it — as a `map`-rel link on the
/// TileSet resource, or as a `stylesheet`/`map` link contributed onto a
/// Collection document — is a promise of a picture that would come back
/// empty. `#220` established this rule on the link-contributor side;
/// `#245` brought the TileSet resource under the same one, through this
/// shared predicate rather than a second implementation of it.
///
/// A style that names no `source-layer` whatsoever (a background-only
/// document) applies to nothing: the empty intersection is the honest
/// answer, not a special case.
pub fn style_paints_any_layer(style_doc: &Value, layer_names: &BTreeSet<String>) -> bool {
    source_layers(style_doc).any(|name| layer_names.contains(name))
}

/// Reads a MapLibre Style JSON document's `layers` array and produces one
/// merged [`LayerPaint`] per `source-layer` — the MVT layer name a style
/// layer targets, and the same key `render_mvt_to_png_styled` looks tiles up
/// by. Style layers without a `source-layer` are skipped (there is no MVT
/// layer to map them to). When two style layers target the same
/// `source-layer` (e.g. a `fill` layer and a `line` outline layer both
/// reading from `buildings`), later layers only overwrite the fields their
/// own paint object actually sets — see [`UNSTYLED`].
///
/// `zoom` is the zoom level the resulting paints are for: every
/// zoom-driven `step`/`interpolate` paint expression in the document is
/// evaluated **at that zoom** ([`eval_zoom_expr`]), so the same style
/// document resolves to different paints at different zoom levels, the way
/// MapLibre itself renders it. There is no zoom-less form of this call:
/// a `["interpolate", ["linear"], ["zoom"], 5, 1, 15, 4]` `line-width` has
/// no single correct answer without one, and `#174` exists because taking
/// the first stop regardless of zoom silently rendered every zoom level at
/// the widest-scale end of the ramp.
pub fn resolve_layer_paints(style_doc: &Value, zoom: f64) -> BTreeMap<String, LayerPaint> {
    let mut paints: BTreeMap<String, LayerPaint> = BTreeMap::new();
    let Some(layers) = style_doc.get("layers").and_then(Value::as_array) else {
        return paints;
    };

    for layer in layers {
        let Some(source_layer) = layer.get("source-layer").and_then(Value::as_str) else {
            continue;
        };
        let layer_type = layer.get("type").and_then(Value::as_str).unwrap_or("");
        let paint_obj = layer.get("paint");
        let entry = paints.entry(source_layer.to_string()).or_insert(UNSTYLED);

        match layer_type {
            "fill" => {
                if let Some(rgba) = resolve_color(paint_obj, "fill-color", zoom) {
                    entry.fill_rgba = rgba;
                }
                if let Some(opacity) = resolve_number(paint_obj, "fill-opacity", zoom) {
                    entry.fill_rgba[3] = apply_opacity(entry.fill_rgba[3], opacity);
                }
            }
            "line" => {
                if let Some(rgba) = resolve_color(paint_obj, "line-color", zoom) {
                    entry.stroke_rgba = rgba;
                }
                if let Some(width) = resolve_number(paint_obj, "line-width", zoom) {
                    entry.stroke_width = width as f32;
                }
            }
            "circle" => {
                if let Some(rgba) = resolve_color(paint_obj, "circle-color", zoom) {
                    entry.fill_rgba = rgba;
                }
                if let Some(radius) = resolve_number(paint_obj, "circle-radius", zoom) {
                    entry.point_radius = radius as f32;
                }
            }
            _ => {}
        }
    }

    paints
}

/// Multiplies an existing alpha byte by a `fill-opacity`-style factor,
/// clamped to MapLibre's documented `[0, 1]` domain.
fn apply_opacity(alpha: u8, opacity: f64) -> u8 {
    (f64::from(alpha) * opacity.clamp(0.0, 1.0)).round() as u8
}

fn resolve_color(paint_obj: Option<&Value>, prop: &str, zoom: f64) -> Option<[u8; 4]> {
    let raw = paint_obj?.get(prop)?;
    match eval_zoom_expr(raw, zoom)? {
        Resolved::Pick(value) => parse_maplibre_color(value.as_str()?),
        Resolved::Blend { from, to, t } => {
            let from = parse_maplibre_color(from.as_str()?)?;
            let to = parse_maplibre_color(to.as_str()?)?;
            Some(blend_rgba(from, to, t))
        }
    }
}

fn resolve_number(paint_obj: Option<&Value>, prop: &str, zoom: f64) -> Option<f64> {
    let raw = paint_obj?.get(prop)?;
    match eval_zoom_expr(raw, zoom)? {
        Resolved::Pick(value) => value.as_f64(),
        Resolved::Blend { from, to, t } => {
            let from = from.as_f64()?;
            let to = to.as_f64()?;
            Some(from + (to - from) * t)
        }
    }
}

/// What a paint property resolved to at one zoom: either one of the
/// document's own literal outputs verbatim ([`Resolved::Pick`] — a plain
/// value, a `step`'s selected stop, or an `interpolate` evaluated at or
/// outside its own stop range), or a position `t` between two neighboring
/// stop outputs ([`Resolved::Blend`]).
///
/// The blend is returned unapplied, as the two endpoint `Value`s plus `t`,
/// rather than being reduced to a number here, because `interpolate`'s
/// outputs are typed by the property that carries it: `line-width` blends
/// two numbers, `fill-color` blends two color *strings* channel-wise. Only
/// [`resolve_number`]/[`resolve_color`] know which, so combining the
/// endpoints is left to them.
enum Resolved<'a> {
    Pick(&'a Value),
    Blend {
        from: &'a Value,
        to: &'a Value,
        t: f64,
    },
}

/// Evaluates a MapLibre paint-property JSON value **at `zoom`** (`#174`).
///
/// - A plain literal (string or number) resolves to itself, at every zoom.
/// - `["step", ["zoom"], base, in1, out1, in2, out2, ...]` resolves to
///   `base` below `in1`, and otherwise to the output of the last stop whose
///   input is `<= zoom` — MapLibre's own piecewise-constant semantics, so a
///   zoom exactly *on* a breakpoint takes the higher class (`>=`), never the
///   one below it.
/// - `["interpolate", interpolation, ["zoom"], in1, out1, ...]` resolves to
///   `out1` at or below `in1` and to the last output at or above the last
///   input (MapLibre clamps outside the stop range rather than
///   extrapolating), and in between to a [`Resolved::Blend`] of the two
///   bracketing outputs. `["linear"]` and `["exponential", base]` are
///   evaluated; `["cubic-bezier", ...]` is **refused** (`None`) rather than
///   approximated with a straight line — a silently wrong easing curve is
///   exactly the kind of "renders something plausible that nobody asked
///   for" this workspace refuses.
///
/// Everything else yields `None`, leaving the corresponding `LayerPaint`
/// field at whatever it already was ([`UNSTYLED`] or a prior style layer's
/// contribution) rather than guessing. That deliberately includes a `step`
/// or `interpolate` whose input is anything other than `["zoom"]` (e.g.
/// `["get", "height"]`): a `LayerPaint` is resolved once per MVT
/// *source-layer*, never per feature, so there is no feature in hand whose
/// property such an expression could be evaluated against — refusing to
/// paint is honest, picking the first stop and calling it the feature's
/// color is not. `match`/`case` and malformed/odd-length stop lists are
/// refused for the same reason.
fn eval_zoom_expr(value: &Value, zoom: f64) -> Option<Resolved<'_>> {
    let items = match value {
        Value::String(_) | Value::Number(_) => return Some(Resolved::Pick(value)),
        Value::Array(items) => items.as_slice(),
        _ => return None,
    };

    match items.first()?.as_str()? {
        "step" => {
            // ["step", input, base, in1, out1, in2, out2, ...]
            require_zoom_input(items.get(1)?)?;
            let base = items.get(2)?;
            let stops = parse_stops(items.get(3..)?)?;
            let mut chosen = base;
            for (input, output) in &stops {
                if zoom >= *input {
                    chosen = *output;
                }
            }
            Some(Resolved::Pick(chosen))
        }
        "interpolate" => {
            // ["interpolate", interpolation, input, in1, out1, in2, out2, ...]
            let exponent_base = interpolation_base(items.get(1)?)?;
            require_zoom_input(items.get(2)?)?;
            let stops = parse_stops(items.get(3..)?)?;
            let (first_in, first_out) = *stops.first()?;
            if zoom <= first_in {
                return Some(Resolved::Pick(first_out));
            }
            let (last_in, last_out) = *stops.last()?;
            if zoom >= last_in {
                return Some(Resolved::Pick(last_out));
            }
            let window = stops
                .windows(2)
                .find(|pair| zoom >= pair[0].0 && zoom <= pair[1].0)?;
            let (z0, from) = window[0];
            let (z1, to) = window[1];
            Some(Resolved::Blend {
                from,
                to,
                t: interpolation_factor(exponent_base, zoom, z0, z1),
            })
        }
        _ => None,
    }
}

/// `Some(())` only for the literal `["zoom"]` input expression — see
/// [`eval_zoom_expr`]'s doc for why a feature-driven input is refused here
/// rather than approximated.
fn require_zoom_input(input: &Value) -> Option<()> {
    let items = input.as_array()?;
    (items.len() == 1 && items[0].as_str() == Some("zoom")).then_some(())
}

/// The exponential base of a MapLibre `interpolate` interpolation type:
/// `["linear"]` is `["exponential", 1]` by definition, and any other type
/// (`cubic-bezier`) is refused. A base `<= 0` is refused too — it makes
/// [`interpolation_factor`]'s own `powf` meaningless rather than merely
/// unusual.
fn interpolation_base(interpolation: &Value) -> Option<f64> {
    let items = interpolation.as_array()?;
    match items.first()?.as_str()? {
        "linear" => Some(1.0),
        "exponential" => {
            let base = items.get(1)?.as_f64()?;
            (base > 0.0).then_some(base)
        }
        _ => None,
    }
}

/// MapLibre's own interpolation factor between two stop inputs: linear when
/// `base == 1`, and otherwise the spec's
/// `(base^(z - z0) - 1) / (base^(z1 - z0) - 1)` easing.
fn interpolation_factor(base: f64, zoom: f64, z0: f64, z1: f64) -> f64 {
    let span = z1 - z0;
    if span <= 0.0 {
        return 0.0;
    }
    let progress = zoom - z0;
    if (base - 1.0).abs() < f64::EPSILON {
        return progress / span;
    }
    (base.powf(progress) - 1.0) / (base.powf(span) - 1.0)
}

/// Parses a flat `[in1, out1, in2, out2, ...]` stop list into `(input,
/// output)` pairs. Refuses (`None`) an empty list, an odd length, a
/// non-numeric input, or inputs that are not strictly ascending — all four
/// are malformed under the Style Spec, and a stop list this code silently
/// reordered or truncated would paint something the document never
/// described.
fn parse_stops(rest: &[Value]) -> Option<Vec<(f64, &Value)>> {
    if rest.is_empty() || !rest.len().is_multiple_of(2) {
        return None;
    }
    let mut stops: Vec<(f64, &Value)> = Vec::with_capacity(rest.len() / 2);
    for pair in rest.chunks_exact(2) {
        let input = pair[0].as_f64()?;
        if let Some((previous, _)) = stops.last() {
            if input <= *previous {
                return None;
            }
        }
        stops.push((input, &pair[1]));
    }
    Some(stops)
}

/// Channel-wise blend of two straight-RGBA colors at `t`, rounding rather
/// than truncating — the same "round, don't truncate" rule the rest of this
/// workspace's color math already follows, and the reason an interpolated
/// `fill-color` is byte-stable across machines: every channel is computed in
/// `f64` and rounded once, with no platform-dependent intermediate.
fn blend_rgba(from: [u8; 4], to: [u8; 4], t: f64) -> [u8; 4] {
    let mut out = [0u8; 4];
    for i in 0..4 {
        let a = f64::from(from[i]);
        let b = f64::from(to[i]);
        out[i] = (a + (b - a) * t).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Parses a MapLibre paint color string: CSS-hex (`"#rrggbb"`/`"#rrggbbaa"`,
/// delegated to [`parse_css_hex_color`]) or a CSS `rgba(r, g, b, a)`
/// function (`r`/`g`/`b` 0-255 integers, `a` 0.0-1.0). Named CSS colors
/// (`"red"`) and alpha-less `rgb()` are out of scope for v1 (undocumented
/// by the task's property list) and yield `None`.
fn parse_maplibre_color(s: &str) -> Option<[u8; 4]> {
    if s.starts_with('#') {
        return parse_css_hex_color(s).ok();
    }
    let inner = s.strip_prefix("rgba(")?.strip_suffix(')')?;
    let mut parts = inner.split(',').map(str::trim);
    let r: u8 = parts.next()?.parse().ok()?;
    let g: u8 = parts.next()?.parse().ok()?;
    let b: u8 = parts.next()?.parse().ok()?;
    let a: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some([r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A zoom for cases whose style document has no zoom expression in it
    /// at all, so the value cannot matter — spelled as a named constant so
    /// a reader can tell those cases apart at a glance from the ones below
    /// that pass a zoom *because* the answer depends on it.
    const ANY_ZOOM: f64 = 10.0;

    #[test]
    fn resolves_fill_color_and_opacity() {
        let doc = json!({
            "layers": [{
                "id": "buildings-fill",
                "type": "fill",
                "source-layer": "buildings",
                "paint": { "fill-color": "#ff000080", "fill-opacity": 0.5 },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        let paint = paints.get("buildings").unwrap();
        // base alpha 0x80 (128) * 0.5 opacity -> 64.
        assert_eq!(paint.fill_rgba, [0xff, 0, 0, 64]);
    }

    #[test]
    fn resolves_rgba_function_color() {
        let doc = json!({
            "layers": [{
                "type": "fill",
                "source-layer": "buildings",
                "paint": { "fill-color": "rgba(51, 136, 255, 0.4)" },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        let paint = paints.get("buildings").unwrap();
        assert_eq!(paint.fill_rgba, [0x33, 0x88, 0xff, 102]); // round(0.4*255)=102
    }

    #[test]
    fn resolves_line_color_and_width() {
        let doc = json!({
            "layers": [{
                "type": "line",
                "source-layer": "roads",
                "paint": { "line-color": "#ff0000", "line-width": 2.5 },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        let paint = paints.get("roads").unwrap();
        assert_eq!(paint.stroke_rgba, [0xff, 0, 0, 0xff]);
        assert_eq!(paint.stroke_width, 2.5);
    }

    #[test]
    fn resolves_circle_color_and_radius() {
        let doc = json!({
            "layers": [{
                "type": "circle",
                "source-layer": "poi",
                "paint": { "circle-color": "#00ff00", "circle-radius": 8 },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        let paint = paints.get("poi").unwrap();
        assert_eq!(paint.fill_rgba, [0, 0xff, 0, 0xff]);
        assert_eq!(paint.point_radius, 8.0);
    }

    #[test]
    fn merges_multiple_style_layers_on_the_same_source_layer() {
        let doc = json!({
            "layers": [
                { "type": "fill", "source-layer": "parcels", "paint": { "fill-color": "#111111" } },
                { "type": "line", "source-layer": "parcels", "paint": { "line-color": "#222222", "line-width": 3 } },
            ],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        let paint = paints.get("parcels").unwrap();
        assert_eq!(paint.fill_rgba, [0x11, 0x11, 0x11, 0xff]);
        assert_eq!(paint.stroke_rgba, [0x22, 0x22, 0x22, 0xff]);
        assert_eq!(paint.stroke_width, 3.0);
    }

    fn roads_line_width(paint: &Value, zoom: f64) -> f32 {
        let doc = json!({
            "layers": [{ "type": "line", "source-layer": "roads", "paint": { "line-width": paint } }],
        });
        resolve_layer_paints(&doc, zoom)
            .get("roads")
            .unwrap()
            .stroke_width
    }

    /// `#174`: the whole point of taking a zoom. Before this, every zoom
    /// resolved to the first stop's `1.0`; now only a zoom at or below the
    /// first stop does, and a mid-range zoom actually interpolates.
    #[test]
    fn linear_interpolate_evaluates_across_the_whole_zoom_range() {
        let expr = json!(["interpolate", ["linear"], ["zoom"], 5, 1.0, 15, 4.0]);
        // Below and at the first stop: clamped to the first output, which is
        // also the only answer the pre-`#174` first-stop reading ever gave —
        // so the low-zoom end is unchanged, deliberately.
        assert_eq!(roads_line_width(&expr, 0.0), 1.0);
        assert_eq!(roads_line_width(&expr, 5.0), 1.0);
        // Halfway (z10 of 5..15) is halfway between 1.0 and 4.0.
        assert_eq!(roads_line_width(&expr, 10.0), 2.5);
        // At and above the last stop: clamped, never extrapolated past 4.0.
        assert_eq!(roads_line_width(&expr, 15.0), 4.0);
        assert_eq!(roads_line_width(&expr, 22.0), 4.0);
    }

    /// An `exponential` interpolation is evaluated with the spec's own
    /// easing, not silently straightened into a linear one: base 2 over
    /// 5..15 at the midpoint gives `(2^5 - 1) / (2^10 - 1) = 31/1023`, well
    /// below linear's own 0.5.
    #[test]
    fn exponential_interpolate_uses_the_declared_base() {
        let expr = json!(["interpolate", ["exponential", 2], ["zoom"], 5, 1.0, 15, 4.0]);
        let expected = (1.0 + 3.0 * (31.0 / 1023.0)) as f32;
        assert!((roads_line_width(&expr, 10.0) - expected).abs() < 1e-5);
    }

    /// `cubic-bezier` easing is not implemented, and is refused rather than
    /// approximated with a straight line — the field keeps its prior value.
    #[test]
    fn cubic_bezier_interpolation_is_refused_not_approximated() {
        let expr = json!([
            "interpolate",
            ["cubic-bezier", 0.4, 0.0, 0.6, 1.0],
            ["zoom"],
            5,
            1.0,
            15,
            4.0
        ]);
        assert_eq!(roads_line_width(&expr, 10.0), UNSTYLED.stroke_width);
    }

    /// `step` is piecewise-constant, and a zoom landing exactly on a
    /// breakpoint takes the higher class — the same "a value equal to a
    /// class edge lands in a defined class, consistently" rule this
    /// workspace's ramp classification already follows.
    #[test]
    fn step_expression_selects_the_class_the_zoom_falls_in() {
        let radius_at = |zoom: f64| {
            let doc = json!({
                "layers": [{
                    "type": "circle",
                    "source-layer": "poi",
                    "paint": { "circle-radius": ["step", ["zoom"], 3.0, 10, 6.0, 15, 10.0] },
                }],
            });
            resolve_layer_paints(&doc, zoom)
                .get("poi")
                .unwrap()
                .point_radius
        };
        assert_eq!(radius_at(0.0), 3.0);
        assert_eq!(radius_at(9.999), 3.0);
        assert_eq!(
            radius_at(10.0),
            6.0,
            "a zoom on the breakpoint takes the class above it"
        );
        assert_eq!(radius_at(14.999), 6.0);
        assert_eq!(radius_at(15.0), 10.0);
        assert_eq!(radius_at(22.0), 10.0);
    }

    /// A zoom-interpolated COLOR blends both endpoints channel-wise, rather
    /// than snapping to whichever stop is nearer.
    #[test]
    fn interpolate_blends_colors_channel_wise() {
        let fill_at = |zoom: f64| {
            let doc = json!({
                "layers": [{
                    "type": "fill",
                    "source-layer": "landuse",
                    "paint": {
                        "fill-color": [
                            "interpolate", ["linear"], ["zoom"],
                            4, "#000000", 12, "#ffffff"
                        ],
                    },
                }],
            });
            resolve_layer_paints(&doc, zoom)
                .get("landuse")
                .unwrap()
                .fill_rgba
        };
        assert_eq!(fill_at(4.0), [0, 0, 0, 255]);
        assert_eq!(fill_at(12.0), [255, 255, 255, 255]);
        // z8 is exactly halfway: 127.5 must round to 128, never truncate.
        assert_eq!(fill_at(8.0), [128, 128, 128, 255]);
    }

    /// A `step`/`interpolate` driven by a FEATURE property, not zoom, is
    /// refused outright: paints are resolved per source-layer, so there is
    /// no feature to evaluate `["get", ...]` against.
    #[test]
    fn a_feature_driven_expression_input_is_refused() {
        let expr = json!(["interpolate", ["linear"], ["get", "width"], 5, 1.0, 15, 4.0]);
        assert_eq!(roads_line_width(&expr, 10.0), UNSTYLED.stroke_width);
        let stepped = json!(["step", ["get", "class"], 1.0, 5, 4.0]);
        assert_eq!(roads_line_width(&stepped, 10.0), UNSTYLED.stroke_width);
    }

    /// A malformed stop list (odd length, or inputs that do not ascend) is
    /// refused rather than half-read.
    #[test]
    fn a_malformed_stop_list_is_refused() {
        let odd = json!(["interpolate", ["linear"], ["zoom"], 5, 1.0, 15]);
        assert_eq!(roads_line_width(&odd, 10.0), UNSTYLED.stroke_width);
        let descending = json!(["step", ["zoom"], 1.0, 15, 4.0, 5, 2.0]);
        assert_eq!(roads_line_width(&descending, 10.0), UNSTYLED.stroke_width);
    }

    #[test]
    fn unrecognized_expression_shape_leaves_the_field_at_its_default() {
        let doc = json!({
            "layers": [{
                "type": "line",
                "source-layer": "roads",
                "paint": {
                    "line-color": ["match", ["get", "class"], "motorway", "#ff0000", "#888888"],
                },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        assert_eq!(
            paints.get("roads").unwrap().stroke_rgba,
            UNSTYLED.stroke_rgba
        );
    }

    #[test]
    fn unparseable_color_leaves_the_field_at_its_default() {
        let doc = json!({
            "layers": [{
                "type": "fill",
                "source-layer": "buildings",
                "paint": { "fill-color": "cornflowerblue" },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        assert_eq!(paints.get("buildings").unwrap().fill_rgba, [0, 0, 0, 0]);
    }

    #[test]
    fn layer_without_source_layer_is_skipped() {
        let doc = json!({
            "layers": [{ "type": "fill", "paint": { "fill-color": "#111111" } }],
        });
        assert!(resolve_layer_paints(&doc, ANY_ZOOM).is_empty());
    }

    #[test]
    fn document_without_layers_array_yields_no_paints() {
        assert!(resolve_layer_paints(&json!({}), ANY_ZOOM).is_empty());
    }

    #[test]
    fn unstyled_layer_type_is_still_registered() {
        let doc = json!({
            "layers": [{
                "type": "background",
                "source-layer": "labels",
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        assert_eq!(paints.get("labels").unwrap(), &UNSTYLED);
    }

    /// `#174` asked for fixed-font golden coverage of label clipping and
    /// seam continuity **"once label rendering exists."** It does not: this
    /// workspace has no glyph rasterizer, no font dependency and no text
    /// paint property anywhere (`src/raster.rs` draws fills, strokes and
    /// circles only), so a `symbol` layer resolves to no paint at all and
    /// draws nothing — the same as any other unhandled layer type.
    ///
    /// Pinned as a test rather than left as prose so the refusal is
    /// machine-checked: the day somebody implements label rendering, this
    /// test is what fails and points at the golden coverage that then has
    /// to come with it. Adding a font to this workspace purely to have a
    /// label golden would be the wrong trade in the other direction — a
    /// golden that depends on a system font is not portable, and one
    /// carrying an embedded font is neither small nor this issue's call to
    /// make.
    #[test]
    fn a_symbol_layer_contributes_no_paint_because_labels_are_not_rendered() {
        let doc = json!({
            "layers": [{
                "id": "place-labels",
                "type": "symbol",
                "source-layer": "places",
                "layout": { "text-field": ["get", "name"], "text-size": 12 },
                "paint": { "text-color": "#111111", "text-halo-color": "#ffffff" },
            }],
        });
        let paints = resolve_layer_paints(&doc, ANY_ZOOM);
        assert_eq!(
            paints.get("places").unwrap(),
            &UNSTYLED,
            "a symbol layer must contribute nothing while this crate has no text \
             rendering — never a guessed fill from its text-color"
        );
    }

    // -- style applicability (`#245`) -------------------------------------

    fn layers(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// The invariant that makes this the right predicate for deciding
    /// whether to advertise a style at all: it answers exactly "does
    /// `resolve_layer_paints` produce a key this tileset carries" — the same
    /// keys the renderer will look tiles up by.
    #[test]
    fn source_layers_are_exactly_the_keys_resolve_layer_paints_produces() {
        let doc = json!({
            "layers": [
                { "type": "fill", "source-layer": "parcels" },
                { "type": "line", "source-layer": "parcels" },
                { "type": "background", "id": "bg" },
                { "type": "circle", "source-layer": "poi" },
            ],
        });
        let named: BTreeSet<String> = source_layers(&doc).map(str::to_string).collect();
        let painted: BTreeSet<String> = resolve_layer_paints(&doc, ANY_ZOOM).into_keys().collect();
        assert_eq!(named, painted);
    }

    #[test]
    fn a_style_applies_when_it_names_any_one_of_the_tilesets_layers() {
        let doc = json!({ "layers": [{ "type": "fill", "source-layer": "roads" }] });
        assert!(style_paints_any_layer(
            &doc,
            &layers(&["buildings", "roads"])
        ));
    }

    #[test]
    fn a_style_naming_only_other_collections_layers_does_not_apply() {
        let doc = json!({ "layers": [{ "type": "fill", "source-layer": "somebody-elses" }] });
        assert!(!style_paints_any_layer(&doc, &layers(&["roads"])));
    }

    /// A background-only document paints nothing anywhere — and a tileset
    /// reporting no layer names has nothing any style could paint.
    #[test]
    fn a_style_with_no_source_layer_and_a_tileset_with_no_layers_both_apply_to_nothing() {
        let background = json!({ "layers": [{ "id": "bg", "type": "background" }] });
        assert!(!style_paints_any_layer(&background, &layers(&["roads"])));

        let real = json!({ "layers": [{ "type": "fill", "source-layer": "roads" }] });
        assert!(!style_paints_any_layer(&real, &BTreeSet::new()));
    }
}
