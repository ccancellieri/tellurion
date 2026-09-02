//! Config-driven single-band colormap application: maps one decoded Zarr
//! sample (a plain `f64`, already widened from whatever dtype the array
//! declares — see `metadata::DType::decode`) to straight RGBA. Consumes
//! `tellurion_core::config::ColormapConf`, the same declared shape
//! `tellurion-cog::colormap` resolves for an 8-bit GeoTIFF band, but applied
//! directly in continuous value space rather than through a 256-entry
//! byte-indexed lookup table: a Zarr sample's domain is whatever its dtype
//! and real-world units are (temperature in Celsius, an index in `[0,1]`,
//! ...), not a fixed 0-255 image byte, so there is no fixed-size domain to
//! precompute a table over. `ColormapConf::Ramp`'s own `min`/`max` and
//! `ColormapConf::Stops`' own `value`s already operate on real (`f64`)
//! numbers, so this reuses that shape as-is.
//!
//! A colormap is mandatory for a Zarr collection to serve PNG tiles at all in
//! this slice (`crate::driver` refuses a raster_tile request outright when
//! none is configured) — unlike an 8-bit COG image, a Zarr array's raw sample
//! has no inherent visual meaning, so this driver never guesses a default
//! numeric-to-color scaling.

use tellurion_core::config::{ColorRamp, ColormapConf, ColormapStop};

pub fn apply(conf: &ColormapConf, value: f64) -> [u8; 4] {
    match conf {
        ColormapConf::Ramp { ramp, min, max } => sample_ramp(*ramp, *min, *max, value),
        ColormapConf::Stops { stops } => sample_stops(stops, value),
    }
}

fn sample_ramp(ramp: ColorRamp, min: f64, max: f64, value: f64) -> [u8; 4] {
    let t = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    lerp_control_points(control_points(ramp), t)
}

/// Control points as `(fraction in [0, 1], rgba)`, ascending — the same two
/// built-in ramps `tellurion-cog::colormap` offers, kept in sync by hand
/// (this slice's own scope is "a couple of built-in ramps," not a shared
/// palette library between driver crates).
fn control_points(ramp: ColorRamp) -> &'static [(f64, [u8; 4])] {
    match ramp {
        ColorRamp::Grayscale => &[(0.0, [0, 0, 0, 255]), (1.0, [255, 255, 255, 255])],
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
            min: -40.0,
            max: 40.0,
        };
        assert_eq!(apply(&conf, -40.0), [0, 0, 0, 255]);
        assert_eq!(apply(&conf, 40.0), [255, 255, 255, 255]);
    }

    #[test]
    fn ramp_midpoint_is_mid_gray_over_a_non_byte_domain() {
        // Values outside [0,255] are exactly the point of operating in
        // continuous space rather than a byte-quantized lookup table.
        let conf = ColormapConf::Ramp {
            ramp: ColorRamp::Grayscale,
            min: -100.0,
            max: 100.0,
        };
        assert_eq!(apply(&conf, 0.0), [128, 128, 128, 255]);
    }

    #[test]
    fn ramp_clamps_values_outside_the_domain() {
        let conf = ColormapConf::Ramp {
            ramp: ColorRamp::Grayscale,
            min: 10.0,
            max: 20.0,
        };
        assert_eq!(apply(&conf, 0.0), [0, 0, 0, 255]);
        assert_eq!(apply(&conf, 1000.0), [255, 255, 255, 255]);
    }

    #[test]
    fn stops_interpolate_linearly_between_two_stops() {
        let conf = ColormapConf::Stops {
            stops: vec![stop(0.0, [0, 0, 0, 255]), stop(100.0, [100, 200, 0, 255])],
        };
        assert_eq!(apply(&conf, 50.0), [50, 100, 0, 255]);
    }

    #[test]
    fn a_stop_with_zero_alpha_renders_transparent() {
        let conf = ColormapConf::Stops {
            stops: vec![stop(0.0, [0, 0, 0, 0]), stop(255.0, [255, 255, 255, 255])],
        };
        assert_eq!(apply(&conf, 0.0)[3], 0);
    }
}
