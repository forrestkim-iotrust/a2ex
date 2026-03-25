use std::collections::{BTreeSet, HashMap};

use serde::Deserialize;

use crate::model::{
    BundleDiagnostic, BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
    BundleDocument, BundleDocumentManifestEntry, BundleDocumentRole, BundleSection,
    BundleSectionKind, FetchedBundleDocument, ParsedSkillBundle, SkillBundle,
    UnresolvedBundleSection,
};
use crate::{BundleError, BundleResult};

#[derive(Debug, Deserialize)]
struct EntryFrontmatter {
    bundle_id: String,
    bundle_format: String,
    bundle_version: String,
    #[serde(default)]
    compatible_daemon: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    documents: Vec<EntryDocumentFrontmatter>,
}

#[derive(Debug, Deserialize)]
struct EntryDocumentFrontmatter {
    id: String,
    role: String,
    path: String,
    required: bool,
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SupportingDocumentFrontmatter {
    document_id: String,
    document_role: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Debug)]
struct SectionCandidate {
    heading: String,
    slug: String,
    heading_level: u8,
    markdown: String,
}

#[derive(Debug)]
struct ParsedDocument {
    document: BundleDocument,
    unresolved_sections: Vec<UnresolvedBundleSection>,
    manifest: Option<Vec<BundleDocumentManifestEntry>>,
    bundle_id: Option<String>,
    bundle_format: Option<String>,
    bundle_version: Option<String>,
    compatible_daemon: Option<String>,
    name: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct EntryDocumentMetadata {
    pub(crate) bundle_id: String,
    pub(crate) bundle_format: String,
    pub(crate) bundle_version: String,
    pub(crate) compatible_daemon: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) manifest: Vec<BundleDocumentManifestEntry>,
}

pub fn parse_skill_bundle_documents(
    fetched_documents: Vec<FetchedBundleDocument>,
) -> BundleResult<ParsedSkillBundle> {
    let mut diagnostics = Vec::new();
    let mut parsed_documents = Vec::new();
    let mut seen_document_ids = BTreeSet::new();

    for fetched in fetched_documents {
        if !seen_document_ids.insert(fetched.document_id.clone()) {
            diagnostics.push(BundleDiagnostic {
                code: BundleDiagnosticCode::DuplicateDocumentId,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message: format!("duplicate fetched document id '{}'", fetched.document_id),
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            });
            continue;
        }

        match parse_single_document(fetched) {
            Ok(document) => parsed_documents.push(document),
            Err(BundleError::Diagnostics {
                diagnostics: document_diagnostics,
            }) => diagnostics.extend(document_diagnostics),
            Err(other) => return Err(other),
        }
    }

    if !diagnostics.is_empty() {
        return Err(BundleError::Diagnostics { diagnostics });
    }

    build_bundle(parsed_documents)
}

pub(crate) fn inspect_entry_document(
    fetched: &FetchedBundleDocument,
) -> BundleResult<EntryDocumentMetadata> {
    let (frontmatter, _body) =
        split_frontmatter(&fetched.body_markdown).map_err(|message| BundleError::Diagnostics {
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MalformedFrontmatter,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message,
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            }],
        })?;

    let Some(frontmatter) = frontmatter else {
        return Err(BundleError::Diagnostics {
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MissingRequiredMetadata,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message: "bundle document is missing required YAML frontmatter".to_owned(),
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            }],
        });
    };

    parse_entry_frontmatter(fetched, &frontmatter)
}

fn build_bundle(parsed_documents: Vec<ParsedDocument>) -> BundleResult<ParsedSkillBundle> {
    let mut diagnostics = Vec::new();
    let mut entry_document: Option<ParsedDocument> = None;
    let mut supporting_documents = Vec::new();
    let mut seen_roles = BTreeSet::new();

    for document in parsed_documents {
        if document.document.document_id == "skill" {
            if entry_document.is_some() {
                diagnostics.push(BundleDiagnostic {
                    code: BundleDiagnosticCode::DuplicateDocumentId,
                    severity: BundleDiagnosticSeverity::Error,
                    phase: BundleDiagnosticPhase::ParseDocument,
                    message: "multiple entry documents were provided".to_owned(),
                    document_id: Some("skill".to_owned()),
                    source_url: Some(document.document.source_url.clone()),
                    section_slug: None,
                });
            } else {
                entry_document = Some(document);
            }
        } else {
            if !seen_roles.insert(document.document.role.clone()) {
                diagnostics.push(BundleDiagnostic {
                    code: BundleDiagnosticCode::DuplicateDocumentRole,
                    severity: BundleDiagnosticSeverity::Error,
                    phase: BundleDiagnosticPhase::ParseDocument,
                    message: format!(
                        "duplicate supporting document role '{:?}'",
                        document.document.role
                    ),
                    document_id: Some(document.document.document_id.clone()),
                    source_url: Some(document.document.source_url.clone()),
                    section_slug: None,
                });
            }
            supporting_documents.push(document);
        }
    }

    let Some(entry_document) = entry_document else {
        diagnostics.push(BundleDiagnostic {
            code: BundleDiagnosticCode::MissingRequiredMetadata,
            severity: BundleDiagnosticSeverity::Error,
            phase: BundleDiagnosticPhase::ParseDocument,
            message: "bundle entry document 'skill' was not provided".to_owned(),
            document_id: Some("skill".to_owned()),
            source_url: None,
            section_slug: None,
        });
        return Err(BundleError::Diagnostics { diagnostics });
    };

    let manifest = entry_document.manifest.clone().unwrap_or_default();
    let supporting_by_id: HashMap<_, _> = supporting_documents
        .iter()
        .map(|document| (document.document.document_id.as_str(), document))
        .collect();

    for entry in &manifest {
        if entry.required && !supporting_by_id.contains_key(entry.document_id.as_str()) {
            diagnostics.push(BundleDiagnostic {
                code: BundleDiagnosticCode::MissingRequiredDocument,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::LoadManifest,
                message: format!("required document '{}' was not provided", entry.document_id),
                document_id: Some(entry.document_id.clone()),
                source_url: None,
                section_slug: None,
            });
        }
    }

    if !diagnostics.is_empty() {
        return Err(BundleError::Diagnostics { diagnostics });
    }

    let mut documents = Vec::with_capacity(1 + supporting_documents.len());
    let mut unresolved_sections = Vec::new();

    let ParsedDocument {
        document: entry_bundle_document,
        unresolved_sections: entry_unresolved,
        manifest,
        bundle_id,
        bundle_format,
        bundle_version,
        compatible_daemon,
        name,
        summary,
    } = entry_document;
    documents.push(entry_bundle_document);
    unresolved_sections.extend(entry_unresolved);

    for supporting_document in supporting_documents {
        unresolved_sections.extend(supporting_document.unresolved_sections.clone());
        documents.push(supporting_document.document);
    }

    Ok(ParsedSkillBundle {
        bundle: SkillBundle {
            bundle_id: bundle_id.expect("entry bundle id already validated"),
            bundle_format: bundle_format.expect("entry bundle format already validated"),
            bundle_version: bundle_version.expect("entry bundle version already validated"),
            compatible_daemon,
            entry_document_id: "skill".to_owned(),
            name,
            summary,
            document_manifest: manifest.unwrap_or_default(),
            documents,
            unresolved_sections,
        },
        diagnostics: Vec::new(),
    })
}

fn parse_single_document(fetched: FetchedBundleDocument) -> BundleResult<ParsedDocument> {
    let (frontmatter, body) =
        split_frontmatter(&fetched.body_markdown).map_err(|message| BundleError::Diagnostics {
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MalformedFrontmatter,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message,
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            }],
        })?;

    let Some(frontmatter) = frontmatter else {
        return Err(BundleError::Diagnostics {
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MissingRequiredMetadata,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message: "bundle document is missing required YAML frontmatter".to_owned(),
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            }],
        });
    };

    let sections = split_sections(&body);
    if fetched.document_id == "skill" {
        parse_entry_document(fetched, &frontmatter, sections)
    } else {
        parse_supporting_document(fetched, &frontmatter, sections)
    }
}

fn parse_entry_document(
    fetched: FetchedBundleDocument,
    frontmatter: &str,
    sections: Vec<SectionCandidate>,
) -> BundleResult<ParsedDocument> {
    let metadata = parse_entry_frontmatter(&fetched, frontmatter)?;

    let (resolved_sections, unresolved_sections) = classify_sections(
        BundleDocumentRole::Entry,
        &fetched.document_id,
        &fetched.source_url,
        sections,
    );

    Ok(ParsedDocument {
        document: BundleDocument {
            document_id: fetched.document_id,
            role: BundleDocumentRole::Entry,
            source_url: fetched.source_url,
            title: metadata.name.clone(),
            revision: Some(metadata.bundle_version.clone()),
            body_markdown: fetched.body_markdown,
            sections: resolved_sections,
        },
        unresolved_sections,
        manifest: Some(metadata.manifest.clone()),
        bundle_id: Some(metadata.bundle_id),
        bundle_format: Some(metadata.bundle_format),
        bundle_version: Some(metadata.bundle_version),
        compatible_daemon: metadata.compatible_daemon,
        name: metadata.name,
        summary: metadata.summary,
    })
}

fn parse_entry_frontmatter(
    fetched: &FetchedBundleDocument,
    frontmatter: &str,
) -> BundleResult<EntryDocumentMetadata> {
    let metadata: EntryFrontmatter =
        serde_yaml::from_str(frontmatter).map_err(|error| BundleError::Diagnostics {
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MalformedFrontmatter,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message: format!("failed to parse entry frontmatter: {error}"),
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            }],
        })?;

    let mut diagnostics = Vec::new();
    for (field, value) in [
        ("bundle_id", metadata.bundle_id.trim()),
        ("bundle_format", metadata.bundle_format.trim()),
        ("bundle_version", metadata.bundle_version.trim()),
    ] {
        if value.is_empty() {
            diagnostics.push(missing_metadata_diagnostic(
                fetched,
                format!("entry document is missing required metadata field '{field}'"),
            ));
        }
    }

    let mut manifest = Vec::with_capacity(metadata.documents.len());
    for document in metadata.documents {
        if document.id.trim().is_empty() || document.path.trim().is_empty() {
            diagnostics.push(missing_metadata_diagnostic(
                fetched,
                "entry document manifest items require non-empty id and path".to_owned(),
            ));
            continue;
        }

        let Some(role) = parse_document_role(&document.role) else {
            return Err(BundleError::Diagnostics {
                diagnostics: vec![missing_metadata_diagnostic(
                    fetched,
                    format!(
                        "entry manifest document '{}' has an unsupported role",
                        document.id
                    ),
                )],
            });
        };

        manifest.push(BundleDocumentManifestEntry {
            document_id: document.id,
            role,
            relative_path: document.path,
            required: document.required,
            revision: document.revision,
        });
    }

    if !diagnostics.is_empty() {
        return Err(BundleError::Diagnostics { diagnostics });
    }

    Ok(EntryDocumentMetadata {
        bundle_id: metadata.bundle_id,
        bundle_format: metadata.bundle_format,
        bundle_version: metadata.bundle_version,
        compatible_daemon: metadata.compatible_daemon,
        name: metadata.name,
        summary: metadata.summary,
        manifest,
    })
}

fn parse_supporting_document(
    fetched: FetchedBundleDocument,
    frontmatter: &str,
    sections: Vec<SectionCandidate>,
) -> BundleResult<ParsedDocument> {
    let metadata: SupportingDocumentFrontmatter =
        serde_yaml::from_str(frontmatter).map_err(|error| BundleError::Diagnostics {
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MalformedFrontmatter,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ParseDocument,
                message: format!("failed to parse supporting document frontmatter: {error}"),
                document_id: Some(fetched.document_id.clone()),
                source_url: Some(fetched.source_url.clone()),
                section_slug: None,
            }],
        })?;

    let role =
        parse_document_role(&metadata.document_role).ok_or_else(|| BundleError::Diagnostics {
            diagnostics: vec![missing_metadata_diagnostic(
                &fetched,
                format!(
                    "supporting document '{}' has an unsupported role",
                    metadata.document_id
                ),
            )],
        })?;

    if metadata.document_id.trim().is_empty() || metadata.document_id != fetched.document_id {
        return Err(BundleError::Diagnostics {
            diagnostics: vec![missing_metadata_diagnostic(
                &fetched,
                format!(
                    "supporting document frontmatter document_id '{}' does not match fetched id '{}'",
                    metadata.document_id, fetched.document_id
                ),
            )],
        });
    }

    let (resolved_sections, unresolved_sections) = classify_sections(
        role.clone(),
        &fetched.document_id,
        &fetched.source_url,
        sections,
    );

    Ok(ParsedDocument {
        document: BundleDocument {
            document_id: fetched.document_id,
            role,
            source_url: fetched.source_url,
            title: metadata.title,
            revision: metadata.revision,
            body_markdown: fetched.body_markdown,
            sections: resolved_sections,
        },
        unresolved_sections,
        manifest: None,
        bundle_id: None,
        bundle_format: None,
        bundle_version: None,
        compatible_daemon: None,
        name: None,
        summary: None,
    })
}

fn classify_sections(
    role: BundleDocumentRole,
    document_id: &str,
    source_url: &reqwest::Url,
    sections: Vec<SectionCandidate>,
) -> (Vec<BundleSection>, Vec<UnresolvedBundleSection>) {
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen_slugs: HashMap<String, usize> = HashMap::new();

    for section in sections {
        let unique_slug = unique_slug(&section.slug, &mut seen_slugs);
        let section_id = format!("{document_id}#{unique_slug}");
        if let Some(kind) = section_kind_for_role(&role, &section.slug) {
            resolved.push(BundleSection {
                section_id,
                document_id: document_id.to_owned(),
                section_heading: section.heading,
                section_slug: unique_slug,
                heading_level: section.heading_level,
                kind,
                source_url: source_url.clone(),
                markdown: section.markdown,
            });
        } else {
            unresolved.push(UnresolvedBundleSection {
                section_id,
                document_id: document_id.to_owned(),
                section_heading: section.heading,
                section_slug: unique_slug,
                heading_level: section.heading_level,
                source_url: source_url.clone(),
                markdown: section.markdown,
            });
        }
    }

    (resolved, unresolved)
}

fn split_frontmatter(markdown: &str) -> Result<(Option<String>, String), String> {
    let normalized = markdown.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        return Ok((None, normalized));
    }

    let mut lines = normalized.lines();
    let _opening = lines.next();
    let mut frontmatter_lines = Vec::new();
    let mut body_index = None;
    let mut offset = 4usize;

    for line in lines {
        if line == "---" {
            body_index = Some(offset + line.len() + 1);
            break;
        }
        frontmatter_lines.push(line);
        offset += line.len() + 1;
    }

    let Some(body_start) = body_index else {
        return Err("frontmatter fence is not closed with a terminating --- line".to_owned());
    };

    let body = normalized[body_start..].trim_start_matches('\n').to_owned();
    Ok((Some(frontmatter_lines.join("\n")), body))
}

fn split_sections(body: &str) -> Vec<SectionCandidate> {
    let normalized = body.replace("\r\n", "\n");
    let mut sections = Vec::new();
    let mut current_heading = "Preamble".to_owned();
    let mut current_slug = "preamble".to_owned();
    let mut current_level = 0u8;
    let mut current_body = Vec::new();
    let mut seen_heading = false;

    for line in normalized.lines() {
        if let Some((heading_level, heading)) = parse_heading_line(line) {
            if seen_heading || !current_body.join("\n").trim().is_empty() {
                sections.push(SectionCandidate {
                    heading: current_heading.clone(),
                    slug: current_slug.clone(),
                    heading_level: current_level,
                    markdown: current_body.join("\n").trim().to_owned(),
                });
            }
            seen_heading = true;
            current_heading = heading.to_owned();
            current_slug = slugify(heading);
            current_level = heading_level;
            current_body.clear();
        } else {
            current_body.push(line.to_owned());
        }
    }

    if seen_heading || !current_body.join("\n").trim().is_empty() {
        sections.push(SectionCandidate {
            heading: current_heading,
            slug: current_slug,
            heading_level: current_level,
            markdown: current_body.join("\n").trim().to_owned(),
        });
    }

    sections
        .into_iter()
        .filter(|section| !(section.heading_level == 0 && section.markdown.is_empty()))
        .collect()
}

fn parse_heading_line(line: &str) -> Option<(u8, &str)> {
    let bytes = line.as_bytes();
    let mut level = 0usize;
    while level < bytes.len() && bytes[level] == b'#' {
        level += 1;
    }
    if level == 0 || level > 6 || bytes.get(level) != Some(&b' ') {
        return None;
    }

    Some((level as u8, line[level + 1..].trim()))
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in input.chars().flat_map(|character| character.to_lowercase()) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }

    slug.trim_matches('-').to_owned()
}

fn unique_slug(slug: &str, seen_slugs: &mut HashMap<String, usize>) -> String {
    let entry = seen_slugs.entry(slug.to_owned()).or_insert(0);
    *entry += 1;
    if *entry == 1 {
        slug.to_owned()
    } else {
        format!("{slug}-{}", *entry)
    }
}

fn section_kind_for_role(role: &BundleDocumentRole, slug: &str) -> Option<BundleSectionKind> {
    match (role, slug) {
        (BundleDocumentRole::Entry, "overview") => Some(BundleSectionKind::Overview),
        (BundleDocumentRole::Entry, "owner-decisions") => Some(BundleSectionKind::OwnerDecisions),
        (BundleDocumentRole::OwnerSetup, "required-secrets") => {
            Some(BundleSectionKind::RequiredSecrets)
        }
        _ => None,
    }
}

fn parse_document_role(role: &str) -> Option<BundleDocumentRole> {
    match role.trim() {
        "owner_setup" => Some(BundleDocumentRole::OwnerSetup),
        "operator_notes" => Some(BundleDocumentRole::OperatorNotes),
        "entry" => Some(BundleDocumentRole::Entry),
        _ => None,
    }
}

fn missing_metadata_diagnostic(
    fetched: &FetchedBundleDocument,
    message: String,
) -> BundleDiagnostic {
    BundleDiagnostic {
        code: BundleDiagnosticCode::MissingRequiredMetadata,
        severity: BundleDiagnosticSeverity::Error,
        phase: BundleDiagnosticPhase::ParseDocument,
        message,
        document_id: Some(fetched.document_id.clone()),
        source_url: Some(fetched.source_url.clone()),
        section_slug: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BundleError;
    use reqwest::Url;

    const ENTRY_SKILL_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.12
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.10
---
# Overview

Track spread divergences.

# Unknown Policy Surface

Keep this unresolved.
"#;

    const OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

    #[test]
    fn parses_valid_bundle_documents_and_preserves_unknown_headings() {
        let entry_url =
            Url::parse("https://bundles.a2ex.local/skills/prediction-spread-arb/skill.md")
                .expect("entry url parses");
        let owner_setup_url = entry_url.join("docs/owner-setup.md").expect("join works");

        let parsed = parse_skill_bundle_documents(vec![
            FetchedBundleDocument {
                document_id: "skill".to_owned(),
                source_url: entry_url.clone(),
                body_markdown: ENTRY_SKILL_MD.to_owned(),
            },
            FetchedBundleDocument {
                document_id: "owner-setup".to_owned(),
                source_url: owner_setup_url,
                body_markdown: OWNER_SETUP_MD.to_owned(),
            },
        ])
        .expect("bundle parses");

        assert!(parsed.diagnostics.is_empty());
        assert_eq!(parsed.bundle.bundle_id, "official.prediction-spread-arb");
        assert_eq!(parsed.bundle.documents.len(), 2);
        assert_eq!(parsed.bundle.documents[0].sections.len(), 1);
        assert_eq!(parsed.bundle.unresolved_sections.len(), 1);
        assert_eq!(
            parsed.bundle.unresolved_sections[0].section_slug,
            "unknown-policy-surface"
        );
        assert!(
            parsed.bundle.unresolved_sections[0]
                .markdown
                .contains("Keep this unresolved")
        );
    }

    #[test]
    fn inspect_entry_document_returns_manifest_without_supporting_documents() {
        let entry_url =
            Url::parse("https://bundles.a2ex.local/skills/prediction-spread-arb/skill.md")
                .expect("entry url parses");
        let metadata = inspect_entry_document(&FetchedBundleDocument {
            document_id: "skill".to_owned(),
            source_url: entry_url,
            body_markdown: ENTRY_SKILL_MD.to_owned(),
        })
        .expect("entry metadata parses");

        assert_eq!(metadata.bundle_id, "official.prediction-spread-arb");
        assert_eq!(metadata.manifest.len(), 1);
        assert_eq!(metadata.manifest[0].document_id, "owner-setup");
    }

    #[test]
    fn malformed_frontmatter_returns_typed_diagnostics() {
        let entry_url =
            Url::parse("https://bundles.a2ex.local/skills/prediction-spread-arb/skill.md")
                .expect("entry url parses");
        let error = parse_skill_bundle_documents(vec![FetchedBundleDocument {
            document_id: "skill".to_owned(),
            source_url: entry_url.clone(),
            body_markdown: "---\nbundle_id: broken\nbundle_format: [\n# Overview\n".to_owned(),
        }])
        .expect_err("malformed frontmatter must fail");

        match error {
            BundleError::Diagnostics { diagnostics } => {
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(
                    diagnostics[0].code,
                    BundleDiagnosticCode::MalformedFrontmatter
                );
                assert_eq!(diagnostics[0].phase, BundleDiagnosticPhase::ParseDocument);
                assert_eq!(diagnostics[0].document_id.as_deref(), Some("skill"));
                assert_eq!(diagnostics[0].source_url.as_ref(), Some(&entry_url));
            }
            other => panic!("expected typed diagnostics, got {other:?}"),
        }
    }
}
