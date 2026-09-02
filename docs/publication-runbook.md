# Publication runbook

This runbook prepares evidence for an owner decision; it does not authorize publication.

1. Create a fresh mirror of the candidate and record its immutable commit.
2. Create the private snapshot in an empty directory outside every repository, including disclosure-surface downloads:
   `scripts/snapshot-publication-state.sh --download-disclosure-surfaces OWNER/REPOSITORY /private/path`.
   The raw evidence, logs, artifacts, and assets stay outside Git.
3. Scan the complete history and every reachable ref. Scan every downloaded GitHub surface from the one canonical private root with
   `gitleaks dir --redact=100 --no-banner --no-color "$PRIVATE_EVIDENCE/github/disclosure-surfaces"`.
   This includes extracted Actions logs and artifacts, release assets, issue-comment bodies, and attachment references; keep the raw scanner report outside Git.
4. Keep the raw locked-dependency inventory outside Git. `THIRD_PARTY_NOTICES.json` is deterministic package/version/licence inventory evidence for candidate review; it is not legal approval or a conclusion that distribution obligations are satisfied.
5. Obtain qualified legal/provenance confirmation, dependency-notice review, and dataset-attribution sign-off. External-code merges remain blocked until a reviewed contributor agreement is operational.
6. If the existing history cannot safely be exposed, create a clean export and a fresh-root public commit; retain the original only as a private archive.
7. Verify the candidate through an anonymous clone and safe workflow execution, including expected refs, licence display, and permissions.
8. Obtain the final explicit owner decision on repository naming and visibility before making any public change.

Making a repository private later cannot retract copies, clones, forks, logs, or assets already disclosed.
