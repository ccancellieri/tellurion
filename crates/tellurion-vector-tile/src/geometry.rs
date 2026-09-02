use geo::{BooleanOps, BoundingRect, Intersects, MapCoordsInPlace};
use geo_types::{Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use geozero::mvt::MvtWriter;
use geozero::{GeomProcessor, GeozeroGeometry};
use tellurion_core::TileCoord;

const WEB_MERCATOR_ORIGIN: f64 = 20_037_508.342_789_244;
const WEB_MERCATOR_RADIUS: f64 = 6_378_137.0;
const WEB_MERCATOR_MAX_LAT: f64 = 85.051_128_78;

pub(crate) fn tile_envelope_3857_unchecked(coord: TileCoord) -> [f64; 4] {
    let matrix_side = 1u64 << coord.z;
    let tile_size = (2.0 * WEB_MERCATOR_ORIGIN) / matrix_side as f64;
    let minx = -WEB_MERCATOR_ORIGIN + f64::from(coord.x) * tile_size;
    let maxx = minx + tile_size;
    let maxy = WEB_MERCATOR_ORIGIN - f64::from(coord.y) * tile_size;
    let miny = maxy - tile_size;
    [minx, miny, maxx, maxy]
}

pub(crate) fn project_to_web_mercator(geometry: &mut Geometry<f64>) {
    geometry.map_coords_in_place(|coord| {
        let latitude = coord.y.clamp(-WEB_MERCATOR_MAX_LAT, WEB_MERCATOR_MAX_LAT);
        geo_types::Coord {
            x: coord.x.to_radians() * WEB_MERCATOR_RADIUS,
            y: (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0)
                .tan()
                .ln()
                * WEB_MERCATOR_RADIUS,
        }
    });
}

pub(crate) fn clip_to_tile(geometry: Geometry<f64>, envelope: [f64; 4]) -> Option<Geometry<f64>> {
    if geometry.bounding_rect().is_some_and(|bounds| {
        bounds.min().x >= envelope[0]
            && bounds.min().y >= envelope[1]
            && bounds.max().x <= envelope[2]
            && bounds.max().y <= envelope[3]
    }) {
        return Some(geometry);
    }
    let clip = envelope_polygon(envelope);
    match geometry {
        Geometry::Point(point) => clip.intersects(&point).then_some(Geometry::Point(point)),
        Geometry::MultiPoint(points) => {
            let points = MultiPoint(
                points
                    .0
                    .into_iter()
                    .filter(|point| clip.intersects(point))
                    .collect(),
            );
            (!points.0.is_empty()).then_some(Geometry::MultiPoint(points))
        }
        Geometry::Line(line) => {
            let clipped = clip.clip(
                &MultiLineString(vec![LineString::from(vec![line.start, line.end])]),
                false,
            );
            collapse_lines(clipped)
        }
        Geometry::LineString(line) => {
            collapse_lines(clip.clip(&MultiLineString(vec![line]), false))
        }
        Geometry::MultiLineString(lines) => collapse_lines(clip.clip(&lines, false)),
        Geometry::Polygon(polygon) => collapse_polygons(polygon.intersection(&clip)),
        Geometry::MultiPolygon(polygons) => collapse_polygons(polygons.intersection(&clip)),
        Geometry::Rect(rect) => collapse_polygons(rect.to_polygon().intersection(&clip)),
        Geometry::Triangle(triangle) => {
            collapse_polygons(triangle.to_polygon().intersection(&clip))
        }
        Geometry::GeometryCollection(_) => {
            unreachable!("geometry collections are normalized before clipping")
        }
    }
}

fn envelope_polygon([minx, miny, maxx, maxy]: [f64; 4]) -> Polygon<f64> {
    Polygon::new(
        LineString::from(vec![
            (minx, miny),
            (maxx, miny),
            (maxx, maxy),
            (minx, maxy),
            (minx, miny),
        ]),
        Vec::new(),
    )
}

fn collapse_lines(mut lines: MultiLineString<f64>) -> Option<Geometry<f64>> {
    match lines.0.len() {
        0 => None,
        1 => Some(Geometry::LineString(lines.0.pop().unwrap())),
        _ => Some(Geometry::MultiLineString(lines)),
    }
}

fn collapse_polygons(mut polygons: MultiPolygon<f64>) -> Option<Geometry<f64>> {
    match polygons.0.len() {
        0 => None,
        1 => Some(Geometry::Polygon(polygons.0.pop().unwrap())),
        _ => Some(Geometry::MultiPolygon(polygons)),
    }
}

#[derive(Default)]
struct CoordinateInspector {
    vertices: u64,
    finite: bool,
}

impl GeomProcessor for CoordinateInspector {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> geozero::error::Result<()> {
        self.vertices += 1;
        self.finite &= x.is_finite() && y.is_finite();
        Ok(())
    }
}

pub(crate) fn inspect_coordinates(geometry: &Geometry<f64>) -> Result<(u64, bool), String> {
    let mut inspector = CoordinateInspector {
        vertices: 0,
        finite: true,
    };
    geometry
        .process_geom(&mut inspector)
        .map_err(|error| error.to_string())?;
    Ok((inspector.vertices, inspector.finite))
}

enum GeometryParts {
    Empty,
    Points(Vec<Point<f64>>),
    Lines(Vec<LineString<f64>>),
    Polygons(Vec<Polygon<f64>>),
}

impl GeometryParts {
    fn add(&mut self, geometry: Geometry<f64>) -> Result<(), ()> {
        match geometry {
            Geometry::Point(point) => self.add_points(vec![point]),
            Geometry::MultiPoint(points) => self.add_points(points.0),
            Geometry::Line(line) => {
                self.add_lines(vec![LineString::from(vec![line.start, line.end])])
            }
            Geometry::LineString(line) => self.add_lines(vec![line]),
            Geometry::MultiLineString(lines) => self.add_lines(lines.0),
            Geometry::Polygon(polygon) => self.add_polygons(vec![polygon]),
            Geometry::MultiPolygon(polygons) => self.add_polygons(polygons.0),
            Geometry::Rect(rect) => self.add_polygons(vec![rect.to_polygon()]),
            Geometry::Triangle(triangle) => self.add_polygons(vec![triangle.to_polygon()]),
            Geometry::GeometryCollection(collection) => {
                for geometry in collection.0 {
                    self.add(geometry)?;
                }
                Ok(())
            }
        }
    }

    fn add_points(&mut self, mut values: Vec<Point<f64>>) -> Result<(), ()> {
        match self {
            Self::Empty => *self = Self::Points(values),
            Self::Points(points) => points.append(&mut values),
            Self::Lines(_) | Self::Polygons(_) => return Err(()),
        }
        Ok(())
    }

    fn add_lines(&mut self, mut values: Vec<LineString<f64>>) -> Result<(), ()> {
        match self {
            Self::Empty => *self = Self::Lines(values),
            Self::Lines(lines) => lines.append(&mut values),
            Self::Points(_) | Self::Polygons(_) => return Err(()),
        }
        Ok(())
    }

    fn add_polygons(&mut self, mut values: Vec<Polygon<f64>>) -> Result<(), ()> {
        match self {
            Self::Empty => *self = Self::Polygons(values),
            Self::Polygons(polygons) => polygons.append(&mut values),
            Self::Points(_) | Self::Lines(_) => return Err(()),
        }
        Ok(())
    }

    fn finish(self) -> Result<Geometry<f64>, ()> {
        match self {
            Self::Empty => Err(()),
            Self::Points(points) => Ok(Geometry::MultiPoint(MultiPoint(points))),
            Self::Lines(lines) => Ok(Geometry::MultiLineString(MultiLineString(lines))),
            Self::Polygons(polygons) => Ok(Geometry::MultiPolygon(MultiPolygon(polygons))),
        }
    }
}

pub(crate) fn normalize_geometry_collection(geometry: Geometry<f64>) -> Result<Geometry<f64>, ()> {
    let Geometry::GeometryCollection(collection) = geometry else {
        return Ok(geometry);
    };
    let mut parts = GeometryParts::Empty;
    for geometry in collection.0 {
        parts.add(geometry)?;
    }
    parts.finish()
}

fn signed_area(points: &[(f64, f64)]) -> f64 {
    points
        .windows(2)
        .map(|pair| pair[0].0 * pair[1].1 - pair[1].0 * pair[0].1)
        .sum()
}

struct RingBuffer {
    index: usize,
    points: Vec<(f64, f64)>,
}

pub(crate) struct WindingNormalizer<'a> {
    inner: &'a mut MvtWriter,
    in_polygon: bool,
    ring: Option<RingBuffer>,
}

impl<'a> WindingNormalizer<'a> {
    pub(crate) fn new(inner: &'a mut MvtWriter) -> Self {
        Self {
            inner,
            in_polygon: false,
            ring: None,
        }
    }
}

impl GeomProcessor for WindingNormalizer<'_> {
    fn xy(&mut self, x: f64, y: f64, idx: usize) -> geozero::error::Result<()> {
        if let Some(ring) = &mut self.ring {
            ring.points.push((x, y));
            Ok(())
        } else {
            self.inner.xy(x, y, idx)
        }
    }

    fn point_begin(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.point_begin(idx)
    }
    fn point_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.point_end(idx)
    }
    fn multipoint_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipoint_begin(size, idx)
    }
    fn multipoint_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipoint_end(idx)
    }
    fn multilinestring_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.multilinestring_begin(size, idx)
    }
    fn multilinestring_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.multilinestring_end(idx)
    }

    fn linestring_begin(
        &mut self,
        tagged: bool,
        size: usize,
        idx: usize,
    ) -> geozero::error::Result<()> {
        if !tagged && self.in_polygon {
            self.ring = Some(RingBuffer {
                index: idx,
                points: Vec::with_capacity(size),
            });
            Ok(())
        } else {
            self.inner.linestring_begin(tagged, size, idx)
        }
    }

    fn linestring_end(&mut self, tagged: bool, idx: usize) -> geozero::error::Result<()> {
        let Some(ring) = self.ring.take() else {
            return self.inner.linestring_end(tagged, idx);
        };
        let mut points = ring.points;
        let area = signed_area(&points);
        if (ring.index == 0 && area > 0.0) || (ring.index != 0 && area < 0.0) {
            points.reverse();
        }
        self.inner
            .linestring_begin(false, points.len(), ring.index)?;
        for (index, (x, y)) in points.into_iter().enumerate() {
            self.inner.xy(x, y, index)?;
        }
        self.inner.linestring_end(false, ring.index)
    }

    fn polygon_begin(
        &mut self,
        tagged: bool,
        size: usize,
        idx: usize,
    ) -> geozero::error::Result<()> {
        self.in_polygon = true;
        self.inner.polygon_begin(tagged, size, idx)
    }
    fn polygon_end(&mut self, tagged: bool, idx: usize) -> geozero::error::Result<()> {
        self.in_polygon = false;
        self.inner.polygon_end(tagged, idx)
    }
    fn multipolygon_begin(&mut self, size: usize, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipolygon_begin(size, idx)
    }
    fn multipolygon_end(&mut self, idx: usize) -> geozero::error::Result<()> {
        self.inner.multipolygon_end(idx)
    }
}
