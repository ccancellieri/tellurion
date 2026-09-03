# Publishing the Tellurion crate family

Tellurion publishes one workspace version across an explicitly ordered set of
27 crates. Publication is manual, irreversible, and separate from Git tags,
GitHub Releases, and binary deployment. The workflow never creates or pushes
any of those objects.

The source of truth for order is
[`release/crates-io-packages.txt`](../release/crates-io-packages.txt). The
workspace version in `Cargo.toml`, the exact 40-character commit, and an
already-existing `v<version>` tag must all identify the same clean tree. The
workflow also requires that commit to be the current `main` commit selected by
the manual run.

## Permanent-registry rule

A crates.io version cannot be overwritten or deleted. Yanking prevents new
dependency resolution from selecting it, but does not remove its source and
does not break existing lockfiles. Treat the final environment approval as the
point of no return. See the official [Cargo publishing
documentation](https://doc.rust-lang.org/cargo/reference/publishing.html).

## One-time owner setup

1. Create a public GitHub Environment named exactly `crates-io`.
2. Restrict it to the protected `main` branch, add a required reviewer, disable
   administrator bypass, and prevent self-review when a second trusted reviewer
   is available. Environment protection is an owner-side control and cannot be
   encoded in this repository.
3. Do not add `CARGO_REGISTRY_TOKEN` or another registry secret to GitHub.
4. For every existing crate, open its crates.io settings and add this trusted
   publisher:

   - provider: GitHub Actions
   - repository owner: `ccancellieri`
   - repository: `tellurion`
   - workflow: `publish-crates.yml`
   - environment: `crates-io`

5. After successfully testing Trusted Publishing, enable crates.io's
   **Trusted Publishing Only** mode for every crate and revoke obsolete API
   tokens.

The publish job grants only `contents: read` and `id-token: write`. The latter
lets the official `rust-lang/crates-io-auth-action` exchange GitHub's OIDC
identity for a temporary crates.io token; the action revokes that token when
the job ends. GitHub documents that `id-token: write` grants token-request
ability, not repository write access. crates.io binds the publisher to the
repository, workflow filename, and optional environment.

## First publication of new names

crates.io cannot configure Trusted Publishing until a crate has been published
once. As checked on 3 September 2026, these 13 names still require their first
publication:

```
tellurion-http-source
tellurion-vector-tile
tellurion-cog
tellurion-zarr
tellurion-control
tellurion-control-sqlite
tellurion-control-postgres
tellurion-shapefile
tellurion-geopackage
tellurion-iceberg
tellurion-duckdb
tellurion-records
tellurion-processes
```

Name availability is first-come, first-served and must be checked again at the
time of release. The workflow preflight refuses all uploads while any allowlist
name is absent, avoiding a predictable mid-run failure.

For the first release only, the crates.io owner must use a short-expiry API
token with the minimum available operations from a clean checkout of the tagged
commit. Export it only in that terminal; never save it in the repository or
GitHub:

```bash
read -rsp 'Temporary crates.io token: ' CARGO_REGISTRY_TOKEN && printf '\n'
export CARGO_REGISTRY_TOKEN
export TELLURION_BOOTSTRAP_CONFIRM='publish first crates for 0.5.0-rc.1 from <40-character-commit>'
./scripts/publish-crates-io.sh \
  --bootstrap \
  --registry crates-io \
  --version 0.5.0-rc.1 \
  --commit <40-character-commit> \
  --resume-from tellurion-core
unset CARGO_REGISTRY_TOKEN TELLURION_BOOTSTRAP_CONFIRM
```

The bootstrap mode refuses to run inside GitHub Actions. It still verifies the
clean tree, exact tag, version, commit, crate order, and byte identity of any
already-present version. Revoke the temporary token immediately afterward,
then add the trusted publisher above to each newly created crate. Future
releases use OIDC only.

## Manual OIDC release

Before dispatching, ensure the release commit is merged as the tip of `main`,
create the exact `v<version>` tag on that commit, and push only those objects as
separate owner actions. Then open **Actions → Publish crates to crates.io → Run
workflow**, select `main`, and enter:

- `version`: the exact workspace version, such as `0.5.0-rc.1`;
- `commit`: the full lowercase 40-character commit;
- `confirmation`: `publish <version> from <commit>`;
- `resume_from`: empty for a normal run, or the first crate whose state needs
  reconsideration after a partial run.

The ungated verification job checks the immutable identity, publication and
licence policies, their mutation tests, the full workspace tests, all crate
packages, and live registry state. The publish job starts only after those
checks and the `crates-io` environment approval succeed. It repeats the
identity and policy checks before requesting its temporary token.

## Partial publication and safe resume

There is no registry transaction spanning 27 crates. A network or registry
failure can therefore leave an ordered prefix published. Rerun the same
version and commit; optionally set `resume_from` to the crate named in the
failure message.

For every already-published crate/version, the script rebuilds the `.crate`
from the tagged commit and compares it byte-for-byte with crates.io. An exact
match is skipped. A mismatch stops the run because the immutable version cannot
be repaired. Every crate before `resume_from` must already exist and match;
missing predecessors are rejected. After each upload, the script verifies the
remote bytes even when Cargo reports a timeout, because Cargo documents that a
polling timeout does not mean the upload failed.

Never change source while resuming. If an uploaded crate is wrong, stop, assess
whether it must be yanked, fix forward under a new version, and keep the
published source available as required by crates.io.

## Official references

- [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing)
- [Official crates.io authentication action](https://github.com/rust-lang/crates-io-auth-action)
- [Cargo `publish`](https://doc.rust-lang.org/cargo/commands/cargo-publish.html)
- [GitHub OIDC permissions](https://docs.github.com/en/actions/reference/security/oidc)
- [GitHub deployment environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)
