# Reproduce before you believe the screenshot

**Scheduled: 2026-08-07**

A map card can communicate a result, but it cannot show how the result was made. For this Rome example, the useful artifact is the chain a reviewer can inspect: input snapshot, code, command, generated vectors, metrics, and image.

The smallest path is intentionally short:

```bash
chmod +x reproduce.sh
./reproduce.sh
```

Those commands and their requirements are documented in [README.md](../README.md#reproduce-in-two-commands). The default run is network-independent after download. `./reproduce.sh --refresh` is explicitly different: it retrieves fresh OSM data and reads the remote ESA COG window. Report which mode you used when comparing results.

The same README documents `--install`, `--load-rome`, and `--serve-rome`, plus feature and vector-tile requests. Tellurion 0.3.0 binaries are public for macOS Apple Silicon, Linux x86_64 and Windows x86_64; the installer selects the matching macOS or Linux archive and verifies it against the published SHA-256 manifest. That path demonstrates an on-prem Tellurion deployment over a GeoPackage; it does not transform the Python/Rasterio calculation into a Tellurion analysis capability.

Reproducibility is also a way to invite disagreement. If a command fails, the most useful report includes the OS/architecture, command, error, and relevant artifact such as [output/metrics.json](../output/metrics.json). If it succeeds but the definition is weak, that is equally valuable feedback.

Would you rather receive a geospatial demo as a container, a small shell runner, or both? And what is the first thing you look for before trusting a published map?

#Reproducibility #OpenScience #DevOps #Geospatial #OpenSource
