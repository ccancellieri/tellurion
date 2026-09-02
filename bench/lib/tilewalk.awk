# Deterministic pseudo-random tile-coordinate generator over WebMercatorQuad
# tile indices within a bbox, at a single zoom level. Reproducible: same seed
# + inputs -> same output.
#
# Usage:
#   awk -f tilewalk.awk -v seed=N -v z=Z -v west=W -v south=S -v east=E -v north=N \
#       -v count=C -v maxstep=M -v mode=walk|uniform
#
#   seed    integer, any value (folded into the Park-Miller range)
#   z       zoom level (0..24)
#   west/south/east/north  bbox in CRS84 degrees (lon/lat)
#   count   number of tile coordinates to emit
#   maxstep max per-axis tile step between consecutive walk points, "walk"
#           mode only (default 2)
#   mode    "walk" (default): a local random walk starting from the bbox
#           center, small clusters of nearby tiles at a given zoom -- this is
#           what repeated cache-hit requests should look like.
#           "uniform": each tile drawn independently and uniformly at random
#           across the whole bbox-derived index range at that zoom, no
#           locality -- high-cardinality, cache-busting coverage of the
#           dataset's real extent.
#
# Output: one "z x y" line per tile, fields fixed in that order regardless of
# any URL convention: x is the tile column (from longitude), y is the tile
# row (from latitude, row 0 at the north -- same convention OGC API Tiles'
# TileRow uses). The caller decides how to place x and y in a URL --
# scenarios.sh's TILE_COORD_ORDER picks row-first ({tileMatrix}/{tileRow}/
# {tileCol}, OGC API Tiles' own order) or column-first (slippy/XYZ-style
# servers) from these same two fields.
#
# PRNG: Park-Miller minimal standard LCG, x1 = (16807 * x0) mod 2147483647.
# Chosen because 16807 * 2147483646 stays within a double's exact-integer range
# (2^53), so the sequence is exactly reproducible under IEEE-754 arithmetic
# regardless of which awk implementation runs it -- unlike glibc-style LCG
# constants, which overflow that range and drift across implementations.

function next_rand() {
    state = (16807 * state) % 2147483647
    return state
}

# Uniform integer in [0, n).
function rand_below(n) {
    return next_rand() % n
}

# Uniform integer in [lo, hi] (inclusive).
function rand_range(lo, hi) {
    return lo + rand_below(hi - lo + 1)
}

function lon2tile(lon, n) {
    v = int((lon + 180.0) / 360.0 * n)
    if (v < 0) v = 0
    if (v > n - 1) v = n - 1
    return v
}

function lat2tile(lat, n,    rad, tan_lat) {
    rad = lat * PI / 180.0
    tan_lat = sin(rad) / cos(rad)
    v = int((1.0 - log(tan_lat + 1.0 / cos(rad)) / PI) / 2.0 * n)
    if (v < 0) v = 0
    if (v > n - 1) v = n - 1
    return v
}

function clamp(v, lo, hi) {
    if (v < lo) return lo
    if (v > hi) return hi
    return v
}

BEGIN {
    PI = atan2(0, -1)

    if (maxstep == "" || maxstep < 1) maxstep = 2
    if (count == "" || count < 1) count = 1

    n = 2 ^ z

    xmin = lon2tile(west, n)
    xmax = lon2tile(east, n)
    if (xmin > xmax) { t = xmin; xmin = xmax; xmax = t }

    # north maps to the smaller tile row (row 0 is the top of the matrix).
    ymin = lat2tile(north, n)
    ymax = lat2tile(south, n)
    if (ymin > ymax) { t = ymin; ymin = ymax; ymax = t }

    # Fold seed into the LCG's valid range [1, m-1]; 0 is a fixed point.
    state = (seed % 2147483646)
    if (state < 0) state += 2147483646
    state += 1

    if (mode == "") mode = "walk"

    if (mode == "uniform") {
        for (i = 0; i < count; i++) {
            print z, rand_range(xmin, xmax), rand_range(ymin, ymax)
        }
    } else {
        x = int((xmin + xmax) / 2)
        y = int((ymin + ymax) / 2)

        for (i = 0; i < count; i++) {
            dx = rand_below(2 * maxstep + 1) - maxstep
            dy = rand_below(2 * maxstep + 1) - maxstep
            x = clamp(x + dx, xmin, xmax)
            y = clamp(y + dy, ymin, ymax)
            print z, x, y
        }
    }
}
