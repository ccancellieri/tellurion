#!/usr/bin/env bash
# Offline checks for the public demo source; never wakes hosted backends.
set -euo pipefail
cd "$(dirname "$0")/../demo/gallery"
python3 -m unittest discover -s tests -p 'test_*.py'
for contract in daily_automation render_deployment render_raster render_stac_harvest render_zarr render_3d render_protocol_gallery docs_site proof_brief; do
    sh "tests/${contract}_contract.sh"
done
python3 tests/static_site_links.py
for snapshot in snapshots/*; do
    python3 scripts/demo_snapshots.py "$snapshot"
done
