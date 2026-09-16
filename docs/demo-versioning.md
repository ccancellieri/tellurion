# Versioned demos and permanent links

The authoritative public source is now `demo/gallery` in
[Tellurion](https://github.com/ccancellieri/tellurion/tree/main/demo/gallery).
The `tellurion-demos` repository remains the publication destination and retains
its existing GitHub Pages and Render addresses during migration. The Italy
case-study source now lives in `italy/`; its legacy deployment and downloads
remain active until the replacement is verified. See
[Italy migration provenance](../italy/MIGRATION.md) and
[repository retirement](https://github.com/ccancellieri/tellurion/issues/37).
No private repository history or Enterprise code is imported.

## Existing versions

`releases/catalog.json` records the configured engine version and service origin
for each lane. `releases/` provides permanent version entry points. The initial
`snapshots/2026-09-15/` captures the public gallery from commit
`020fa04b8f04cf03daa25325e4bf5546b1d68bdf` of `tellurion-demos`. This is a dated
viewer snapshot, not a reconstruction of the release-day website. Its gallery
was mixed: raster/Zarr/3D use historical 0.3.0; vector/query/maps/STAC use
0.5.0-rc.1 source. The public-file evaluator is a separately evolving service.

Existing `/demos/...` links remain in place. Never silently repoint a published
historical backend to a newer engine. A snapshot protects local viewer and
evidence bytes, not third-party CDN files, external datasets or service uptime.
Historical engine archives keep their BUSL-1.1 terms. Public source is AGPL;
dataset-specific terms and attribution remain unchanged.

Source release exports omit vendored archives under `demo/gallery/dist/*.zip`
to avoid nesting previous full source releases inside new ones. Gallery source,
documentation and frozen viewers remain included. Use the Git checkout when
publishing the complete gallery with its historical downloadable archives.

## New release procedure

1. Commit the reviewed gallery, configurations and data provenance with the
   intended Tellurion release. Record exact engine commit/image digest, features,
   configuration and data checksums. A version string alone is insufficient.
2. Provision a **separate** service for that engine version. Keep old service
   names and domains. Pin its immutable image or source revision; disable
   automatic deployment from a moving development branch. Verify the configured
   version against the running service before labelling it live.
3. Keep data read-only and either baked into the image or reproducibly restored
   from checksummed inputs: free Render local storage is ephemeral. Do not share
   writable stores or migrations across historical engine versions.
4. Freeze the viewer from its exact commit, from this directory:

   ```sh
   python3 scripts/demo_snapshots.py --freeze COMMIT snapshots/RELEASE-SNAPSHOT
   ```

   Existing destinations are refused. Add a version entry under `releases/`
   linking the matching snapshot lanes, and extend the catalog. Do not advertise
   v0.5.0 final until its corresponding release and services are verified.
5. Run `bash scripts/check-gallery.sh` from the Tellurion repository root.
   Validate the selected live maps and their attribution separately. Retained
   pages and passing static tests are not proof of running backend identity.
6. Commit, review and merge in Tellurion. From a clean source checkout, export
   to a separate clean checkout of the existing publishing repository:

   ```sh
   python3 demo/gallery/scripts/publish_gallery.py /absolute/path/to/publishing-checkout
   python3 demo/gallery/scripts/publish_gallery.py /absolute/path/to/publishing-checkout --apply
   ```

   The first command previews the file count. Export refuses changes to an
   existing version/snapshot, preserves older files, and never deletes, commits
   or pushes. Review the destination diff, run its checks and publish normally.
   `publication.json` records the source commit. Repository hosting configuration
   and credentials are never exported.

## Wake-up, not redeploy

A request to an existing idle Render service can wake that service. Visitors
must never receive deploy credentials or trigger a rollback of a shared server.
Only the selected demo should make requests; the version index makes none.
Avoid keep-alive polling that prevents historical services from sleeping.

Render Free currently shares 750 instance hours per workspace per month and
sleeps after 15 idle minutes. Cold starts, quotas, remote data changes and
historical software vulnerabilities remain operational concerns. Suspend an
unsafe historical backend with an explicit explanation rather than promising
indefinite availability. Preserve evidence and a reproducible local route.

The initial migration does **not** create new cloud services or prove that
automatic deployment has been disabled in the Render dashboard. That deployment
freeze is a separate operational step, to be verified before replacing any
current service with a new release.
