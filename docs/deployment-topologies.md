# Deployment topologies and the portability floor

Tellurion is a static binary plus PostgreSQL. That is the whole runtime, and it
is why this document targets *conformance floors* rather than distributions: if
a platform meets the floor below, the manifests in `deploy/` apply to it
unchanged, and no vendor-specific artifact is needed or wanted.

## The floor

1. **CNCF-conformant Kubernetes API only.** Plain `Deployment`, `Service`,
   `ConfigMap`, `Secret`. No operator, no custom resource, and no admission
   plugin is required to run tellurion. (Two overlays *offer* CRD-backed
   conveniences — `gateway-api` and `openshift` — but the base serves traffic
   without either.)
2. **Pod Security Standards `restricted`.** The image runs non-root with an
   arbitrary UID, drops every capability, forbids privilege escalation and uses
   a read-only root filesystem. `scripts/validate-deploy-manifests.sh` asserts
   this against every rendered overlay in CI, so the promise cannot rot.
3. **Ingress portability.** A standard `Ingress` (k3s, EKS, GKE, AKS overlays),
   Gateway API (`gateway-api` overlay) and the OpenShift `Route` all front the
   same `Service`. Every one of them is optional: nothing in the base depends on
   a particular ingress implementation.
4. **Air-gap readiness.** `deploy/airgap/images.txt` lists every image the
   repository references, checked in CI against the manifests themselves.

Nothing here requires a cloud SDK, a managed database, or a proprietary control
plane. The same portability stance the storage layer takes — "any S3-compatible
endpoint", MinIO on-prem — applies to orchestration.

## Kubernetes overlays

All under `deploy/k8s/`. Render any of them with `kubectl kustomize` or
`kustomize build`; the output is plain YAML with no post-processing.

| Path | What it adds | For |
| --- | --- | --- |
| `base` | Deployment, Service, ConfigMap | Any conformant cluster; also consumable by `podman kube play` |
| `overlays/k3s` | Traefik-annotated `Ingress`, host `tellurion.local` | k3s and the rest of the light on-prem family |
| `overlays/eks` | AWS Load Balancer Controller `Ingress` | EKS |
| `overlays/gke` | `gce` `Ingress` + container-native load balancing annotation on the Service | GKE |
| `overlays/aks` | Application routing add-on `Ingress` | AKS |
| `overlays/openshift` | `Route` + restricted-SCC security context | OpenShift **and OKD** |
| `overlays/gateway-api` | `HTTPRoute` attached to an existing `Gateway` | Any cluster with Gateway API, no vendor annotations at all |
| `overlays/ha` | The `ha` component: `PodDisruptionBudget` + 2 replicas | Multi-replica installs (read the caveat below first) |

`deploy/k8s/components/ha` is a kustomize *component*, so a cloud overlay can
opt into the same PDB and replica count without duplicating it:

```yaml
# deploy/k8s/overlays/<your-overlay>/kustomization.yaml
components:
  - ../../components/ha
```

`deploy/k8s/examples/hpa.yaml` is a `HorizontalPodAutoscaler` example, kept out
of the overlays because an HPA and a fixed replica count fight over the same
field, and because autoscaling thresholds are a capacity decision nobody should
inherit from a repository default. It scales on CPU (metrics-server, portable
everywhere) and, optionally, on tellurion's own `tenant_admission_queue_depth`
metric via a custom-metrics adapter.

### OpenShift and OKD

The `openshift` overlay serves **OKD** — the fully open-source distribution
OpenShift is built from — unchanged. Same restricted SCC, same `Route` API, same
manifests, no license. This matters for the "open standards, on-prem, no
proprietary dependency" case where OpenShift Container Platform itself would not
qualify.

The overlay clears the base's `runAsUser`/`runAsGroup` pins rather than setting
different ones: the restricted SCC assigns a UID from a per-namespace range at
admission time, and pinning either field would be rejected. `runAsNonRoot` and
the seccomp profile stay enforced, so the result satisfies the restricted SCC
*and* PSS `restricted` from one manifest set.

### Distributions covered by the floor, with nothing to add

These need no dedicated overlay or tooling — they are conformant Kubernetes, so
the base plus whichever ingress overlay matches the cluster is the whole story:

- **Rancher RKE2** (and Rancher-managed clusters): CNCF-conformant, with a
  hardened/FIPS-oriented posture that pairs naturally with the PSS `restricted`
  floor.
- **k0s, MicroK8s**: same family as the k3s overlay's light on-prem footprint.
- **Talos Linux**: an immutable, API-managed minimal OS running vanilla
  Kubernetes — an excellent bare-metal security floor, and nothing to do on the
  tellurion side.
- **VMware Tanzu, EKS Anywhere, GKE on-prem, AKS-Arc**: conformant
  distributions; building anything vendor-specific for them would violate the
  "no vendor SDKs" principle for no gain.

## Running more than one replica

Read serving is stateless: request handling touches no process-local state that
another replica needs, so any number of replicas can serve traffic behind one
Service. Readiness makes rollouts safe on its own — `/readyz` turns 200 only
once the registry and every configured storage have passed their latest probe,
and turns 503 the moment the process starts draining on SIGTERM, so a rolling
update never routes to a replica that is on its way out.

The constraint is the **background outbox consumers**, not the serving path.
The index applier, the tile-invalidation consumer, webhook delivery and outbox
retention are each a single ordered consumer per collection. So:

- **All four consumers are OFF by default** (`server.index_applier.enabled`,
  `server.tile_invalidation.enabled`, `server.webhook_delivery.enabled`,
  `server.outbox_retention.enabled`). A deployment that has not turned any of
  them on — which is every deployment that only serves reads and writes through
  the API — can run N replicas today with no coordination whatsoever.
- **With the index applier enabled, declare a lease** (`server.index_applier.
  lease`) and N replicas coordinate themselves: a PostgreSQL advisory lock
  elects one leader per collection, the others idle and take over when the
  leader goes away. Declaring the key *is* the opt-in — there is no separate
  `enabled` flag, and an undeclared lease leaves the applier behaving exactly
  as it did before. A collection whose configured lease cannot be resolved is
  skipped rather than started unleased, since starting it would produce the
  very second drainer the declaration exists to prevent.
- **With any consumer enabled and no lease declared, run exactly one replica.**
  Two appliers draining the same collection is not a correctness bug in the
  apply itself (obligations are idempotent and version-gated) but it duplicates
  work, contends on the same rows, and gives up the "single ordered consumer"
  property the contract is built on. The lease covers the index applier; the
  other three consumers have no lease yet, so a deployment that enables those
  still needs the single-replica split below.

That split remains the answer for the unleased consumers: one single-replica
Deployment with them enabled, and a separate multi-replica Deployment with them
off serving read traffic. Both point at the same database and use the same
image; only the ConfigMap differs.

## Air-gapped installs

`deploy/airgap/images.txt` lists every image this repository references —
runtime images first, then the two build-time base images needed only if the
container is built inside the air gap. The file's header shows the mirroring
one-liner and the `kustomize edit set image` invocation that repoints an
overlay at the internal registry without patching a manifest.

CI reconciles the list against the rendered manifests, the compose file and the
Dockerfile in both directions: a referenced-but-unlisted image fails the build
(an install would stall pulling it), and so does a listed-but-unreferenced one
(a mirror would copy something nothing uses).

## Without Kubernetes

Orchestrator-independence is not a claim, it is the consequence of shipping a
static binary. Each of these is a worked example, not a supported product
surface:

| Path | Shape |
| --- | --- |
| `deploy/systemd/tellurion.service` | Single node, hardened systemd unit, no container |
| `deploy/systemd/ha/keepalived.conf.example` | Two nodes + a floating VIP; failover driven by `/readyz` |
| `deploy/systemd/ha/haproxy.cfg.example` | Optional companion turning the same two nodes active/active |
| `deploy/podman/tellurion.container` | Rootless Podman Quadlet unit — containers under plain systemd, no daemon |
| `deploy/nomad/tellurion.nomad.hcl` | Nomad `exec` job running the raw binary; no OCI image involved |
| `deploy/compose/docker-compose.yml` | Single-host stack including PostGIS, for development and benchmarks |

The keepalived pair is the important one: two hosts, one VIP, one PostgreSQL,
and no cluster software at all. It is the simple on-prem floor the design must
never break, and it is also the proof that coordination genuinely lives in the
application — keepalived only ever decides which node owns an *address*, never
which node owns the *work*.

## What CI enforces

`scripts/validate-deploy-manifests.sh` (the `deploy manifests` job in
`.github/workflows/ci.yml`) runs on every pull request and:

1. renders the base and **every** overlay, discovering them by directory listing
   so a new overlay is covered the day it is added;
2. asserts PSS `restricted` on every rendered pod-bearing object, via
   `scripts/check-pss-restricted.py`, which implements the profile's controls
   directly — no cluster, no admission webhook, no vendor linter;
3. parses the standalone examples;
4. reconciles the air-gap image list.

It runs locally the same way, given `python3` with PyYAML and either
`kustomize` or a recent `kubectl`:

```sh
./scripts/validate-deploy-manifests.sh
```
