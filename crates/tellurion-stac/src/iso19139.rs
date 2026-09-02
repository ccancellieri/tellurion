//! Pure `CanonicalDescriptor` -> ISO 19115 (19139 XML encoding) mapping
//! (`#50`, second half): the same "no I/O, no link-building" split
//! `mapping.rs` keeps for the STAC Collection projection, reading ONLY
//! `CanonicalDescriptor` — never a raw `CollectionDescriptor`/`StacConf`
//! directly — so this projection and the STAC one can never quietly read
//! two different truths about the same collection.
//!
//! Hand-rolled XML, not a library: this workspace has no XML crate anywhere
//! in its dependency tree (checked `Cargo.lock` before writing this), and
//! the document shape below is fixed and shallow enough that a dependency
//! for it would cost more than it saves. Every text node runs through
//! [`escape_xml`]; there is no other way text reaches the output.
//!
//! `application/vnd.iso.19139+xml` is the media type this projection
//! serves under (also `ISO19139_MEDIA_TYPE`) — the de facto value in
//! widespread use for ISO 19139 XML records (pycsw, GeoNetwork, OGC API -
//! Records catalogs); not independently re-verified against a live IANA/OGC
//! registry entry this session.
//!
//! # What CanonicalDescriptor cannot provide (explicit omissions)
//!
//! ISO 19115's `MD_Metadata/contact` is schema-mandatory (`1..*`). As of
//! `#187`'s first slice there *is* a contacts model — `config::ContactDecl`,
//! reaching this projection as `CanonicalStac::contacts` — so a declared
//! contact is emitted as a real `gmd:CI_ResponsibleParty`. It stays
//! operator-opt-in: a collection whose settings chain declares no contact
//! at all still gets `gmd:contact` with `gco:nilReason="unknown"`, ISO's
//! own mechanism for "this element is required but the value is not known
//! here", byte-for-byte as before `#187`. Nothing is ever fabricated to
//! fill the mandatory slot. `dataQualityInfo`/lineage is *optional*
//! (`0..*`) in the schema, so with no declared lineage it is simply never
//! emitted — no nilReason needed for an element the schema doesn't require
//! in the first place, and no boilerplate ("data quality unknown") standing
//! in for a fact nobody asserted: a collection that gains no lineage keeps
//! its document byte-for-byte. A declared `stac.lineage`
//! (`tellurion_core::LineageDecl`, `#50`'s lineage slice — the decision
//! `#187` deferred) fills it for real: this workspace records no
//! collection-level provenance fact of its own anywhere the server can read
//! back at request time (see `LineageDecl`'s own doc for the surveyed-and-
//! rejected candidates: the `#191` harvester's CLI-side bookmark/report,
//! the `#202` per-item sidecar, the secret-shaped `url_env` file paths), so
//! the operator's declaration is the only honest source, exactly as it is
//! for contacts. Same optional-vs-mandatory split governs every other
//! absent fact below; each call is made once, at the call site, with its
//! own comment.
//!
//! Per-contact, the same rule applies field by field: `name` is the only
//! thing `ContactDecl` requires, and every optional field absent from the
//! declaration omits its XML element rather than emitting an empty
//! `gco:CharacterString`. `role` is the one exception in shape only —
//! `CI_ResponsibleParty/role` is itself mandatory (`1`), so an undeclared
//! role falls back to the `pointOfContact` codelist value, the only
//! defensible reading of "an operator listed this party as the contact for
//! this dataset and said nothing further."
//!
//! # Provenance
//!
//! `CanonicalDescriptor` tracks [`tellurion_core::Provenance`] only on the
//! physical identity fields (`table`/`geometry`/`pk`/`datetime`) and schema
//! properties — none of which ISO 19115's own vocabulary has a slot for
//! (title/abstract/extent/CRS/constraints/keywords are either plain facts
//! with no override concept in `CanonicalDescriptor` itself, like `srid`
//! and `extent`, or the whole `stac` group, which is `Declared` by
//! construction whenever present at all). To still exercise this
//! projection against a real provenance-bearing fact, `table`'s value
//! (when present) is carried through as a secondary `citation/identifier`
//! (a legitimate ISO home for "this resource's underlying physical
//! identifier") — see the `override_and_derived_table_provenance_render_
//! identically` test: an `Override` and a `Derived` `table` field with the
//! same value produce byte-identical XML, because ISO 19139 has nowhere to
//! put that distinction. This is documented here rather than worked around
//! with an invented extension element.

use std::time::{SystemTime, UNIX_EPOCH};

use tellurion_core::{CanonicalDescriptor, ContactDecl, LineageDecl};

/// Media type this crate serves an ISO 19139 XML representation of a
/// collection under — see this module's own doc for the "de facto, not
/// independently re-verified" caveat.
pub const ISO19139_MEDIA_TYPE: &str = "application/vnd.iso.19139+xml";

/// Fixed metadata/resource language for every record this projection
/// produces — a system-wide constant (this deployment only ever describes
/// its collections in English), not a per-collection fact `CanonicalDescriptor`
/// could ever carry; same category as `mapping::STAC_VERSION` being a fixed
/// value rather than something read off the descriptor.
const LANGUAGE: &str = "eng";

/// `MD_ScopeCode` value every record declares: every collection this server
/// describes is a dataset, never a series/service/collection-of-datasets —
/// fixed, not derived.
const HIERARCHY_LEVEL: &str = "dataset";

/// The codelist URI convention widely used by ISO 19139 producers (pycsw,
/// GeoNetwork) for `MD_ScopeCode` and friends. Fixed reference text, not a
/// per-collection fact.
const SCOPE_CODE_LIST: &str =
    "http://standards.iso.org/ittf/PubliclyAvailableStandards/ISO_19139_Schemas/resources/Codelist/gmxCodelists.xml#MD_ScopeCode";

/// The same codelist convention as [`SCOPE_CODE_LIST`], for
/// `CI_ResponsibleParty/role`'s own `CI_RoleCode` (`#187`).
const ROLE_CODE_LIST: &str =
    "http://standards.iso.org/ittf/PubliclyAvailableStandards/ISO_19139_Schemas/resources/Codelist/gmxCodelists.xml#CI_RoleCode";

/// `CI_RoleCode` value used for a declared contact that names no `role` of
/// its own. `CI_ResponsibleParty/role` is schema-mandatory (`1`), so unlike
/// every *optional* contact field this one cannot simply be omitted — see
/// this module's own doc for why `pointOfContact` is the honest default
/// rather than a nilReason.
const DEFAULT_CONTACT_ROLE: &str = "pointOfContact";

/// Escapes the five XML predefined entities. The only path any text node in
/// this module's output reaches the wire through.
fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// `<{tag}><gco:CharacterString>{value}</gco:CharacterString></{tag}>`,
/// indented and newline-terminated — the shape every free-text ISO 19139
/// property below takes.
fn character_string_element(indent: &str, tag: &str, value: &str) -> String {
    format!(
        "{indent}<{tag}><gco:CharacterString>{}</gco:CharacterString></{tag}>\n",
        escape_xml(value)
    )
}

/// `<{tag} gco:nilReason="{reason}"/>` — ISO 19139's own mechanism for a
/// schema-mandatory element with no known value, per this module's own doc.
fn nil_element(indent: &str, tag: &str, reason: &str) -> String {
    format!("{indent}<{tag} gco:nilReason=\"{reason}\"/>\n")
}

/// One declared [`ContactDecl`] as a `gmd:contact/gmd:CI_ResponsibleParty`
/// (`#187`, first slice), in the element order ISO 19139's own schema
/// fixes: `individualName`, `organisationName`, `contactInfo`, `role`.
/// Every optional field absent from the declaration omits its element
/// entirely; `role` alone falls back to [`DEFAULT_CONTACT_ROLE`] because
/// the schema makes it mandatory. `contactInfo` itself is emitted only when
/// there is something to put in it (an email, a URL, or both) — an empty
/// `CI_Contact` would be structural noise, not information.
fn contact_element(indent: &str, contact: &ContactDecl) -> String {
    let party = format!("{indent}  ");
    let field = format!("{indent}    ");
    let mut xml = format!("{indent}<gmd:contact>\n{party}<gmd:CI_ResponsibleParty>\n");
    xml.push_str(&character_string_element(
        &field,
        "gmd:individualName",
        &contact.name,
    ));
    if let Some(organization) = &contact.organization {
        xml.push_str(&character_string_element(
            &field,
            "gmd:organisationName",
            organization,
        ));
    }
    if contact.email.is_some() || contact.url.is_some() {
        xml.push_str(&format!(
            "{field}<gmd:contactInfo>\n{field}  <gmd:CI_Contact>\n"
        ));
        if let Some(email) = &contact.email {
            xml.push_str(&format!(
                "{field}    <gmd:address>\n{field}      <gmd:CI_Address>\n"
            ));
            xml.push_str(&character_string_element(
                &format!("{field}        "),
                "gmd:electronicMailAddress",
                email,
            ));
            xml.push_str(&format!(
                "{field}      </gmd:CI_Address>\n{field}    </gmd:address>\n"
            ));
        }
        if let Some(url) = &contact.url {
            xml.push_str(&format!(
                "{field}    <gmd:onlineResource>\n{field}      <gmd:CI_OnlineResource>\n"
            ));
            xml.push_str(&format!(
                "{field}        <gmd:linkage><gmd:URL>{}</gmd:URL></gmd:linkage>\n",
                escape_xml(url)
            ));
            xml.push_str(&format!(
                "{field}      </gmd:CI_OnlineResource>\n{field}    </gmd:onlineResource>\n"
            ));
        }
        xml.push_str(&format!(
            "{field}  </gmd:CI_Contact>\n{field}</gmd:contactInfo>\n"
        ));
    }
    let role = contact.role.as_deref().unwrap_or(DEFAULT_CONTACT_ROLE);
    let role = escape_xml(role);
    xml.push_str(&format!(
        "{field}<gmd:role><gmd:CI_RoleCode codeList=\"{ROLE_CODE_LIST}\" codeListValue=\"{role}\">{role}</gmd:CI_RoleCode></gmd:role>\n"
    ));
    xml.push_str(&format!(
        "{party}</gmd:CI_ResponsibleParty>\n{indent}</gmd:contact>\n"
    ));
    xml
}

/// One declared [`LineageDecl`] as the complete
/// `gmd:dataQualityInfo/gmd:DQ_DataQuality` element (`#50`, lineage slice),
/// in the nesting ISO 19139's own schema fixes: `DQ_DataQuality` requires a
/// `scope` (`1`), whose `DQ_Scope/level` reuses [`HIERARCHY_LEVEL`] — the
/// same every-collection-is-a-dataset fact `gmd:hierarchyLevel` already
/// states, not a new assertion — and `lineage/LI_Lineage` then carries only
/// what the operator declared, in the schema's own member order:
/// `statement`, then every `processStep`, then every `source`. Each
/// `LI_Source`/`LI_ProcessStep` carries exactly its declared `description`
/// (required in the config model — see `LineageSourceDecl`/
/// `LineageProcessStepDecl`), so no element here ever has a mandatory child
/// this projection cannot fill with a real fact.
///
/// Only ever called with a non-empty declaration — `to_iso19139` filters
/// the empty shape defensively even though `StacConf::validate` already
/// refuses it by name at config load, so no path can emit an empty
/// `gmd:LI_Lineage`.
fn data_quality_element(indent: &str, lineage: &LineageDecl) -> String {
    let dq = format!("{indent}  ");
    let member = format!("{indent}    ");
    let mut xml = format!("{indent}<gmd:dataQualityInfo>\n{dq}<gmd:DQ_DataQuality>\n");
    xml.push_str(&format!(
        "{member}<gmd:scope>\n\
         {member}  <gmd:DQ_Scope>\n\
         {member}    <gmd:level><gmd:MD_ScopeCode codeList=\"{SCOPE_CODE_LIST}\" codeListValue=\"{HIERARCHY_LEVEL}\">{HIERARCHY_LEVEL}</gmd:MD_ScopeCode></gmd:level>\n\
         {member}  </gmd:DQ_Scope>\n\
         {member}</gmd:scope>\n"
    ));
    let li = format!("{member}  ");
    let fact = format!("{member}    ");
    xml.push_str(&format!("{member}<gmd:lineage>\n{li}<gmd:LI_Lineage>\n"));
    if let Some(statement) = &lineage.statement {
        xml.push_str(&character_string_element(&fact, "gmd:statement", statement));
    }
    for step in &lineage.process_steps {
        xml.push_str(&format!(
            "{fact}<gmd:processStep>\n{fact}  <gmd:LI_ProcessStep>\n"
        ));
        xml.push_str(&character_string_element(
            &format!("{fact}    "),
            "gmd:description",
            &step.description,
        ));
        xml.push_str(&format!(
            "{fact}  </gmd:LI_ProcessStep>\n{fact}</gmd:processStep>\n"
        ));
    }
    for source in &lineage.sources {
        xml.push_str(&format!("{fact}<gmd:source>\n{fact}  <gmd:LI_Source>\n"));
        xml.push_str(&character_string_element(
            &format!("{fact}    "),
            "gmd:description",
            &source.description,
        ));
        xml.push_str(&format!("{fact}  </gmd:LI_Source>\n{fact}</gmd:source>\n"));
    }
    xml.push_str(&format!("{li}</gmd:LI_Lineage>\n{member}</gmd:lineage>\n"));
    xml.push_str(&format!(
        "{dq}</gmd:DQ_DataQuality>\n{indent}</gmd:dataQualityInfo>\n"
    ));
    xml
}

/// Days-since-Unix-epoch -> proleptic Gregorian `(year, month, day)`.
/// Howard Hinnant's `civil_from_days` algorithm: small, exact, and avoids
/// pulling in a date/calendar crate (`chrono`/`time` are transitive-only in
/// this workspace, not a direct dependency of anything) just to stamp a
/// metadata generation date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m: u32 = if mp < 10 {
        (mp + 3) as u32
    } else {
        (mp - 9) as u32
    };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Today's date (UTC), `YYYY-MM-DD` — the ISO 19139 `dateStamp`'s own
/// meaning is "when this metadata record was produced," which genuinely is
/// today, not a fact this projection has to read off `CanonicalDescriptor`
/// at all (there is no dataset-level "metadata date" anywhere in this
/// workspace's model).
fn date_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let days = (now.as_secs() / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Maps `canonical` (this collection's merged `CanonicalDescriptor`, or
/// `None` when resolution failed outright — same tolerant contract
/// `mapping::to_stac_collection` documents for itself) into a complete ISO
/// 19139 XML document. `external_id` is this collection's public id
/// (`#39`) — never the internal id, same rule every protocol crate follows.
///
/// Absent facts are never fabricated: a `None` field either omits its whole
/// (optional) XML element, or — for a schema-mandatory element with no
/// source (the citation `date` always; `contact` when this collection's
/// settings chain declares none, `#187`) — `gco:nilReason="unknown"`. See
/// this module's own doc for the full list.
pub fn to_iso19139(canonical: Option<&CanonicalDescriptor>, external_id: &str) -> String {
    let stac = canonical.and_then(|c| c.stac.as_ref());
    let keywords: &[String] = stac.map(|s| s.keywords.as_slice()).unwrap_or(&[]);
    let license = stac.and_then(|s| s.license.as_deref());
    let contacts: &[ContactDecl] = stac.map(|s| s.contacts.as_slice()).unwrap_or(&[]);
    // Defensive `is_empty` filter on top of `StacConf::validate`'s own named
    // load-time refusal of the empty shape: no path — not even a descriptor
    // built outside a validated config — may emit an empty `gmd:LI_Lineage`.
    let lineage = stac
        .and_then(|s| s.lineage.as_ref())
        .filter(|lineage| !lineage.is_empty());
    let table = canonical
        .and_then(|c| c.table.as_ref())
        .map(|f| f.value.as_str());
    let extent = canonical.and_then(|c| c.extent);
    let has_temporal_dimension = canonical.and_then(|c| c.datetime.as_ref()).is_some();
    let srid = canonical.and_then(|c| c.srid);

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(
        "<gmd:MD_Metadata xmlns:gmd=\"http://www.isotc211.org/2005/gmd\" \
         xmlns:gco=\"http://www.isotc211.org/2005/gco\" \
         xmlns:gml=\"http://www.opengis.net/gml/3.2\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n",
    );

    xml.push_str(&character_string_element(
        "  ",
        "gmd:fileIdentifier",
        external_id,
    ));
    xml.push_str(&character_string_element("  ", "gmd:language", LANGUAGE));
    xml.push_str(&format!(
        "  <gmd:hierarchyLevel><gmd:MD_ScopeCode codeList=\"{SCOPE_CODE_LIST}\" codeListValue=\"{HIERARCHY_LEVEL}\">{HIERARCHY_LEVEL}</gmd:MD_ScopeCode></gmd:hierarchyLevel>\n"
    ));
    // Schema-mandatory (`1..*`). A declared contact (`#187`) fills it for
    // real; with none declared the value genuinely is not known here, so
    // this keeps emitting nilReason rather than inventing a party — which
    // also means an unconfigured deployment's XML is unchanged.
    if contacts.is_empty() {
        xml.push_str(&nil_element("  ", "gmd:contact", "unknown"));
    } else {
        for contact in contacts {
            xml.push_str(&contact_element("  ", contact));
        }
    }
    xml.push_str(&format!(
        "  <gmd:dateStamp><gco:Date>{}</gco:Date></gmd:dateStamp>\n",
        date_stamp()
    ));

    xml.push_str("  <gmd:identificationInfo>\n    <gmd:MD_DataIdentification>\n");
    xml.push_str("      <gmd:citation>\n        <gmd:CI_Citation>\n");
    xml.push_str(&character_string_element(
        "          ",
        "gmd:title",
        external_id,
    ));
    // `CI_Citation/date` is schema-mandatory (`1..*`); this workspace has no
    // citation date for a collection (not the same thing as `dateStamp`
    // above, which is about the metadata record, not the dataset) — nilReason.
    xml.push_str(&nil_element("          ", "gmd:date", "unknown"));
    // Secondary identifier, optional (`0..*`): the collection's physical
    // table, when derivation succeeded. See this module's own doc for why
    // this is here (the provenance-collapse test needs a real
    // provenance-bearing fact to exercise).
    if let Some(table) = table {
        xml.push_str("          <gmd:identifier>\n            <gmd:MD_Identifier>\n");
        xml.push_str(&character_string_element(
            "              ",
            "gmd:code",
            table,
        ));
        xml.push_str("            </gmd:MD_Identifier>\n          </gmd:identifier>\n");
    }
    xml.push_str("        </gmd:CI_Citation>\n      </gmd:citation>\n");
    // `abstract` is schema-mandatory (`1`); `CanonicalDescriptor` has no
    // free-text description field at all (same gap `mapping::
    // to_stac_collection` fills with its own generic per-id sentence for
    // STAC's equally-mandatory `description`) — this mirrors that exact
    // convention rather than inventing a second one.
    xml.push_str(&character_string_element(
        "      ",
        "gmd:abstract",
        &format!("Dataset '{external_id}'."),
    ));
    xml.push_str(&character_string_element(
        "      ",
        "gmd:language",
        LANGUAGE,
    ));

    if !keywords.is_empty() {
        xml.push_str("      <gmd:descriptiveKeywords>\n        <gmd:MD_Keywords>\n");
        for keyword in keywords {
            xml.push_str(&character_string_element(
                "          ",
                "gmd:keyword",
                keyword,
            ));
        }
        xml.push_str("        </gmd:MD_Keywords>\n      </gmd:descriptiveKeywords>\n");
    }

    // `resourceConstraints` is optional (`0..*`); unlike STAC's Collection
    // `license` (spec-required, so `mapping::to_stac_collection` falls back
    // to the placeholder `"other"`), ISO has no such requirement — an
    // unconfigured license is a clean omission here, not a fabricated
    // placeholder value.
    if let Some(license) = license {
        xml.push_str("      <gmd:resourceConstraints>\n        <gmd:MD_LegalConstraints>\n");
        xml.push_str(&character_string_element(
            "          ",
            "gmd:useLimitation",
            license,
        ));
        xml.push_str("        </gmd:MD_LegalConstraints>\n      </gmd:resourceConstraints>\n");
    }

    // `extent` is optional (`0..*`) — no whole-Earth fallback the way STAC's
    // spec-required `extent.spatial.bbox` needs one; a genuinely unknown
    // extent is simply omitted. Temporal: a configured datetime column means
    // this collection DOES have a temporal dimension, but `CanonicalDescriptor`
    // only ever carries the column *name*, never actual min/max values
    // (`mapping::to_stac_collection`'s own doc makes the identical point for
    // STAC's temporal extent) — unlike STAC's `[null, null]`, ISO's
    // `gml:TimePeriod` has no open-interval idiom to reuse, so this emits
    // `gco:nilReason="unknown"` on the one property element that would hold
    // it: "a temporal extent exists, its bounds are not known here," not "no
    // temporal dimension at all" (that case omits `temporalElement` entirely).
    if extent.is_some() || has_temporal_dimension {
        xml.push_str("      <gmd:extent>\n        <gmd:EX_Extent>\n");
        if let Some(extent) = extent {
            xml.push_str(
                "          <gmd:geographicElement>\n            <gmd:EX_GeographicBoundingBox>\n",
            );
            xml.push_str(&format!(
                "              <gmd:westBoundLongitude><gco:Decimal>{}</gco:Decimal></gmd:westBoundLongitude>\n",
                extent.bbox[0]
            ));
            xml.push_str(&format!(
                "              <gmd:eastBoundLongitude><gco:Decimal>{}</gco:Decimal></gmd:eastBoundLongitude>\n",
                extent.bbox[2]
            ));
            xml.push_str(&format!(
                "              <gmd:southBoundLatitude><gco:Decimal>{}</gco:Decimal></gmd:southBoundLatitude>\n",
                extent.bbox[1]
            ));
            xml.push_str(&format!(
                "              <gmd:northBoundLatitude><gco:Decimal>{}</gco:Decimal></gmd:northBoundLatitude>\n",
                extent.bbox[3]
            ));
            xml.push_str(
                "            </gmd:EX_GeographicBoundingBox>\n          </gmd:geographicElement>\n",
            );
        }
        if has_temporal_dimension {
            xml.push_str("          <gmd:temporalElement>\n            <gmd:EX_TemporalExtent>\n");
            xml.push_str(&nil_element("              ", "gmd:extent", "unknown"));
            xml.push_str(
                "            </gmd:EX_TemporalExtent>\n          </gmd:temporalElement>\n",
            );
        }
        xml.push_str("        </gmd:EX_Extent>\n      </gmd:extent>\n");
    }

    xml.push_str("    </gmd:MD_DataIdentification>\n  </gmd:identificationInfo>\n");

    // `referenceSystemInfo` is optional (`0..*`) — omitted when the backend
    // never reported a storage SRID at all.
    if let Some(srid) = srid {
        xml.push_str(
            "  <gmd:referenceSystemInfo>\n    <gmd:MD_ReferenceSystem>\n      <gmd:referenceSystemIdentifier>\n        <gmd:RS_Identifier>\n",
        );
        xml.push_str(&character_string_element(
            "          ",
            "gmd:codeSpace",
            "EPSG",
        ));
        xml.push_str(&character_string_element(
            "          ",
            "gmd:code",
            &srid.to_string(),
        ));
        xml.push_str(
            "        </gmd:RS_Identifier>\n      </gmd:referenceSystemIdentifier>\n    </gmd:MD_ReferenceSystem>\n  </gmd:referenceSystemInfo>\n",
        );
    }

    // `dataQualityInfo` is optional (`0..*`): emitted only when this
    // collection's settings chain declares a lineage (`#50`, lineage slice)
    // — never an empty element, never boilerplate prose, so a collection
    // with no declared lineage keeps its document byte-for-byte. See
    // `data_quality_element` for the fixed `DQ_DataQuality > scope +
    // lineage > LI_Lineage` nesting.
    if let Some(lineage) = lineage {
        xml.push_str(&data_quality_element("  ", lineage));
    }

    xml.push_str("</gmd:MD_Metadata>\n");
    xml
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use tellurion_core::{
        CanonicalCapabilities, CanonicalField, CanonicalStac, ContactDecl, Provenance,
        SpatialExtent,
    };

    /// An otherwise-empty declared metadata block, so a contacts-focused
    /// test can name only the field it actually exercises.
    fn stac_block() -> CanonicalStac {
        CanonicalStac {
            license: None,
            keywords: vec![],
            providers: vec![],
            assets: BTreeMap::new(),
            contacts: vec![],
            lineage: None,
        }
    }

    /// A minimal, stack-based well-formedness check: every opening tag has a
    /// matching closing tag in the right order, self-closing tags never push
    /// onto the stack, and the stack is empty at the end. Deliberately not a
    /// real XML parser (no external crate for it — see this module's own
    /// doc on why no XML dependency exists in this workspace) — just enough
    /// to catch a mismatched or unclosed tag, which is what "well-formed"
    /// means for this test's purposes.
    fn assert_well_formed(xml: &str) {
        let body = xml
            .strip_prefix("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
            .expect("document must start with the XML declaration");
        let mut stack: Vec<String> = Vec::new();
        let mut chars = body.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            if ch != '<' {
                continue;
            }
            let end = body[i..]
                .find('>')
                .map(|offset| i + offset)
                .expect("unterminated tag");
            let tag = &body[i + 1..end];
            if let Some(name) = tag.strip_prefix('/') {
                let expected = stack.pop().expect("closing tag with no matching opener");
                assert_eq!(expected, name, "mismatched close tag");
            } else if let Some(name) = tag.strip_suffix('/') {
                let name = name.split_whitespace().next().unwrap_or(name);
                let _ = name; // self-closing: never pushed, nothing to verify further
            } else {
                let name = tag.split_whitespace().next().unwrap_or(tag);
                stack.push(name.to_string());
            }
            // Skip consumed chars.
            while let Some(&(j, _)) = chars.peek() {
                if j <= end {
                    chars.next();
                } else {
                    break;
                }
            }
        }
        assert!(stack.is_empty(), "unclosed tags remain: {stack:?}");
    }

    fn complete_canonical() -> CanonicalDescriptor {
        CanonicalDescriptor {
            kind: tellurion_core::CollectionKind::Vector,
            table: Some(CanonicalField {
                value: "physical_demo".to_string(),
                provenance: Provenance::Derived,
            }),
            geometry: Some(CanonicalField {
                value: "geom".to_string(),
                provenance: Provenance::Derived,
            }),
            pk: Some(CanonicalField {
                value: "id".to_string(),
                provenance: Provenance::Derived,
            }),
            datetime: Some(CanonicalField {
                value: "observed_at".to_string(),
                provenance: Provenance::Derived,
            }),
            srid: Some(4326),
            projection: None,
            extent: Some(SpatialExtent {
                bbox: [-5.0, 45.0, 5.0, 55.0],
            }),
            row_estimate: Some(1000),
            schema: None,
            stac: Some(CanonicalStac {
                license: Some("CC-BY-4.0".to_string()),
                keywords: vec!["imagery".to_string(), "satellite".to_string()],
                providers: vec![],
                assets: BTreeMap::new(),
                contacts: vec![],
                lineage: None,
            }),
            capabilities: CanonicalCapabilities::default(),
            geometry_profile: None,
        }
    }

    #[test]
    fn no_canonical_at_all_still_produces_a_well_formed_minimal_document() {
        let xml = to_iso19139(None, "demo");
        assert_well_formed(&xml);
        assert!(xml.contains("<gmd:MD_Metadata"));
        assert!(xml.contains(">demo<"));
    }

    #[test]
    fn complete_descriptor_is_well_formed_and_includes_every_available_fact() {
        let canonical = complete_canonical();
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        assert!(xml.contains("<gco:CharacterString>demo</gco:CharacterString>"));
        assert!(xml.contains("<gco:CharacterString>imagery</gco:CharacterString>"));
        assert!(xml.contains("<gco:CharacterString>satellite</gco:CharacterString>"));
        assert!(xml.contains("<gco:CharacterString>CC-BY-4.0</gco:CharacterString>"));
        assert!(xml.contains("<gco:CharacterString>physical_demo</gco:CharacterString>"));
        assert!(xml.contains("<gco:Decimal>-5</gco:Decimal>"));
        assert!(xml.contains("<gco:Decimal>55</gco:Decimal>"));
        assert!(xml.contains("<gco:CharacterString>4326</gco:CharacterString>"));
        assert!(xml.contains("<gco:CharacterString>EPSG</gco:CharacterString>"));
        // Temporal dimension known (datetime column configured) but bounds
        // unknown: nilReason, not an omission and not a fabricated interval.
        assert!(xml.contains("<gmd:extent gco:nilReason=\"unknown\"/>"));
    }

    #[test]
    fn partial_descriptor_omits_every_absent_fact() {
        let canonical = CanonicalDescriptor {
            kind: tellurion_core::CollectionKind::Vector,
            table: None,
            geometry: None,
            pk: None,
            datetime: None,
            srid: None,
            projection: None,
            extent: None,
            row_estimate: None,
            schema: None,
            stac: None,
            capabilities: CanonicalCapabilities::default(),
            geometry_profile: None,
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        assert!(!xml.contains("gmd:descriptiveKeywords"));
        assert!(!xml.contains("gmd:resourceConstraints"));
        assert!(!xml.contains("gmd:extent"));
        assert!(!xml.contains("gmd:referenceSystemInfo"));
        assert!(!xml.contains("gmd:identifier"));
        // Still present: facts with no dependency on `canonical` at all.
        assert!(xml.contains("<gco:CharacterString>demo</gco:CharacterString>"));
        assert!(xml.contains("gco:nilReason=\"unknown\""));
    }

    /// `contact` is schema-mandatory, so with nothing declared it stays a
    /// nilReason — for a collection with no `stac` block at all, and for
    /// one that has a `stac` block declaring everything *but* contacts.
    #[test]
    fn contact_uses_nil_reason_when_no_contact_is_declared() {
        let xml = to_iso19139(None, "demo");
        assert!(xml.contains("<gmd:contact gco:nilReason=\"unknown\"/>"));
        assert!(!xml.contains("CI_ResponsibleParty"));

        // `complete_canonical` carries a `stac` block whose `contacts` list
        // is empty: still nil, no fabricated party.
        let canonical = complete_canonical();
        let xml = to_iso19139(Some(&canonical), "demo");
        assert!(xml.contains("<gmd:contact gco:nilReason=\"unknown\"/>"));
        assert!(!xml.contains("CI_ResponsibleParty"));
    }

    /// `#187`: the pre-existing, unconfigured output is a hard compatibility
    /// boundary — a deployment that declares no contact must get the exact
    /// same bytes it got before contacts existed.
    #[test]
    fn an_empty_contacts_list_renders_identically_to_no_stac_block_at_all() {
        let with_empty_contacts = CanonicalDescriptor {
            stac: Some(CanonicalStac {
                license: None,
                keywords: vec![],
                providers: vec![],
                assets: BTreeMap::new(),
                contacts: vec![],
                lineage: None,
            }),
            ..complete_canonical()
        };
        let without_stac = CanonicalDescriptor {
            stac: None,
            ..complete_canonical()
        };
        assert_eq!(
            to_iso19139(Some(&with_empty_contacts), "demo"),
            to_iso19139(Some(&without_stac), "demo"),
            "an empty contacts list must not perturb the record at all"
        );
    }

    #[test]
    fn a_declared_contact_replaces_the_nil_reason_with_a_real_responsible_party() {
        let canonical = CanonicalDescriptor {
            stac: Some(CanonicalStac {
                contacts: vec![ContactDecl {
                    name: "Ada Lovelace".to_string(),
                    organization: Some("Example Org".to_string()),
                    email: Some("ada@example.com".to_string()),
                    role: Some("custodian".to_string()),
                    url: Some("https://example.com/ada".to_string()),
                }],
                ..stac_block()
            }),
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        assert!(!xml.contains("<gmd:contact gco:nilReason=\"unknown\"/>"));
        assert!(xml.contains("<gmd:CI_ResponsibleParty>"));
        assert!(xml.contains("<gmd:individualName><gco:CharacterString>Ada Lovelace</gco:CharacterString></gmd:individualName>"));
        assert!(xml.contains("<gmd:organisationName><gco:CharacterString>Example Org</gco:CharacterString></gmd:organisationName>"));
        assert!(xml.contains("<gmd:electronicMailAddress><gco:CharacterString>ada@example.com</gco:CharacterString></gmd:electronicMailAddress>"));
        assert!(xml.contains("<gmd:URL>https://example.com/ada</gmd:URL>"));
        assert!(xml.contains("codeListValue=\"custodian\""));
    }

    /// Only `name` is required; every other field absent means an omitted
    /// element, never an empty `gco:CharacterString`. `role` is the single
    /// exception the schema forces — see this module's own doc.
    #[test]
    fn a_name_only_contact_omits_every_optional_element_and_defaults_its_role() {
        let canonical = CanonicalDescriptor {
            stac: Some(CanonicalStac {
                contacts: vec![ContactDecl {
                    name: "Grace Hopper".to_string(),
                    organization: None,
                    email: None,
                    role: None,
                    url: None,
                }],
                ..stac_block()
            }),
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        assert!(xml.contains("<gco:CharacterString>Grace Hopper</gco:CharacterString>"));
        assert!(!xml.contains("organisationName"));
        assert!(!xml.contains("contactInfo"));
        assert!(!xml.contains("electronicMailAddress"));
        assert!(!xml.contains("onlineResource"));
        assert!(!xml.contains("<gco:CharacterString></gco:CharacterString>"));
        assert!(xml.contains("codeListValue=\"pointOfContact\""));
    }

    /// `contact` is `1..*`: every declared party is emitted, in the
    /// operator's own declaration order, with no de-duplication.
    #[test]
    fn every_declared_contact_is_emitted_in_declaration_order() {
        let canonical = CanonicalDescriptor {
            stac: Some(CanonicalStac {
                contacts: vec![
                    ContactDecl {
                        name: "First Party".to_string(),
                        organization: None,
                        email: None,
                        role: Some("owner".to_string()),
                        url: None,
                    },
                    ContactDecl {
                        name: "Second Party".to_string(),
                        organization: None,
                        email: None,
                        role: Some("distributor".to_string()),
                        url: None,
                    },
                ],
                ..stac_block()
            }),
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        assert_eq!(xml.matches("<gmd:CI_ResponsibleParty>").count(), 2);
        let first = xml.find("First Party").expect("first contact present");
        let second = xml.find("Second Party").expect("second contact present");
        assert!(first < second, "declaration order must be preserved");
    }

    /// Contact text is operator-authored free text and reaches the wire
    /// through the same single escaping path every other text node uses —
    /// including the `gmd:URL` linkage, which is not a `CharacterString`.
    #[test]
    fn contact_text_and_url_are_xml_escaped() {
        let canonical = CanonicalDescriptor {
            stac: Some(CanonicalStac {
                contacts: vec![ContactDecl {
                    name: "Ada & <Friends>".to_string(),
                    organization: None,
                    email: None,
                    role: None,
                    url: Some("https://example.com/?a=1&b=2".to_string()),
                }],
                ..stac_block()
            }),
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        assert!(xml.contains("Ada &amp; &lt;Friends&gt;"));
        assert!(xml.contains("<gmd:URL>https://example.com/?a=1&amp;b=2</gmd:URL>"));
        assert!(!xml.contains("Ada & <Friends>"));
    }

    // -- lineage (`#50`, lineage slice) -----------------------------------

    /// An otherwise-complete lineage declaration, so each test below can
    /// override only the member it exercises.
    fn lineage_block() -> LineageDecl {
        LineageDecl {
            statement: Some("Digitised from the 1:25000 IGM series.".to_string()),
            sources: vec![tellurion_core::LineageSourceDecl {
                description: "IGM 1:25000 sheet 45".to_string(),
            }],
            process_steps: vec![tellurion_core::LineageProcessStepDecl {
                description: "Reprojected to EPSG:4326 with ogr2ogr".to_string(),
            }],
        }
    }

    fn canonical_with_lineage(lineage: Option<LineageDecl>) -> CanonicalDescriptor {
        CanonicalDescriptor {
            stac: Some(CanonicalStac {
                lineage,
                ..stac_block()
            }),
            ..complete_canonical()
        }
    }

    /// The compatibility bar this whole slice hangs on: a collection whose
    /// settings chain declares no lineage emits no `gmd:dataQualityInfo` at
    /// all — not an empty element, not boilerplate — for a `None` stac
    /// group, a stac group with no lineage, and a `None` canonical alike,
    /// and the undeclared document is byte-identical to one from a
    /// descriptor that never heard of the field (same proof shape
    /// `an_empty_contacts_list_renders_identically_to_no_stac_block_at_all`
    /// pins for contacts).
    #[test]
    fn undeclared_lineage_emits_no_data_quality_info_at_all() {
        for xml in [
            to_iso19139(None, "demo"),
            to_iso19139(Some(&complete_canonical()), "demo"),
            to_iso19139(Some(&canonical_with_lineage(None)), "demo"),
        ] {
            assert!(!xml.contains("dataQualityInfo"), "{xml}");
            assert!(!xml.contains("LI_Lineage"));
        }
    }

    /// Defense in depth behind `StacConf::validate`'s named load-time
    /// refusal of `lineage: {}`: even if an empty declaration reaches this
    /// pure function (a descriptor built outside a validated config), it
    /// must not fabricate an empty `gmd:LI_Lineage` — the document is
    /// byte-identical to the undeclared one.
    #[test]
    fn an_empty_lineage_declaration_renders_identically_to_no_lineage_at_all() {
        let empty = canonical_with_lineage(Some(LineageDecl::default()));
        assert_eq!(
            to_iso19139(Some(&empty), "demo"),
            to_iso19139(Some(&canonical_with_lineage(None)), "demo"),
            "an empty lineage declaration must not perturb the record at all"
        );
    }

    /// The full nesting ISO 19139 fixes: `gmd:dataQualityInfo >
    /// gmd:DQ_DataQuality > gmd:scope (with a real `DQ_Scope/level`, the
    /// schema-mandatory child) + gmd:lineage > gmd:LI_Lineage`, members in
    /// the schema's own order: statement, processStep, source.
    #[test]
    fn a_declared_lineage_emits_data_quality_with_scope_and_the_fixed_nesting() {
        let canonical = canonical_with_lineage(Some(lineage_block()));
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);

        // Nesting, in document order, within the dataQualityInfo element
        // itself (the whole document also mentions `codeListValue="dataset"`
        // earlier, on `gmd:hierarchyLevel`).
        let dq_start = xml
            .find("<gmd:dataQualityInfo>")
            .expect("dataQualityInfo present");
        let dq = &xml[dq_start..];
        let positions: Vec<usize> = [
            "<gmd:DQ_DataQuality>",
            "<gmd:scope>",
            "<gmd:DQ_Scope>",
            "<gmd:level>",
            "codeListValue=\"dataset\"",
            "</gmd:scope>",
            "<gmd:lineage>",
            "<gmd:LI_Lineage>",
            "<gmd:statement>",
            "<gmd:processStep>",
            "<gmd:LI_ProcessStep>",
            "<gmd:source>",
            "<gmd:LI_Source>",
            "</gmd:LI_Lineage>",
            "</gmd:DQ_DataQuality>",
        ]
        .iter()
        .map(|needle| {
            dq.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "lineage members out of schema order: {xml}"
        );

        // Content: exactly the declared facts, nothing invented.
        assert!(xml.contains(
            "<gmd:statement><gco:CharacterString>Digitised from the 1:25000 IGM series.</gco:CharacterString></gmd:statement>"
        ));
        assert!(xml.contains(
            "<gmd:description><gco:CharacterString>IGM 1:25000 sheet 45</gco:CharacterString></gmd:description>"
        ));
        assert!(xml.contains(
            "<gmd:description><gco:CharacterString>Reprojected to EPSG:4326 with ogr2ogr</gco:CharacterString></gmd:description>"
        ));
    }

    /// Every absent optional member omits its element: a statement-only
    /// declaration carries no `LI_Source`/`LI_ProcessStep` at all, and a
    /// sources-only declaration carries no `gmd:statement` — never an empty
    /// `gco:CharacterString` standing in for either.
    #[test]
    fn lineage_members_absent_from_the_declaration_omit_their_elements() {
        let statement_only = canonical_with_lineage(Some(LineageDecl {
            sources: vec![],
            process_steps: vec![],
            ..lineage_block()
        }));
        let xml = to_iso19139(Some(&statement_only), "demo");
        assert_well_formed(&xml);
        assert!(xml.contains("<gmd:statement>"));
        assert!(!xml.contains("LI_Source"));
        assert!(!xml.contains("LI_ProcessStep"));
        assert!(!xml.contains("<gco:CharacterString></gco:CharacterString>"));

        let sources_only = canonical_with_lineage(Some(LineageDecl {
            statement: None,
            process_steps: vec![],
            ..lineage_block()
        }));
        let xml = to_iso19139(Some(&sources_only), "demo");
        assert_well_formed(&xml);
        assert!(!xml.contains("gmd:statement"));
        assert!(xml.contains("<gmd:LI_Source>"));
        // `scope` is schema-mandatory inside `DQ_DataQuality` and must be
        // present whichever members the declaration carries.
        assert!(xml.contains("<gmd:DQ_Scope>"));
    }

    /// `source`/`processStep` are `0..*`: every declared entry is emitted,
    /// in the operator's own declaration order.
    #[test]
    fn every_declared_lineage_source_and_step_is_emitted_in_declaration_order() {
        let canonical = canonical_with_lineage(Some(LineageDecl {
            statement: None,
            sources: vec![
                tellurion_core::LineageSourceDecl {
                    description: "First source".to_string(),
                },
                tellurion_core::LineageSourceDecl {
                    description: "Second source".to_string(),
                },
            ],
            process_steps: vec![
                tellurion_core::LineageProcessStepDecl {
                    description: "First step".to_string(),
                },
                tellurion_core::LineageProcessStepDecl {
                    description: "Second step".to_string(),
                },
            ],
        }));
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);
        assert_eq!(xml.matches("<gmd:LI_Source>").count(), 2);
        assert_eq!(xml.matches("<gmd:LI_ProcessStep>").count(), 2);
        assert!(xml.find("First source").unwrap() < xml.find("Second source").unwrap());
        assert!(xml.find("First step").unwrap() < xml.find("Second step").unwrap());
    }

    /// Lineage text is operator-authored free text and reaches the wire
    /// through the same single escaping path every other text node uses.
    #[test]
    fn lineage_text_is_xml_escaped() {
        let canonical = canonical_with_lineage(Some(LineageDecl {
            statement: Some("Derived from <survey> & \"field\" notes".to_string()),
            sources: vec![],
            process_steps: vec![],
        }));
        let xml = to_iso19139(Some(&canonical), "demo");
        assert_well_formed(&xml);
        assert!(xml.contains("Derived from &lt;survey&gt; &amp; &quot;field&quot; notes"));
        assert!(!xml.contains("<survey>"));
    }

    #[test]
    fn absent_license_is_a_clean_omission_not_a_fabricated_placeholder() {
        let canonical = CanonicalDescriptor {
            stac: None,
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert!(!xml.contains("resourceConstraints"));
        assert!(!xml.contains("\"other\""));
    }

    #[test]
    fn no_datetime_column_omits_temporal_element_entirely() {
        let canonical = CanonicalDescriptor {
            datetime: None,
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert!(!xml.contains("temporalElement"));
        // Spatial extent is independent of the temporal one and must still
        // be present.
        assert!(xml.contains("EX_GeographicBoundingBox"));
    }

    #[test]
    fn datetime_column_without_spatial_extent_still_emits_the_temporal_nil_reason() {
        let canonical = CanonicalDescriptor {
            extent: None,
            ..complete_canonical()
        };
        let xml = to_iso19139(Some(&canonical), "demo");
        assert!(!xml.contains("EX_GeographicBoundingBox"));
        assert!(xml.contains("temporalElement"));
        assert!(xml.contains("<gmd:extent gco:nilReason=\"unknown\"/>"));
    }

    /// The concrete demonstration this module's own doc promises: ISO 19139
    /// has no provenance slot, so an `Override` and a `Derived` `table` field
    /// carrying the identical value render byte-for-byte identically.
    #[test]
    fn override_and_derived_table_provenance_render_identically() {
        let derived = CanonicalDescriptor {
            table: Some(CanonicalField {
                value: "physical_demo".to_string(),
                provenance: Provenance::Derived,
            }),
            ..complete_canonical()
        };
        let overridden = CanonicalDescriptor {
            table: Some(CanonicalField {
                value: "physical_demo".to_string(),
                provenance: Provenance::Override,
            }),
            ..complete_canonical()
        };
        assert_eq!(
            to_iso19139(Some(&derived), "demo"),
            to_iso19139(Some(&overridden), "demo"),
            "ISO 19139 has no provenance slot: Override and Derived facts of \
             the same value must render identically"
        );
    }

    #[test]
    fn date_stamp_format_is_a_plain_iso_calendar_date() {
        let stamp = date_stamp();
        assert_eq!(stamp.len(), 10);
        assert_eq!(stamp.as_bytes()[4], b'-');
        assert_eq!(stamp.as_bytes()[7], b'-');
    }

    #[test]
    fn civil_from_days_matches_known_reference_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-02-29 (a leap day) is 11016 days after the epoch.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn escape_xml_covers_the_five_predefined_entities() {
        assert_eq!(
            escape_xml("<a & b> \"c\" 'd'"),
            "&lt;a &amp; b&gt; &quot;c&quot; &apos;d&apos;"
        );
    }
}
