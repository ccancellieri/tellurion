//! Config-driven single-band colormap application (`#92`): maps a raw 8-bit
//! grayscale sample to straight RGBA. Consumes `tellurion_core::config::
//! ColormapConf` — an operator's own declared shape, already eagerly
//! validated by `AppConfig::validate` (a `Stops` list is always non-empty
//! and strictly ascending by the time it reaches here) — and turns it into
//! ready-to-apply RGBA bytes. The built-in ramp color tables live here
//! rather than in `tellurion-core` because they're rendering data, not
//! config shape: the same split `tellurion-render::style::RenderStyle`
//! draws against its own `config::StyleConf`.
//!
//! [`ResolvedColormap::build`] precomputes every one of the 256 possible
//! byte values into a lookup table once per tile request, rather than
//! interpolating per pixel inside `reader::read_window`'s own per-pixel
//! loop — that loop is the read path's hot spot; a fixed-size array lookup
//! costs nothing extra there, while repeating the ramp/stop interpolation
//! per pixel would.

use tellurion_core::config::{ColorRamp, ColormapConf, ColormapStop};

/// A colormap resolved into a 256-entry byte -> RGBA lookup table.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedColormap {
    lut: [[u8; 4]; 256],
}

impl ResolvedColormap {
    pub fn build(conf: &ColormapConf) -> Self {
        let mut lut = [[0u8; 4]; 256];
        for (value, slot) in lut.iter_mut().enumerate() {
            *slot = sample(conf, value as u8);
        }
        Self { lut }
    }

    /// Resolves a GeoTIFF `ColorMap` tag's own raw values into a lookup
    /// table — this crate's authoring path (`author.rs`) writes, and its
    /// reader (`reader.rs`) reads, exactly this 8-bit-palette shape: `raw`
    /// MUST be 768 entries (3 planes of 256, all Red values then all Green
    /// then all Blue — TIFF6 Section 8's own layout, never interleaved per
    /// index); the caller (`reader::read_and_validate_colormap`) already
    /// guarantees that length before this ever runs. Each 16-bit channel
    /// value spans the tag's own full `0..=65535` range regardless of the
    /// image's real bit depth (TIFF6's own convention), so it's downscaled
    /// to 8-bit the same way GDAL does: the high byte. An embedded palette
    /// carries no alpha channel, so every resolved color is fully opaque —
    /// unlike an operator-configured [`ColormapConf::Stops`] entry, there is
    /// no per-index transparency lever here.
    pub fn from_tiff_colormap(raw: &[u16]) -> Self {
        debug_assert_eq!(
            raw.len(),
            768,
            "a caller must validate an 8-bit TIFF ColorMap's length before calling this"
        );
        let mut lut = [[0u8; 4]; 256];
        for (index, slot) in lut.iter_mut().enumerate() {
            let r = raw.get(index).copied().unwrap_or(0);
            let g = raw.get(256 + index).copied().unwrap_or(0);
            let b = raw.get(512 + index).copied().unwrap_or(0);
            *slot = [(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8, 255];
        }
        Self { lut }
    }

    pub fn apply(&self, sample: u8) -> [u8; 4] {
        self.lut[sample as usize]
    }
}

fn sample(conf: &ColormapConf, value: u8) -> [u8; 4] {
    match conf {
        ColormapConf::Ramp { ramp, min, max } => sample_ramp(*ramp, *min, *max, value),
        ColormapConf::Stops { stops } => sample_stops(stops, f64::from(value)),
    }
}

fn sample_ramp(ramp: ColorRamp, min: f64, max: f64, value: u8) -> [u8; 4] {
    let t = if max > min {
        ((f64::from(value) - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    lerp_control_points(control_points(ramp), t)
}

/// Control points as `(fraction in [0, 1], rgba)`, ascending — a built-in
/// ramp's own color science ("a couple of built-in ramps," this slice's own
/// scope). An operator who needs anything else declares an explicit
/// `ColormapConf::Stops` list instead.
fn control_points(ramp: ColorRamp) -> &'static [(f64, [u8; 4])] {
    match ramp {
        ColorRamp::Grayscale => &[(0.0, [0, 0, 0, 255]), (1.0, [255, 255, 255, 255])],
        // A coarse (5-stop) approximation of matplotlib's Viridis — close
        // enough for a tile thumbnail, not a scientific-precision
        // reproduction.
        ColorRamp::Viridis => &[
            (0.0, [68, 1, 84, 255]),
            (0.25, [59, 82, 139, 255]),
            (0.5, [33, 145, 140, 255]),
            (0.75, [94, 201, 98, 255]),
            (1.0, [253, 231, 37, 255]),
        ],
    }
}

fn lerp_control_points(points: &[(f64, [u8; 4])], t: f64) -> [u8; 4] {
    let first = points
        .first()
        .expect("a built-in ramp always declares at least one control point");
    if t <= first.0 {
        return first.1;
    }
    let last = points
        .last()
        .expect("a built-in ramp always declares at least one control point");
    if t >= last.0 {
        return last.1;
    }
    for pair in points.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t >= t0 && t <= t1 {
            let local = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            return lerp_rgba(c0, c1, local);
        }
    }
    last.1
}

/// `stops` MUST be non-empty and strictly ascending by `value` —
/// `AppConfig::validate` (`tellurion_core::config::ColormapConf::validate`)
/// already guarantees both eagerly, at config load, before any collection
/// reaches a driver.
fn sample_stops(stops: &[ColormapStop], value: f64) -> [u8; 4] {
    let first = stops
        .first()
        .expect("ColormapConf::validate guarantees a non-empty stop list");
    if value <= first.value {
        return first.rgba;
    }
    let last = stops
        .last()
        .expect("ColormapConf::validate guarantees a non-empty stop list");
    if value >= last.value {
        return last.rgba;
    }
    for pair in stops.windows(2) {
        if value >= pair[0].value && value <= pair[1].value {
            let t = (value - pair[0].value) / (pair[1].value - pair[0].value);
            return lerp_rgba(pair[0].rgba, pair[1].rgba, t);
        }
    }
    last.rgba
}

fn lerp_rgba(a: [u8; 4], b: [u8; 4], t: f64) -> [u8; 4] {
    let mut out = [0u8; 4];
    for i in 0..4 {
        out[i] = (f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t).round() as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(value: f64, rgba: [u8; 4]) -> ColormapStop {
        ColormapStop { value, rgba }
    }

    #[test]
    fn ramp_endpoints_return_the_exact_control_point_colors() {
        let conf = ColormapConf::Ramp {
            ramp: ColorRamp::Grayscale,
            min: 0.0,
            max: 255.0,
        };
        let resolved = ResolvedColormap::build(&conf);
        assert_eq!(resolved.apply(0), [0, 0, 0, 255]);
        assert_eq!(resolved.apply(255), [255, 255, 255, 255]);
    }

    #[test]
    fn grayscale_ramp_midpoint_is_mid_gray() {
        let conf = ColormapConf::Ramp {
            ramp: ColorRamp::Grayscale,
            min: 0.0,
            max: 255.0,
        };
        let resolved = ResolvedColormap::build(&conf);
        // t = 128/255 -- chosen so `0 + 255 * t` cancels back to exactly
        // 128, no float-rounding ambiguity.
        assert_eq!(resolved.apply(128), [128, 128, 128, 255]);
    }

    #[test]
    fn ramp_clamps_values_outside_the_domain() {
        let conf = ColormapConf::Ramp {
            ramp: ColorRamp::Grayscale,
            min: 10.0,
            max: 20.0,
        };
        let resolved = ResolvedColormap::build(&conf);
        assert_eq!(
            resolved.apply(0),
            [0, 0, 0, 255],
            "below min clamps to the ramp's low end"
        );
        assert_eq!(
            resolved.apply(255),
            [255, 255, 255, 255],
            "above max clamps to the ramp's high end"
        );
    }

    #[test]
    fn stops_exact_values_return_their_own_color_unblended() {
        let conf = ColormapConf::Stops {
            stops: vec![
                stop(0.0, [255, 0, 0, 255]),
                stop(100.0, [0, 255, 0, 255]),
                stop(200.0, [0, 0, 255, 255]),
            ],
        };
        let resolved = ResolvedColormap::build(&conf);
        assert_eq!(resolved.apply(0), [255, 0, 0, 255]);
        assert_eq!(resolved.apply(100), [0, 255, 0, 255]);
        assert_eq!(resolved.apply(200), [0, 0, 255, 255]);
    }

    #[test]
    fn stops_interpolate_linearly_between_two_stops() {
        let conf = ColormapConf::Stops {
            stops: vec![stop(0.0, [0, 0, 0, 255]), stop(100.0, [100, 200, 0, 255])],
        };
        let resolved = ResolvedColormap::build(&conf);
        // value 50 is exactly halfway between the two stops.
        assert_eq!(resolved.apply(50), [50, 100, 0, 255]);
    }

    #[test]
    fn stops_clamp_outside_the_declared_range() {
        let conf = ColormapConf::Stops {
            stops: vec![
                stop(50.0, [10, 20, 30, 255]),
                stop(200.0, [40, 50, 60, 255]),
            ],
        };
        let resolved = ResolvedColormap::build(&conf);
        assert_eq!(resolved.apply(0), [10, 20, 30, 255]);
        assert_eq!(resolved.apply(255), [40, 50, 60, 255]);
    }

    // -- `from_tiff_colormap` (embedded palette, categorical authoring) -----

    #[test]
    fn from_tiff_colormap_downscales_full_range_16_bit_channels_to_8_bit() {
        let mut raw = vec![0u16; 768];
        raw[0] = 65535; // R plane, index 0
        raw[512 + 1] = 65535; // B plane, index 1
        let resolved = ResolvedColormap::from_tiff_colormap(&raw);
        assert_eq!(resolved.apply(0), [255, 0, 0, 255], "index 0 is pure red");
        assert_eq!(resolved.apply(1), [0, 0, 255, 255], "index 1 is pure blue");
        assert_eq!(
            resolved.apply(2),
            [0, 0, 0, 255],
            "every other index defaults to opaque black, never transparent"
        );
    }

    #[test]
    fn from_tiff_colormap_reads_planes_in_r_then_g_then_b_order_never_interleaved() {
        let mut raw = vec![0u16; 768];
        raw[10] = 0x1234; // R plane, index 10
        raw[256 + 10] = 0x5678; // G plane, index 10
        raw[512 + 10] = 0x9abc; // B plane, index 10
        let resolved = ResolvedColormap::from_tiff_colormap(&raw);
        assert_eq!(resolved.apply(10), [0x12, 0x56, 0x9a, 255]);
    }

    /// A stop's own alpha channel is this config model's only lever for a
    /// nodata-style sentinel value — no separate nodata concept exists.
    #[test]
    fn a_stop_with_zero_alpha_renders_transparent() {
        let conf = ColormapConf::Stops {
            stops: vec![stop(0.0, [0, 0, 0, 0]), stop(255.0, [255, 255, 255, 255])],
        };
        let resolved = ResolvedColormap::build(&conf);
        assert_eq!(
            resolved.apply(0)[3],
            0,
            "a nodata-style stop stays transparent, not opaque black"
        );
    }
}
