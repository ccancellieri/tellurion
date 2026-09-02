# Specification deviations and interpretations

Where this server knowingly does something other than what a clause of a
standard literally says, that fact is written down here, in one place, with
its reasoning — so that later readers can tell a deliberate, argued deviation
apart from a bug, and audit the decision instead of rediscovering it. Each
entry names the document, the clause it deviates from, the clause (if any)
that authorizes the deviation, what this server does, and why.

This register is documentation, not behaviour. It declares nothing to
clients: no conformance document, landing page, or API response references
it, and adding an entry here neither claims nor disclaims any conformance
class. A conformance list is maintained on its own evidence (see, e.g., the
per-driver CQL2 class accessors); this file records only the places where a
clause's letter and this server's behaviour part ways on purpose.

The bar for an entry in the first section is a genuine self-contradiction:
two clauses of the same document, in scope at the same time, requiring
incompatible behaviour, so that *no* implementation can honour both and any
choice deviates from one of them. A specification that is merely ambiguous,
or in tension with what a backend can honestly do, is not a deviation — those
decisions are recorded separately, in the second section, precisely so the
two categories cannot blur into an inflated count of "spec contradictions".

## Deviations

### OGC 20-058 (OGC API — Maps Part 1: Core): `bbox-crs` without `bbox` — Requirement 18 clause F vs §13.5

- **Where in code:** `crates/tellurion-tiles/src/maps.rs`, `parse_request`
  (the `bbox` match's `None` arm, and `parse_crs` being called before it).

**The conflicting clauses.** Requirement 18
(`/req/spatial-subsetting/bbox-crs`) clause F, verbatim:

> If the bbox parameter is not used, the bbox-crs SHALL be ignored.

§13.5 ("Error conditions"), verbatim, and stated unconditionally — it does
not except the case where the companion parameter is absent:

> If the CRS in the parameter value bbox-crs, subset-crs or center-crs is
> not supported by the server for this resource, or the parameter value is
> out-of-range, the status code of the response will be 400.

For a request carrying `bbox-crs` with an unsupported CRS value and no
`bbox`, these two clauses of the same document require incompatible
behaviour: clause F says the parameter shall be ignored (so the request
proceeds), §13.5 says the response will be 400. No implementation can do
both.

**What this server does.** The parameter's *effect* is ignored and its
*value* is still validated:

- A supported `bbox-crs` with no `bbox` changes nothing: the response is
  byte-for-byte the one the same request without the parameter produces —
  same status, same headers, same body — and the ignored parameter does not
  reach the render or fragment the cache key.
- An unsupported CRS in `bbox-crs` is refused by name (`400`,
  `CrsNotSupported`, naming the value refused and the CRSs this server does
  serve) whether or not `bbox` is present. This is the deviation from clause
  F's letter, taking §13.5's side where the two conflict.

**Why.** `bbox-crs` exists solely to declare the CRS in which `bbox` is
expressed; with no `bbox` there is nothing for it to qualify, so a supported
value has nothing to be wrong *about* and refusing it would fail a request
that cannot be incorrect — the client most likely to send an unused
`bbox-crs` is one filling a query-string template, and a hard failure there
carries no information it can act on. That is clause F's case, and it is
honoured. But ignoring an unused parameter is not the same as accepting a
nonsense one: a value naming a CRS this server could never serve is wrong on
its own terms, and waving it through because it happened to be unused today
is the silent-degradation shape this project refuses everywhere else — the
same request with a `bbox` added tomorrow would suddenly fail. So §13.5's
named refusal is kept for the value regardless of `bbox`'s presence,
deviating from clause F's unconditional "SHALL be ignored" in exactly that
case. There is no configuration flag: the ambiguity is resolved here, once,
in the open, not delegated to operators with less information.

Requirements 19 and 20 carry identical clause F wording for `subset-crs` and
`center-crs`; neither parameter exists on this server today, and if either
lands it takes this same rule, not a new decision.

## Interpretations

Decisions where a specification was ambiguous, or in tension with what a
backend can honestly do, and this server resolved the question by reading
the document — **without** the document contradicting itself, and without
deviating from any clause. Recorded here because the reasoning deserves one
findable home; distinguished from the section above because collapsing the
two categories is how "the specs keep contradicting themselves" gets
manufactured out of decisions that were actually ours to make.

### OGC 21-065r2 (CQL2): GeoPackage's positional `S_INTERSECTS` restriction vs declaring `basic-spatial-functions`

- **Where in code:** `crates/tellurion-geopackage/src/driver.rs`
  (`cql2_conformance_classes`, whose doc comment carries the full
  clause-by-clause chain), `sql::collect_intersects_check`, and
  `intersects_general_form_and_declared_class_agree` in
  `crates/tellurion-geopackage/tests/driver_contract.rs`.

**The question.** The GeoPackage driver compiles `S_INTERSECTS` only in a
restricted positional form — at most once per filter, and never beneath
`OR`/`NOT` — a soundness consequence of its R\*Tree
bbox-prune-then-exact-check strategy. Does declaring the
`basic-spatial-functions` conformance class tolerate that restriction, or
does the class promise the predicate in arbitrary boolean composition?

**The resolution.** Read against the published standard, the class promises
the general form: its dependency on Basic CQL2's Requirement 1 lifts
`spatialPredicate` into the full `booleanExpression` grammar, the normative
BNF puts no positional or count limit on where a predicate may sit, the
class's two narrowing permissions (Permissions 6 and 7) are about operands
rather than position, and the Abstract Test Suite (Conformance Tests 26 and
27) executes the two-predicate, `NOT` and `OR` shapes directly. So the
document is not ambiguous after all, and it is not contradicted: the class
is **withheld** for GeoPackage rather than declared. The restricted form
still works and is still answered exactly; the general form is still refused
by name, never silently approximated by the coarse bbox test. An honest
narrowing of an advertisement, not a deviation — no clause is
disobeyed, which is what keeps this entry out of the section above.
