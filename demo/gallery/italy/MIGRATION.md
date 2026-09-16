# Italy field case source migration

This directory consolidates the Italy field case into Tellurion. It preserves
the existing analysis, measured results, reproduction scripts, attribution and
licensing; it does not repeat or update the historical benchmarks.

## Source revisions

- Original repository: https://github.com/ccancellieri/tellurion-italy-demo
- Local source revision: `dc74cf328cd83d33c0e5b0a7831f02101d056fab`.
- Public source revision reviewed: `5f26af7437a30d93b5e9cb003e2caad82a044061`.

The local revision retains the read-only YAML guards, explicit dependency
checks, readiness health check and human-facing landing links. The public
revision adds the STAC field-note links, which are included here. Its weaker
health check and write-enabled reproduction configuration are not imported.

## Publication status

Canonical Pages is live at <https://ccancellieri.github.io/tellurion/italy/>.
Active download links target the byte-identical historical assets under the
`archive-italy-demo-v0.2.0` and `archive-tellurion-v0.3.0` tags in Tellurion.
Confirm their checksums before retiring either old repository. Existing Render
services still need to be repointed to this monorepo and tested independently;
the Pages move does not establish a working replacement backend.

See [the migration tracker](https://github.com/ccancellieri/tellurion/issues/37).
