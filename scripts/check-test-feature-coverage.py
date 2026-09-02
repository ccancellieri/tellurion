#!/usr/bin/env python3
"""Fails while any feature-gated `[[test]]` target is built by no CI invocation.

The bug this exists to prevent already happened. `cog_binary.rs`,
`zarr_binary.rs`, `duckdb_binary.rs` and `geopackage_binary.rs` -- 1459 lines
of driver acceptance test -- compiled to nothing in every CI job, because each
is gated twice:

    [[test]]
    name = "geopackage_binary"
    required-features = ["geopackage"]

    #![cfg(all(feature = "geopackage", not(feature = "postgis")))]

`required-features` alone cannot say "and this other feature OFF", so the file
carries an inner `#![cfg]` too -- and the only feature combination satisfying
both is one that no matrix leg named. `geopackage_binary.rs` shipped with an
assertion that could never pass and nothing went red, because nothing compiled
it. That is not a bug in a test; it is a hole in the pipeline, and one more
leg would only close today's instance of it.

So this checks the class: for every `[[test]]` target carrying
`required-features`, is there at least one `cargo test` invocation in
`.github/workflows/ci.yml` whose package scope and resolved feature set
actually build that file, inner `#![cfg]` included?

Deliberately conservative. Every construct it cannot resolve -- an unknown cfg
predicate, a feature that is not in the crate's own `[features]` table, a
`cargo test` command it cannot parse -- is reported as NOT covered rather than
assumed fine. A guard that guesses in the permissive direction is how the
original hole stayed open.

Usage: ./scripts/check-test-feature-coverage.py [--workflow PATH]
Exit 0 = every feature-gated test target is built somewhere in CI.
"""

import argparse
import glob
import os
import re
import shlex
import sys

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11
    print(
        "ERROR: this script needs Python 3.11+ for `tomllib` (no third-party TOML "
        "parser is assumed on a bare runner)",
        file=sys.stderr,
    )
    raise SystemExit(2)

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class Unresolvable(Exception):
    """Raised for anything this checker cannot evaluate with certainty."""


# --- the crate side ---------------------------------------------------------


def crate_manifests():
    return sorted(glob.glob(os.path.join(REPO_ROOT, "crates", "*", "Cargo.toml")))


def feature_table(manifest):
    with open(manifest, "rb") as handle:
        return tomllib.load(handle)


def close_features(names, features):
    """Every feature of THIS crate implied by `names`.

    Only bare feature names matter: `dep:foo` activates an optional
    dependency and `pkg/feat` a dependency's feature, and neither makes
    `cfg(feature = ...)` true in this crate, which is all the inner `#![cfg]`
    predicates below can test.
    """
    active, queue = set(), list(names)
    while queue:
        name = queue.pop()
        if name in active:
            continue
        active.add(name)
        for implied in features.get(name, []):
            if ":" in implied or "/" in implied:
                continue
            queue.append(implied)
    return active


CFG_RE = re.compile(r"^#!\[cfg\((.*)\)\]\s*$")
FEATURE_RE = re.compile(r'^feature\s*=\s*"([^"]+)"$')


def split_top_level(text):
    parts, depth, current = [], 0, ""
    for char in text:
        if char == "," and depth == 0:
            parts.append(current.strip())
            current = ""
            continue
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        current += char
    if current.strip():
        parts.append(current.strip())
    return parts


def eval_cfg(expr, active):
    """Evaluate a `#![cfg(...)]` predicate against an active feature set.

    Raises `Unresolvable` for any predicate shape not handled -- `target_os`,
    `test`, a bare identifier -- so an unrecognised gate can never be silently
    treated as satisfied.
    """
    expr = expr.strip()
    match = FEATURE_RE.match(expr)
    if match:
        return match.group(1) in active
    for name, combine in (("all", all), ("any", any)):
        prefix = name + "("
        if expr.startswith(prefix) and expr.endswith(")"):
            inner = expr[len(prefix) : -1]
            return combine(eval_cfg(part, active) for part in split_top_level(inner))
    if expr.startswith("not(") and expr.endswith(")"):
        return not eval_cfg(expr[4:-1], active)
    raise Unresolvable(f"unsupported cfg predicate: {expr}")


def inner_cfg(path):
    if not os.path.exists(path):
        raise Unresolvable(f"test file not found: {path}")
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith("//"):
                continue
            match = CFG_RE.match(line)
            if match:
                return match.group(1)
            if line.startswith("#!["):
                continue
            return None
    return None


def gated_test_targets():
    """Every `[[test]]` target in the workspace that carries required-features."""
    targets = []
    for manifest in crate_manifests():
        data = feature_table(manifest)
        package = data.get("package", {}).get("name")
        if not package:
            continue
        crate_dir = os.path.dirname(manifest)
        for entry in data.get("test", []):
            required = entry.get("required-features")
            if not required:
                continue
            name = entry["name"]
            targets.append(
                {
                    "package": package,
                    "name": name,
                    "required": set(required),
                    "features": data.get("features", {}),
                    "path": os.path.join(crate_dir, "tests", f"{name}.rs"),
                }
            )
    return targets


# --- the workflow side ------------------------------------------------------

MATRIX_FLAGS_RE = re.compile(r"^\s*flags:\s*(.+?)\s*$", re.MULTILINE)
# Stops at a newline, a pipe, `&`, or a redirection, so a command that tees or
# redirects contributes only its own arguments.
CARGO_TEST_RE = re.compile(r"cargo test\b([^\n|&>]*)")


def workflow_invocations(workflow_path):
    """Every `cargo test` invocation ci.yml runs, matrix flags expanded.

    Read as text rather than parsed YAML on purpose: the command lives in a
    `run:` scalar either way, and this keeps the checker's only dependency the
    standard library, so it runs on a bare runner with no `pip install` step.
    """
    with open(workflow_path, encoding="utf-8") as handle:
        text = handle.read()

    flag_sets = [m.group(1) for m in MATRIX_FLAGS_RE.finditer(text)]
    invocations = []
    for match in CARGO_TEST_RE.finditer(text):
        command = match.group(1)
        if "${{ matrix.flags }}" in command:
            for flags in flag_sets:
                invocations.append(command.replace("${{ matrix.flags }}", flags))
        elif "${{" in command:
            raise Unresolvable(f"cargo test command has unexpanded template: {command}")
        else:
            invocations.append(command)
    if not invocations:
        raise Unresolvable("no cargo test invocation found in the workflow")
    return invocations


def parse_invocation(command):
    """(packages | None for --workspace, no_default, all_features, features)."""
    tokens = shlex.split(command)
    packages, features = [], set()
    workspace = no_default = all_features = False
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token in ("-p", "--package"):
            index += 1
            packages.append(tokens[index])
        elif token == "--workspace":
            workspace = True
        elif token == "--no-default-features":
            no_default = True
        elif token == "--all-features":
            all_features = True
        elif token == "--features":
            index += 1
            features.update(re.split(r"[,\s]+", tokens[index]))
        elif token.startswith("--features="):
            features.update(re.split(r"[,\s]+", token.split("=", 1)[1]))
        index += 1
    return (None if workspace else packages), no_default, all_features, features


def active_features(target, invocation):
    packages, no_default, all_features, requested = parse_invocation(invocation)
    if packages is not None and target["package"] not in packages:
        return None
    declared = target["features"]
    if all_features:
        return set(declared)
    seeds = set(requested)
    if not no_default:
        seeds.update(declared.get("default", []))
    unknown = {f for f in requested if f not in declared}
    if unknown:
        # A leg naming a feature this crate does not declare cannot be
        # reasoned about; fail closed rather than treat it as inert.
        raise Unresolvable(
            f"{target['package']} has no feature(s) {sorted(unknown)} named by: {invocation.strip()}"
        )
    return close_features(seeds, declared)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--workflow", default=os.path.join(REPO_ROOT, ".github/workflows/ci.yml")
    )
    args = parser.parse_args(argv[1:])

    try:
        invocations = workflow_invocations(args.workflow)
    except (Unresolvable, OSError) as err:
        print(f"FAIL: {err}", file=sys.stderr)
        return 1

    targets = gated_test_targets()
    if not targets:
        print("FAIL: no feature-gated [[test]] targets found -- did the layout move?")
        return 1

    failures = 0
    print(f"{'TEST TARGET':<44} {'BUILT BY':<10} HOW")
    for target in sorted(targets, key=lambda t: (t["package"], t["name"])):
        label = f"{target['package']}/{target['name']}"
        try:
            cfg = inner_cfg(target["path"])
        except Unresolvable as err:
            print(f"{label:<44} {'NO':<10} {err}")
            failures += 1
            continue

        covered_by, problems = None, []
        for invocation in invocations:
            try:
                active = active_features(target, invocation)
            except Unresolvable as err:
                problems.append(str(err))
                continue
            if active is None:
                continue
            if not target["required"].issubset(active):
                continue
            if cfg is not None:
                try:
                    if not eval_cfg(cfg, active):
                        continue
                except Unresolvable as err:
                    problems.append(str(err))
                    continue
            covered_by = invocation.strip()
            break

        if covered_by:
            print(f"{label:<44} {'yes':<10} cargo test {covered_by}")
        else:
            required = ", ".join(sorted(target["required"]))
            detail = f"required-features [{required}]"
            if cfg:
                detail += f" + #![cfg({cfg})]"
            print(f"{label:<44} {'NO':<10} {detail}")
            print(
                f"{'':<44} {'':<10} no ci.yml cargo test invocation builds this file",
                file=sys.stderr,
            )
            for problem in problems:
                print(f"{'':<44} {'':<10} note: {problem}", file=sys.stderr)
            failures += 1

    if failures:
        print(
            f"\nFAIL: {failures} feature-gated test target(s) are built by no CI "
            "invocation, so nothing in them can ever go red",
            file=sys.stderr,
        )
        return 1
    print("\nevery feature-gated test target is built by at least one CI invocation")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
