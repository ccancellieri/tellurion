# Public demo sources

`public-examples.yaml` is the single source of truth for the public-source
gallery. The interface may list every entry, but it may only enable an entry
whose `status` is `active` and whose `executable` flag is `true`.

The active examples are the ESA WorldCover 2021 Italy COG, the Monaco Open
Buildings GeoParquet object, and the Natural Earth 1:110m coastline Shapefile
archive. GeoParquet and COG are read through bounded byte ranges; the ZIP is
copied to a bounded, session-private spool before validation and extraction.
Every active object's length and strong ETag are checked by the opt-in
verifier. That verifier uses a one-byte `Range` request, refuses redirects,
sends no credentials, cookies, or proxy configuration, and asks for identity
encoding.

Candidate entries are deliberately non-executable. Their `connector` section
states what is missing, and their `activation` section makes the remaining
licence, attribution, immutability, or implementation gate explicit. A
`bounded-spool` candidate is still intended for direct use once enabled: it
uses a bounded temporary copy rather than an ingestion pipeline. A
`chunk-native` entry is a prefix, not one object, so it does not claim a single
length or ETag.

The gallery must render a licence link only when `license.verification` is
`confirmed`; then it uses `license.terms_url`. When verification is
`review-required`, it must show the honest `license.label` without a licence
link. A source page describes provenance, not necessarily the terms for a
fixture, and must not be presented as a licence link.

Run the ordinary schema check without network access:

```sh
cargo test -p tellurion-http-source --test public_demo_inventory
```

Run the live verification only when intentionally checking the active gallery:

```sh
cargo test -p tellurion-http-source --test public_demo_inventory -- --ignored
```

If an active object no longer returns the recorded exact 206 range response,
length, and strong ETag, the live verifier fails. Update the inventory only
after validating the upstream change, licence, attribution, and rendering
behaviour.

For the source build, stateless limits, and Render deployment procedure, see
[`docs/quickstart/public-demo.md`](../../docs/quickstart/public-demo.md).
