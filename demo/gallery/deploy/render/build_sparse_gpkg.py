#!/usr/bin/env python3
"""Seed sparse pentadal & dekadal FAO-style granule tables into a GeoPackage."""

from __future__ import annotations

import pathlib
import sqlite3
import struct
import sys


def make_gpkg_polygon_wkb(minx: float, miny: float, maxx: float, maxy: float) -> bytes:
    """Encode an EPSG:4326 bounding rectangle in GeoPackage StandardBinary WKB."""
    # GP magic + version 0 + flags (0x03 = binary little-endian, envelope XY) + srid 4326
    gp_header = b"GP\x00\x03" + struct.pack("<I4d", 4326, minx, maxx, miny, maxy)
    ring = [
        (minx, miny),
        (maxx, miny),
        (maxx, maxy),
        (minx, maxy),
        (minx, miny),
    ]
    wkb = struct.pack("<BIII", 1, 3, 1, len(ring))
    for x, y in ring:
        wkb += struct.pack("<dd", x, y)
    return gp_header + wkb


def seed_gpkg(gpkg_path: pathlib.Path) -> None:
    conn = sqlite3.connect(str(gpkg_path))
    cur = conn.cursor()

    cur.execute(
        """
        CREATE TABLE IF NOT EXISTS pentadal_sparse_granules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            geom BLOB NOT NULL,
            pentad_id TEXT NOT NULL,
            start_datetime TEXT NOT NULL,
            end_datetime TEXT NOT NULL,
            status TEXT NOT NULL,
            coverage_note TEXT NOT NULL
        )
        """
    )
    cur.execute("DELETE FROM pentadal_sparse_granules")

    cur.execute(
        """
        CREATE TABLE IF NOT EXISTS dekadal_sparse_granules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            geom BLOB NOT NULL,
            dekad_id TEXT NOT NULL,
            start_datetime TEXT NOT NULL,
            end_datetime TEXT NOT NULL,
            status TEXT NOT NULL
        )
        """
    )
    cur.execute("DELETE FROM dekadal_sparse_granules")

    pentadal_rows = [
        (
            "2021-01-P1",
            "2021-01-01T00:00:00Z",
            "2021-01-05T23:59:59Z",
            "available",
            "full_aoi",
            (12.0, 39.0, 14.5, 41.5),
        ),
        (
            "2021-01-P3",
            "2021-01-11T00:00:00Z",
            "2021-01-15T23:59:59Z",
            "partial",
            "sw_only_partial_granule",
            (12.0, 39.0, 12.6, 39.6),
        ),
        (
            "2021-01-P6",
            "2021-01-26T00:00:00Z",
            "2021-01-31T23:59:59Z",
            "available",
            "irregular_6day_p6",
            (12.0, 39.0, 14.5, 41.5),
        ),
    ]

    dekadal_rows = [
        (
            "2021-01-D1",
            "2021-01-01T00:00:00Z",
            "2021-01-10T23:59:59Z",
            "available",
            (12.0, 39.0, 14.5, 41.5),
        ),
        (
            "2021-01-D3",
            "2021-01-21T00:00:00Z",
            "2021-01-31T23:59:59Z",
            "available",
            (12.0, 39.0, 14.5, 41.5),
        ),
        (
            "2021-02-D2",
            "2021-02-11T00:00:00Z",
            "2021-02-20T23:59:59Z",
            "available",
            (12.0, 39.0, 14.5, 41.5),
        ),
    ]

    for table in ("pentadal_sparse_granules", "dekadal_sparse_granules"):
        cur.execute(f"DROP TABLE IF EXISTS rtree_{table}_geom")
        cur.execute(
            f"CREATE VIRTUAL TABLE rtree_{table}_geom USING rtree(id, minx, maxx, miny, maxy)"
        )

    for pid, start_dt, end_dt, status, note, bbox in pentadal_rows:
        wkb = make_gpkg_polygon_wkb(*bbox)
        cur.execute(
            "INSERT INTO pentadal_sparse_granules (geom, pentad_id, start_datetime, end_datetime, status, coverage_note) VALUES (?, ?, ?, ?, ?, ?)",
            (wkb, pid, start_dt, end_dt, status, note),
        )
        rid = cur.lastrowid
        cur.execute(
            "INSERT INTO rtree_pentadal_sparse_granules_geom (id, minx, maxx, miny, maxy) VALUES (?, ?, ?, ?, ?)",
            (rid, bbox[0], bbox[2], bbox[1], bbox[3]),
        )

    for did, start_dt, end_dt, status, bbox in dekadal_rows:
        wkb = make_gpkg_polygon_wkb(*bbox)
        cur.execute(
            "INSERT INTO dekadal_sparse_granules (geom, dekad_id, start_datetime, end_datetime, status) VALUES (?, ?, ?, ?, ?)",
            (wkb, did, start_dt, end_dt, status),
        )
        rid = cur.lastrowid
        cur.execute(
            "INSERT INTO rtree_dekadal_sparse_granules_geom (id, minx, maxx, miny, maxy) VALUES (?, ?, ?, ?, ?)",
            (rid, bbox[0], bbox[2], bbox[1], bbox[3]),
        )

    cur.execute(
        """
        INSERT OR REPLACE INTO gpkg_contents
            (table_name, data_type, identifier, description, last_change, min_x, min_y, max_x, max_y, srs_id)
        VALUES
            ('pentadal_sparse_granules', 'features', 'pentadal_sparse_granules',
             'Sparse pentadal granule footprints (P2/P4/P5 missing, P3 SW-partial)', '2021-01-01T00:00:00Z',
             12.0, 39.0, 14.5, 41.5, 4326),
            ('dekadal_sparse_granules', 'features', 'dekadal_sparse_granules',
             'Sparse dekadal granule footprints (2021-01-D2 intentionally missing)', '2021-01-01T00:00:00Z',
             12.0, 39.0, 14.5, 41.5, 4326)
        """
    )
    cur.execute(
        """
        INSERT OR REPLACE INTO gpkg_geometry_columns
            (table_name, column_name, geometry_type_name, srs_id, z, m)
        VALUES
            ('pentadal_sparse_granules', 'geom', 'POLYGON', 4326, 0, 0),
            ('dekadal_sparse_granules', 'geom', 'POLYGON', 4326, 0, 0)
        """
    )
    cur.execute(
        "DELETE FROM gpkg_extensions WHERE table_name IN ('pentadal_sparse_granules', 'dekadal_sparse_granules')"
    )
    cur.execute(
        """
        INSERT INTO gpkg_extensions (table_name, column_name, extension_name, definition, scope)
        VALUES
            ('pentadal_sparse_granules', 'geom', 'gpkg_rtree_index', 'http://www.geopackage.org/spec120/#extension_rtree', 'write-only'),
            ('dekadal_sparse_granules', 'geom', 'gpkg_rtree_index', 'http://www.geopackage.org/spec120/#extension_rtree', 'write-only')
        """
    )
    conn.commit()
    conn.close()


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: build_sparse_gpkg.py PATH_TO_WORLDCOVER_GPKG")
    gpkg = pathlib.Path(sys.argv[1])
    if not gpkg.exists():
        raise SystemExit(f"GeoPackage not found: {gpkg}")
    seed_gpkg(gpkg)


if __name__ == "__main__":
    main()
