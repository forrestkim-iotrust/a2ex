use std::collections::{BTreeMap, BTreeSet};

use reqwest::{Client, Url};

use crate::model::{
    BundleDiagnostic, BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
    BundleDocumentManifestEntry, BundleDocumentRole, BundleLoadOutcome, FetchedBundleDocument,
};
use crate::parser::inspect_entry_document;
use crate::{BundleError, BundleResult, parse_skill_bundle_documents};

const MAX_SUPPORTING_DOCUMENTS: usize = 32;

pub async fn load_skill_bundle_from_url(
    client: &Client,
    entry_url: Url,
) -> BundleResult<BundleLoadOutcome> {
    let mut diagnostics = Vec::new();

    let entry_document =
        match fetch_markdown_document(client, "skill", entry_url.clone(), true).await {
            Ok(Some(entry_document)) => entry_document,
            Ok(None) => {
                diagnostics.push(BundleDiagnostic {
                    code: BundleDiagnosticCode::MissingRequiredDocument,
                    severity: BundleDiagnosticSeverity::Error,
                    phase: BundleDiagnosticPhase::LoadManifest,
                    message: "entry document could not be fetched".to_owned(),
                    document_id: Some("skill".to_owned()),
                    source_url: Some(entry_url),
                    section_slug: None,
                });
                return Ok(BundleLoadOutcome {
                    bundle: None,
                    diagnostics,
                });
            }
            Err(BundleError::Diagnostics {
                diagnostics: fetch_diagnostics,
            }) => {
                diagnostics.extend(fetch_diagnostics);
                return Ok(BundleLoadOutcome {
                    bundle: None,
                    diagnostics,
                });
            }
            Err(other) => return Err(other),
        };

    let entry_metadata = match inspect_entry_document(&entry_document) {
        Ok(metadata) => metadata,
        Err(BundleError::Diagnostics {
            diagnostics: parse_diagnostics,
        }) => {
            diagnostics.extend(parse_diagnostics);
            return Ok(BundleLoadOutcome {
                bundle: None,
                diagnostics,
            });
        }
        Err(other) => return Err(other),
    };

    if entry_metadata.manifest.len() > MAX_SUPPORTING_DOCUMENTS {
        diagnostics.push(BundleDiagnostic {
            code: BundleDiagnosticCode::ReferenceDepthExceeded,
            severity: BundleDiagnosticSeverity::Error,
            phase: BundleDiagnosticPhase::ResolveDocument,
            message: format!(
                "entry manifest declares {} supporting documents, exceeding the loader limit of {}",
                entry_metadata.manifest.len(),
                MAX_SUPPORTING_DOCUMENTS
            ),
            document_id: Some("skill".to_owned()),
            source_url: Some(entry_document.source_url.clone()),
            section_slug: None,
        });
        return Ok(BundleLoadOutcome {
            bundle: None,
            diagnostics,
        });
    }

    let supporting_fetch_plan = build_supporting_fetch_plan(
        &entry_document.source_url,
        &entry_metadata.manifest,
        &mut diagnostics,
    );
    let has_blocking_resolution_error = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == BundleDiagnosticSeverity::Error);

    let mut fetched_documents = vec![entry_document];

    for fetch_target in supporting_fetch_plan {
        match fetch_markdown_document(
            client,
            &fetch_target.document_id,
            fetch_target.resolved_url.clone(),
            fetch_target.required,
        )
        .await
        {
            Ok(Some(document)) => fetched_documents.push(document),
            Ok(None) if fetch_target.required => diagnostics.push(BundleDiagnostic {
                code: BundleDiagnosticCode::MissingRequiredDocument,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::LoadManifest,
                message: format!(
                    "required document '{}' could not be fetched",
                    fetch_target.document_id
                ),
                document_id: Some(fetch_target.document_id),
                source_url: Some(fetch_target.resolved_url),
                section_slug: None,
            }),
            Ok(None) => diagnostics.push(fetch_failed_diagnostic(
                &fetch_target.document_id,
                fetch_target.resolved_url,
                false,
                None,
                format!(
                    "bundle document '{}' could not be fetched",
                    fetch_target.document_id
                ),
            )),
            Err(BundleError::Diagnostics {
                diagnostics: fetch_diagnostics,
            }) => diagnostics.extend(fetch_diagnostics),
            Err(other) => return Err(other),
        }
    }

    let has_blocking_fetch_error = diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == BundleDiagnosticSeverity::Error
            && matches!(
                diagnostic.phase,
                BundleDiagnosticPhase::FetchDocument | BundleDiagnosticPhase::ResolveDocument
            )
    });

    if has_blocking_resolution_error || has_blocking_fetch_error {
        return Ok(BundleLoadOutcome {
            bundle: None,
            diagnostics,
        });
    }

    match parse_skill_bundle_documents(fetched_documents) {
        Ok(parsed) => {
            diagnostics.extend(parsed.diagnostics);
            Ok(BundleLoadOutcome {
                bundle: Some(parsed.bundle),
                diagnostics,
            })
        }
        Err(BundleError::Diagnostics {
            diagnostics: parse_diagnostics,
        }) => {
            diagnostics.extend(parse_diagnostics);
            Ok(BundleLoadOutcome {
                bundle: None,
                diagnostics,
            })
        }
        Err(other) => Err(other),
    }
}

#[derive(Debug)]
struct SupportingFetchTarget {
    document_id: String,
    required: bool,
    resolved_url: Url,
}

fn build_supporting_fetch_plan(
    entry_source_url: &Url,
    manifest: &[BundleDocumentManifestEntry],
    diagnostics: &mut Vec<BundleDiagnostic>,
) -> Vec<SupportingFetchTarget> {
    let mut fetch_targets = Vec::new();
    let mut seen_document_ids: BTreeMap<String, Url> = BTreeMap::new();
    let mut seen_roles: BTreeMap<BundleDocumentRole, String> = BTreeMap::new();
    let mut fetched_urls: BTreeSet<Url> = BTreeSet::new();

    for entry in manifest {
        let resolved_url = match entry_source_url.join(&entry.relative_path) {
            Ok(url) => url,
            Err(error) => {
                diagnostics.push(BundleDiagnostic {
                    code: BundleDiagnosticCode::InvalidDocumentReference,
                    severity: if entry.required {
                        BundleDiagnosticSeverity::Error
                    } else {
                        BundleDiagnosticSeverity::Warning
                    },
                    phase: BundleDiagnosticPhase::ResolveDocument,
                    message: format!(
                        "failed to resolve '{}' from '{}': {error}",
                        entry.relative_path, entry_source_url
                    ),
                    document_id: Some(entry.document_id.clone()),
                    source_url: Some(entry_source_url.clone()),
                    section_slug: None,
                });
                continue;
            }
        };

        if resolved_url == *entry_source_url {
            diagnostics.push(BundleDiagnostic {
                code: BundleDiagnosticCode::ReferenceCycle,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::ResolveDocument,
                message: format!(
                    "document '{}' resolves back to the entry document URL",
                    entry.document_id
                ),
                document_id: Some(entry.document_id.clone()),
                source_url: Some(resolved_url),
                section_slug: None,
            });
            continue;
        }

        if let Some(existing_url) = seen_document_ids.get(&entry.document_id) {
            if *existing_url != resolved_url {
                diagnostics.push(BundleDiagnostic {
                    code: BundleDiagnosticCode::DuplicateDocumentId,
                    severity: BundleDiagnosticSeverity::Error,
                    phase: BundleDiagnosticPhase::ResolveDocument,
                    message: format!(
                        "document id '{}' is declared more than once with different URLs",
                        entry.document_id
                    ),
                    document_id: Some(entry.document_id.clone()),
                    source_url: Some(resolved_url),
                    section_slug: None,
                });
            }
            continue;
        }

        if let Some(existing_document_id) = seen_roles.get(&entry.role) {
            if existing_document_id != &entry.document_id {
                diagnostics.push(BundleDiagnostic {
                    code: BundleDiagnosticCode::DuplicateDocumentRole,
                    severity: BundleDiagnosticSeverity::Error,
                    phase: BundleDiagnosticPhase::ResolveDocument,
                    message: format!(
                        "document role '{:?}' is assigned to both '{}' and '{}'",
                        entry.role, existing_document_id, entry.document_id
                    ),
                    document_id: Some(entry.document_id.clone()),
                    source_url: Some(resolved_url),
                    section_slug: None,
                });
                continue;
            }
        }

        seen_document_ids.insert(entry.document_id.clone(), resolved_url.clone());
        seen_roles.insert(entry.role.clone(), entry.document_id.clone());

        if !fetched_urls.insert(resolved_url.clone()) {
            continue;
        }

        fetch_targets.push(SupportingFetchTarget {
            document_id: entry.document_id.clone(),
            required: entry.required,
            resolved_url,
        });
    }

    fetch_targets
}

async fn fetch_markdown_document(
    client: &Client,
    document_id: &str,
    url: Url,
    required: bool,
) -> BundleResult<Option<FetchedBundleDocument>> {
    let response = match client.get(url.clone()).send().await {
        Ok(response) => response,
        Err(error) => {
            return Err(BundleError::Diagnostics {
                diagnostics: vec![fetch_failed_diagnostic(
                    document_id,
                    url,
                    required,
                    Some(error),
                    format!("request for bundle document '{document_id}' failed"),
                )],
            });
        }
    };

    let source_url = response.url().clone();
    let status = response.status();
    if !status.is_success() {
        return Ok(None);
    }

    match response.text().await {
        Ok(body_markdown) => Ok(Some(FetchedBundleDocument {
            document_id: document_id.to_owned(),
            source_url,
            body_markdown,
        })),
        Err(error) => Err(BundleError::Diagnostics {
            diagnostics: vec![fetch_failed_diagnostic(
                document_id,
                url,
                required,
                Some(error),
                format!("response body for bundle document '{document_id}' could not be read"),
            )],
        }),
    }
}

fn fetch_failed_diagnostic(
    document_id: &str,
    source_url: Url,
    required: bool,
    error: Option<reqwest::Error>,
    message: String,
) -> BundleDiagnostic {
    let suffix = error.map(|error| format!(": {error}")).unwrap_or_default();
    BundleDiagnostic {
        code: BundleDiagnosticCode::FetchFailed,
        severity: if required {
            BundleDiagnosticSeverity::Error
        } else {
            BundleDiagnosticSeverity::Warning
        },
        phase: BundleDiagnosticPhase::FetchDocument,
        message: format!("{message}{suffix}"),
        document_id: Some(document_id.to_owned()),
        source_url: Some(source_url),
        section_slug: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_manifest_cycles_and_duplicate_roles_before_fetch() {
        let entry_url =
            Url::parse("https://bundles.a2ex.local/skills/prediction-spread-arb/skill.md")
                .expect("entry url parses");
        let manifest = vec![
            BundleDocumentManifestEntry {
                document_id: "owner-setup".to_owned(),
                role: BundleDocumentRole::OwnerSetup,
                relative_path: "docs/owner-setup.md".to_owned(),
                required: true,
                revision: None,
            },
            BundleDocumentManifestEntry {
                document_id: "owner-setup-copy".to_owned(),
                role: BundleDocumentRole::OwnerSetup,
                relative_path: "docs/owner-setup-copy.md".to_owned(),
                required: true,
                revision: None,
            },
            BundleDocumentManifestEntry {
                document_id: "operator-notes".to_owned(),
                role: BundleDocumentRole::OperatorNotes,
                relative_path: "skill.md".to_owned(),
                required: true,
                revision: None,
            },
        ];

        let mut diagnostics = Vec::new();
        let plan = build_supporting_fetch_plan(&entry_url, &manifest, &mut diagnostics);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].document_id, "owner-setup");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == BundleDiagnosticCode::ReferenceCycle
                && diagnostic.phase == BundleDiagnosticPhase::ResolveDocument
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == BundleDiagnosticCode::DuplicateDocumentRole
                && diagnostic.phase == BundleDiagnosticPhase::ResolveDocument
        }));
    }
}
