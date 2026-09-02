#!/usr/bin/env python3
"""Asserts that rendered Kubernetes manifests satisfy Pod Security Standards
`restricted`.

The tellurion image runs non-root with an arbitrary UID -- the OpenShift
restricted-SCC overlay depends on it, and the deployment portability floor
promises PSS `restricted` compliance to every conformant cluster. That promise
is only worth something if something checks it, so this script implements the
`restricted` profile's controls directly (as of Kubernetes v1.25, where the
profile was last versioned) and fails the build when a manifest drifts out of
compliance.

Implementing the controls here rather than shelling out to a cluster keeps the
check dependency-free: no cluster, no admission webhook, no vendor linter --
just the rendered YAML that `kustomize build` already produces.

Usage:  check-pss-restricted.py FILE [FILE...]      (`-` reads stdin)

Requires: python3 with PyYAML.

Exit code 0 = every pod-bearing object in every file complies. Non-zero = at
least one control failed; each FAIL line names the object, the field and the
control.
"""

import sys

try:
    import yaml
except ImportError:  # pragma: no cover - environment problem, not a policy one
    sys.stderr.write(
        "ERROR: PyYAML not found. Install it with `python3 -m pip install pyyaml`\n"
    )
    raise SystemExit(1)

# Objects that carry a pod template (or, for Pod, a pod spec directly). The
# value is the path from the object root down to the pod spec.
POD_SPEC_PATHS = {
    "Pod": ("spec",),
    "Deployment": ("spec", "template", "spec"),
    "ReplicaSet": ("spec", "template", "spec"),
    "ReplicationController": ("spec", "template", "spec"),
    "StatefulSet": ("spec", "template", "spec"),
    "DaemonSet": ("spec", "template", "spec"),
    "Job": ("spec", "template", "spec"),
    "CronJob": ("spec", "jobTemplate", "spec", "template", "spec"),
}

# Restricted narrows the baseline volume set to these types.
ALLOWED_VOLUME_TYPES = {
    "configMap",
    "csi",
    "downwardAPI",
    "emptyDir",
    "ephemeral",
    "persistentVolumeClaim",
    "projected",
    "secret",
}

# Restricted allows exactly one capability to be added back.
ALLOWED_ADDED_CAPABILITIES = {"NET_BIND_SERVICE"}

ALLOWED_SECCOMP_TYPES = {"RuntimeDefault", "Localhost"}

ALLOWED_SELINUX_TYPES = {"container_t", "container_init_t", "container_kvm_t"}

# The baseline profile's safe sysctl set; restricted inherits it unchanged.
ALLOWED_SYSCTLS = {
    "kernel.shm_rmid_forced",
    "net.ipv4.ip_local_port_range",
    "net.ipv4.ip_unprivileged_port_start",
    "net.ipv4.tcp_syncookies",
    "net.ipv4.ping_group_range",
    "net.ipv4.ip_local_reserved_ports",
}


def dig(obj, path):
    """Walks `path` through nested mappings, returning None if any hop is absent."""
    for key in path:
        if not isinstance(obj, dict):
            return None
        obj = obj.get(key)
    return obj


def containers_of(pod_spec):
    """Yields (label, container) for every container class the profile covers."""
    for field in ("initContainers", "containers", "ephemeralContainers"):
        for container in pod_spec.get(field) or []:
            name = container.get("name", "<unnamed>")
            yield f"{field}[{name}]", container


def check_host_namespaces(pod_spec, fail):
    for field in ("hostNetwork", "hostPID", "hostIPC"):
        if pod_spec.get(field):
            fail(f"spec.{field}=true (host namespaces are forbidden)")


def check_volumes(pod_spec, fail):
    for volume in pod_spec.get("volumes") or []:
        name = volume.get("name", "<unnamed>")
        # A volume is `{name: ..., <exactly one type key>: {...}}`.
        types = [key for key in volume if key != "name"]
        for volume_type in types:
            if volume_type not in ALLOWED_VOLUME_TYPES:
                fail(
                    f"spec.volumes[{name}] uses type '{volume_type}', "
                    "outside the restricted volume allowlist"
                )


def check_sysctls(pod_spec, fail):
    for sysctl in dig(pod_spec, ("securityContext", "sysctls")) or []:
        name = sysctl.get("name")
        if name not in ALLOWED_SYSCTLS:
            fail(f"spec.securityContext.sysctls['{name}'] is not in the safe set")


def check_selinux(where, security_context, fail):
    selinux_type = dig(security_context, ("seLinuxOptions", "type"))
    if selinux_type is not None and selinux_type not in ALLOWED_SELINUX_TYPES:
        fail(f"{where}.seLinuxOptions.type='{selinux_type}' is not allowed")
    for field in ("user", "role"):
        if dig(security_context, ("seLinuxOptions", field)) is not None:
            fail(f"{where}.seLinuxOptions.{field} must not be set")


def check_apparmor(metadata, fail):
    for key, value in (metadata.get("annotations") or {}).items():
        if not key.startswith("container.apparmor.security.beta.kubernetes.io/"):
            continue
        if value != "runtime/default" and not value.startswith("localhost/"):
            fail(f"annotation {key}='{value}' is not an allowed AppArmor profile")


def check_seccomp(pod_spec, fail):
    """Seccomp must be RuntimeDefault/Localhost, set on the pod or on every
    container -- `restricted` rejects an unset profile, not just a bad one."""
    pod_type = dig(pod_spec, ("securityContext", "seccompProfile", "type"))
    if pod_type is not None and pod_type not in ALLOWED_SECCOMP_TYPES:
        fail(
            f"spec.securityContext.seccompProfile.type='{pod_type}' "
            "must be RuntimeDefault or Localhost"
        )
    for where, container in containers_of(pod_spec):
        container_type = dig(
            container, ("securityContext", "seccompProfile", "type")
        )
        if container_type is not None and container_type not in ALLOWED_SECCOMP_TYPES:
            fail(
                f"spec.{where}.securityContext.seccompProfile.type="
                f"'{container_type}' must be RuntimeDefault or Localhost"
            )
        if container_type is None and pod_type is None:
            fail(
                f"spec.{where} has no seccompProfile and the pod sets none "
                "(restricted requires an explicit RuntimeDefault or Localhost)"
            )


def check_run_as_non_root(pod_spec, fail):
    """runAsNonRoot must be explicitly true on the pod or on every container."""
    pod_value = dig(pod_spec, ("securityContext", "runAsNonRoot"))
    if pod_value is False:
        fail("spec.securityContext.runAsNonRoot=false")
    for where, container in containers_of(pod_spec):
        container_value = dig(container, ("securityContext", "runAsNonRoot"))
        if container_value is False:
            fail(f"spec.{where}.securityContext.runAsNonRoot=false")
        if container_value is None and pod_value is not True:
            fail(
                f"spec.{where} does not set runAsNonRoot and the pod does not "
                "set it to true"
            )


def check_run_as_user(pod_spec, fail):
    """runAsUser may be unset (the OpenShift overlay clears it so the restricted
    SCC can assign one), but it may never be 0."""
    if dig(pod_spec, ("securityContext", "runAsUser")) == 0:
        fail("spec.securityContext.runAsUser=0")
    for where, container in containers_of(pod_spec):
        if dig(container, ("securityContext", "runAsUser")) == 0:
            fail(f"spec.{where}.securityContext.runAsUser=0")


def check_containers(pod_spec, fail):
    for where, container in containers_of(pod_spec):
        security_context = container.get("securityContext") or {}
        prefix = f"spec.{where}.securityContext"

        if security_context.get("privileged"):
            fail(f"{prefix}.privileged=true")

        if security_context.get("allowPrivilegeEscalation") is not False:
            fail(
                f"{prefix}.allowPrivilegeEscalation must be explicitly false "
                "(restricted rejects unset)"
            )

        proc_mount = security_context.get("procMount")
        if proc_mount is not None and proc_mount != "Default":
            fail(f"{prefix}.procMount='{proc_mount}' must be Default")

        if dig(security_context, ("windowsOptions", "hostProcess")):
            fail(f"{prefix}.windowsOptions.hostProcess=true")

        check_selinux(prefix, security_context, fail)

        drop = {str(cap) for cap in dig(security_context, ("capabilities", "drop")) or []}
        if "ALL" not in drop:
            fail(f"{prefix}.capabilities.drop must contain ALL (got {sorted(drop)})")

        add = {str(cap) for cap in dig(security_context, ("capabilities", "add")) or []}
        disallowed = sorted(add - ALLOWED_ADDED_CAPABILITIES)
        if disallowed:
            fail(
                f"{prefix}.capabilities.add contains {disallowed}; restricted "
                f"allows only {sorted(ALLOWED_ADDED_CAPABILITIES)}"
            )

        for port in container.get("ports") or []:
            if port.get("hostPort"):
                fail(f"spec.{where}.ports hostPort={port['hostPort']} is forbidden")


def check_document(document, source, failures):
    kind = document.get("kind")
    path = POD_SPEC_PATHS.get(kind)
    if path is None:
        return 0

    name = dig(document, ("metadata", "name")) or "<unnamed>"
    pod_spec = dig(document, path)
    if not isinstance(pod_spec, dict):
        failures.append(f"{source}: {kind}/{name}: no pod spec at {'.'.join(path)}")
        return 1

    def fail(message):
        failures.append(f"{source}: {kind}/{name}: {message}")

    # Pod-template metadata carries the AppArmor annotations for pre-1.30
    # clusters; a bare Pod carries them on its own metadata.
    metadata_path = path[:-1] + ("metadata",) if len(path) > 1 else ("metadata",)
    check_apparmor(dig(document, metadata_path) or {}, fail)

    check_host_namespaces(pod_spec, fail)
    check_volumes(pod_spec, fail)
    check_sysctls(pod_spec, fail)
    check_selinux("spec.securityContext", pod_spec.get("securityContext") or {}, fail)
    check_seccomp(pod_spec, fail)
    check_run_as_non_root(pod_spec, fail)
    check_run_as_user(pod_spec, fail)
    check_containers(pod_spec, fail)
    return 1


def main(argv):
    paths = argv[1:]
    if not paths:
        sys.stderr.write(f"usage: {argv[0]} FILE [FILE...]  ('-' reads stdin)\n")
        return 2

    failures = []
    checked = 0

    for path in paths:
        if path == "-":
            source, text = "<stdin>", sys.stdin.read()
        else:
            source = path
            with open(path, "r", encoding="utf-8") as handle:
                text = handle.read()

        try:
            documents = list(yaml.safe_load_all(text))
        except yaml.YAMLError as error:
            failures.append(f"{source}: not parseable as YAML: {error}")
            continue

        for document in documents:
            if isinstance(document, dict):
                checked += check_document(document, source, failures)

    for failure in failures:
        print(f"FAIL {failure}")

    if failures:
        print(f"\n{len(failures)} PSS `restricted` violation(s) across {checked} object(s)")
        return 1

    if checked == 0:
        sys.stderr.write("ERROR: no pod-bearing objects found; nothing was asserted\n")
        return 1

    print(f"PASS PSS `restricted`: {checked} pod-bearing object(s) checked")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
