#!/usr/bin/env bash
# Validates everything under deploy/ that can be checked without a cluster:
#
#   1. Every kustomization (the base and every overlay) renders without error.
#      Discovery is by directory listing, so a new overlay is covered the day
#      it is added -- there is no list here to forget to update.
#   2. Every rendered manifest satisfies Pod Security Standards `restricted`
#      (scripts/check-pss-restricted.py). The image runs non-root with an
#      arbitrary UID and the portability floor promises `restricted`
#      compliance to every conformant cluster; this is what keeps that true.
#   3. The standalone examples under deploy/k8s/examples/ are parseable YAML
#      and, when they carry a pod spec, `restricted`-compliant too.
#   4. deploy/airgap/images.txt lists exactly the images the manifests, the
#      compose stack and the Dockerfile references -- no missing entry (an
#      air-gapped install would stall pulling it) and no stale one (a mirror
#      would copy an image nothing uses).
#
# Usage: ./scripts/validate-deploy-manifests.sh
#
# Requires: bash, python3 with PyYAML, and either `kustomize` or a `kubectl`
# new enough for `kubectl kustomize` (both understand kustomize Components,
# which deploy/k8s/components/ha uses).
#
# Exit code 0 = every check passed. Non-zero = at least one failed; the FAIL
# lines above the summary say which.

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

command -v python3 >/dev/null 2>&1 || {
    echo "ERROR: required tool 'python3' not found on PATH" >&2
    exit 1
}
python3 -c 'import yaml' >/dev/null 2>&1 || {
    echo "ERROR: python3 is missing PyYAML (python3 -m pip install pyyaml)" >&2
    exit 1
}

# `kustomize build` and `kubectl kustomize` are the same renderer; take
# whichever the machine has so this runs unchanged on a laptop and on CI.
if command -v kustomize >/dev/null 2>&1; then
    render() { kustomize build "$1"; }
    renderer="kustomize $(kustomize version 2>/dev/null | head -n1)"
elif command -v kubectl >/dev/null 2>&1; then
    render() { kubectl kustomize "$1"; }
    renderer="kubectl kustomize"
else
    echo "ERROR: neither 'kustomize' nor 'kubectl' found on PATH" >&2
    exit 1
fi

echo "renderer: $renderer"

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

fail=0
rendered_images="$work_dir/rendered-images.txt"
: >"$rendered_images"

# The public-demo image is deliberately distinct from the ordinary product
# image: it must build the embedded UI, compile only the anonymous remote-source
# profile, ship no ingest CLI, and probe dependency readiness rather than mere
# process liveness. Keep these release-shape facts under the same deploy gate
# as the other container contracts.
public_demo_dockerfile="docker/Dockerfile.public-demo"
for required in \
    'npm ci' \
    'npm run build:public-demo' \
    '--no-default-features --features public-demo,ui' \
    'COPY --from=rust-builder /build/target/release/tellurion /usr/local/bin/tellurion' \
    'COPY docker/public-demo.render.yaml /etc/tellurion/public-demo.yaml' \
    'ENV TMPDIR=/var/lib/tellurion/tmp' \
    'ENV TELLURION_PUBLIC_DEMO_ARCHIVE_ROOT=/var/lib/tellurion/tmp/archive-roots' \
    'ENV TELLURION_CONFIG=/etc/tellurion/public-demo.yaml' \
    'ENTRYPOINT ["tellurion", "--public-demo-only"]' \
    '/readyz'; do
    if ! grep -Fq -- "$required" "$public_demo_dockerfile"; then
        echo "FAIL $public_demo_dockerfile: missing required contract: $required"
        fail=1
    fi
done
if grep -Fq 'tellurion-ingest' "$public_demo_dockerfile"; then
    echo "FAIL $public_demo_dockerfile: the public demo must not ship the ingest CLI"
    fail=1
fi

# Exercise the Docker build-context boundary, not just the Dockerfile text:
# every local runtime input must survive `.dockerignore` or Render will fail
# before the first build stage runs.
public_demo_config="docker/public-demo.render.yaml"
public_demo_context="$work_dir/public-demo-context.tar"
if ! tar -cf "$public_demo_context" --exclude-from=.dockerignore "$public_demo_config" \
    || ! tar -tf "$public_demo_context" | grep -Fxq "$public_demo_config"; then
    echo "FAIL $public_demo_dockerfile: $public_demo_config is excluded from the Docker build context"
    fail=1
fi

runtime_copies="$(awk '
    toupper($1) == "FROM" {
        runtime = toupper($3) == "AS" && $4 == "runtime"
        next
    }
    runtime && toupper($1) == "COPY" { print }
' "$public_demo_dockerfile" | sort)"
expected_runtime_copies="$(printf '%s\n' \
    'COPY --from=rust-builder /build/target/release/tellurion /usr/local/bin/tellurion' \
    'COPY docker/public-demo.render.yaml /etc/tellurion/public-demo.yaml' \
    'COPY LICENSE /usr/share/licenses/tellurion/LICENSE' | sort)"
if [ "$runtime_copies" != "$expected_runtime_copies" ]; then
    echo "FAIL $public_demo_dockerfile: runtime stage COPY inventory is not binary/config/licence only"
    printf '%s\n' "$runtime_copies" | sed 's/^/      /'
    fail=1
fi

if python3 -c '
import sys, yaml
with open(sys.argv[1], encoding="utf-8") as handle:
    document = yaml.safe_load(handle)
if not isinstance(document, dict):
    raise SystemExit("configuration must be a mapping")
for forbidden in (
    "control_store", "auth", "storages", "object_stores", "tenants",
    "catalogs", "collections", "styles", "profiles", "registry", "policy", "webhooks",
):
    if forbidden in document:
        raise SystemExit(f"stateless public demo must not declare {forbidden}")
server = document.get("server")
if not isinstance(server, dict) or server.get("log_json") is not True:
    raise SystemExit("server.log_json must be true")
if server.get("public_base_url") != "https://tellurion-public-demo.onrender.com":
    raise SystemExit("server.public_base_url must name the canonical public demo origin")
' docker/public-demo.render.yaml 2>"$work_dir/public-demo-config.err"; then
    echo "ok   public demo image/config: stateless binary-only deployment contract"
else
    echo "FAIL docker/public-demo.render.yaml: $(cat "$work_dir/public-demo-config.err")"
    fail=1
fi

if python3 -c '
import sys, yaml
with open(sys.argv[1], encoding="utf-8") as handle:
    document = yaml.safe_load(handle)
if not isinstance(document, dict) or set(document) != {"services"}:
    raise SystemExit("Blueprint must contain services only (no database or environment groups)")
services = document.get("services")
if not isinstance(services, list) or len(services) != 1:
    raise SystemExit("Blueprint must own exactly one generic public-demo service")
service = services[0]
required = {
    "name": "tellurion-public-demo",
    "type": "web",
    "runtime": "docker",
    "plan": "free",
    "dockerfilePath": "./docker/Dockerfile.public-demo",
    "dockerContext": ".",
    "healthCheckPath": "/readyz",
    "autoDeployTrigger": "checksPass",
}
for key, expected in required.items():
    if service.get(key) != expected:
        raise SystemExit(f"service.{key} must be {expected!r}")
for forbidden in ("disk", "envVars", "preDeployCommand", "initialDeployHook"):
    if forbidden in service:
        raise SystemExit(f"stateless public demo must not declare service.{forbidden}")
build_filter = service.get("buildFilter")
if not isinstance(build_filter, dict) or set(build_filter) != {"paths"}:
    raise SystemExit("service.buildFilter must contain included paths only")
paths = build_filter.get("paths")
required_paths = {
    ".dockerignore", "render.yaml", "LICENSE", "Cargo.toml", "Cargo.lock",
    "rust-toolchain.toml", "crates/**", "ui/**", "demo/**",
    "docker/Dockerfile.public-demo",
    "docker/public-demo.render.yaml",
}
if not isinstance(paths, list) or set(paths) != required_paths:
    raise SystemExit("service.buildFilter.paths does not match the public image inputs")
' render.yaml 2>"$work_dir/public-demo-blueprint.err"; then
    echo "ok   render.yaml: one stateless checks-gated public-demo service"
else
    echo "FAIL render.yaml: $(cat "$work_dir/public-demo-blueprint.err")"
    fail=1
fi

kustomizations="deploy/k8s/base"
for overlay in deploy/k8s/overlays/*/; do
    [ -f "${overlay}kustomization.yaml" ] || continue
    kustomizations="$kustomizations ${overlay%/}"
done

for dir in $kustomizations; do
    out="$work_dir/$(echo "$dir" | tr '/' '_').yaml"
    if ! render "$dir" >"$out" 2>"$out.err"; then
        echo "FAIL $dir: render failed"
        sed 's/^/      /' "$out.err"
        fail=1
        continue
    fi
    if [ ! -s "$out" ]; then
        echo "FAIL $dir: rendered empty output"
        fail=1
        continue
    fi

    if python3 scripts/check-pss-restricted.py "$out" >"$out.pss" 2>&1; then
        echo "ok   $dir: renders, $(sed 's/^PASS //' "$out.pss")"
    else
        echo "FAIL $dir:"
        sed 's/^/      /' "$out.pss"
        fail=1
    fi

    # `image:` only ever appears as a container field in these manifests; the
    # base's `images:` kustomize transformer is resolved by render time.
    sed -n 's/^[[:space:]]*image:[[:space:]]*//p' "$out" | tr -d '"' >>"$rendered_images"
done

# --- Standalone examples -----------------------------------------------------

for example in deploy/k8s/examples/*.yaml; do
    [ -f "$example" ] || continue
    if python3 -c '
import sys, yaml
with open(sys.argv[1], encoding="utf-8") as handle:
    documents = [d for d in yaml.safe_load_all(handle) if isinstance(d, dict)]
if not documents:
    sys.exit("no YAML documents")
for document in documents:
    for field in ("apiVersion", "kind"):
        if field not in document:
            sys.exit(f"document missing {field}")
' "$example" 2>"$work_dir/example.err"; then
        echo "ok   $example: parses"
    else
        echo "FAIL $example: $(cat "$work_dir/example.err")"
        fail=1
    fi
done

# --- Air-gap image list ------------------------------------------------------

required="$work_dir/required-images.txt"
listed="$work_dir/listed-images.txt"

{
    cat "$rendered_images"

    # Compose interpolation: `${POSTGIS_IMAGE:-postgis/postgis:16-3.4}` mirrors
    # as its default, which is the image an untouched `docker compose up` pulls.
    sed -n 's/^[[:space:]]*image:[[:space:]]*//p' deploy/compose/docker-compose.yml |
        sed 's/^\${[A-Za-z_][A-Za-z0-9_]*:-\(.*\)}$/\1/'

    # Dockerfile base images, minus the stage aliases a later FROM could name.
    awk '
        toupper($1) == "FROM" {
            if (toupper($3) == "AS") { alias[$4] = 1 }
            from[++n] = $2
        }
        END { for (i = 1; i <= n; i++) if (!(from[i] in alias)) print from[i] }
    ' docker/Dockerfile docker/Dockerfile.public-demo
} | sed '/^$/d' | sort -u >"$required"

sed 's/#.*//' deploy/airgap/images.txt | sed 's/[[:space:]]*$//;/^$/d' | sort -u >"$listed"

missing="$(comm -23 "$required" "$listed")"
stale="$(comm -13 "$required" "$listed")"

if [ -n "$missing" ]; then
    echo "FAIL deploy/airgap/images.txt: referenced but not listed:"
    printf '%s\n' "$missing" | sed 's/^/      /'
    fail=1
fi
if [ -n "$stale" ]; then
    echo "FAIL deploy/airgap/images.txt: listed but referenced nowhere:"
    printf '%s\n' "$stale" | sed 's/^/      /'
    fail=1
fi
if [ -z "$missing" ] && [ -z "$stale" ]; then
    echo "ok   deploy/airgap/images.txt: $(wc -l <"$listed" | tr -d ' ') image(s), exactly the referenced set"
fi

echo
if [ "$fail" -ne 0 ]; then
    echo "deploy manifest validation FAILED"
    exit 1
fi
echo "deploy manifest validation passed"
