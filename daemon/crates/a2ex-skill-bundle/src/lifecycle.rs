use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::model::{
    BundleDiagnosticCode, BundleDiagnosticSeverity, BundleDocumentLifecycleChange,
    BundleDocumentLifecycleChangeKind, BundleLifecycleChange, BundleLifecycleClassification,
    BundleLifecycleDaemonCompatibility, BundleLifecycleDiagnostic, BundleLifecycleDiagnosticCode,
    BundleLifecycleDocumentSnapshot, BundleLifecycleManifestEntrySnapshot, BundleLifecycleSnapshot,
    BundleLoadOutcome, SkillBundle,
};

const CURRENT_DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn diff_bundle_lifecycle(
    current: &BundleLoadOutcome,
    previous: Option<&BundleLoadOutcome>,
) -> BundleLifecycleChange {
    let previous_snapshot = previous.and_then(BundleLifecycleSnapshot::from_outcome);
    let current_snapshot = BundleLifecycleSnapshot::from_outcome(current);

    let changed_documents =
        diff_changed_documents(previous_snapshot.as_ref(), current_snapshot.as_ref());
    let diagnostics = collect_lifecycle_diagnostics(
        current,
        previous,
        previous_snapshot.as_ref(),
        current_snapshot.as_ref(),
    );
    let metadata_changed = metadata_changed(previous_snapshot.as_ref(), current_snapshot.as_ref());

    let classification = if !diagnostics.is_empty() {
        BundleLifecycleClassification::BlockingDrift
    } else if !changed_documents.is_empty() {
        BundleLifecycleClassification::DocumentsChanged
    } else if metadata_changed {
        BundleLifecycleClassification::MetadataChanged
    } else {
        BundleLifecycleClassification::NoChange
    };

    BundleLifecycleChange {
        previous: previous_snapshot,
        current: current_snapshot,
        classification,
        changed_documents,
        diagnostics,
    }
}

impl BundleLifecycleSnapshot {
    pub(crate) fn from_outcome(outcome: &BundleLoadOutcome) -> Option<Self> {
        outcome.bundle.as_ref().map(Self::from_bundle)
    }

    fn from_bundle(bundle: &SkillBundle) -> Self {
        let documents_by_id: BTreeMap<_, _> = bundle
            .documents
            .iter()
            .map(|document| (document.document_id.as_str(), document))
            .collect();

        let mut manifest_entries: Vec<_> = bundle
            .document_manifest
            .iter()
            .map(|entry| BundleLifecycleManifestEntrySnapshot {
                document_id: entry.document_id.clone(),
                role: entry.role.clone(),
                relative_path: entry.relative_path.clone(),
                required: entry.required,
                declared_revision: entry.revision.clone(),
                resolved_source_url: documents_by_id
                    .get(entry.document_id.as_str())
                    .map(|document| document.source_url.clone()),
            })
            .collect();
        manifest_entries.sort_by(|left, right| left.document_id.cmp(&right.document_id));

        let mut documents: Vec<_> = bundle
            .documents
            .iter()
            .map(|document| {
                let manifest_entry = bundle
                    .document_manifest
                    .iter()
                    .find(|entry| entry.document_id == document.document_id);
                BundleLifecycleDocumentSnapshot {
                    document_id: document.document_id.clone(),
                    role: document.role.clone(),
                    revision: document.revision.clone(),
                    title: document.title.clone(),
                    source_url: Some(document.source_url.clone()),
                    declared_revision: manifest_entry.and_then(|entry| entry.revision.clone()),
                    required: manifest_entry.map(|entry| entry.required),
                    content_fingerprint: Some(fingerprint_markdown(&document.body_markdown)),
                }
            })
            .collect();
        documents.sort_by(|left, right| left.document_id.cmp(&right.document_id));

        Self {
            bundle_id: Some(bundle.bundle_id.clone()),
            bundle_format: Some(bundle.bundle_format.clone()),
            bundle_version: Some(bundle.bundle_version.clone()),
            compatible_daemon: bundle.compatible_daemon.clone(),
            daemon_compatibility: bundle.compatible_daemon.as_ref().map(|requirement| {
                BundleLifecycleDaemonCompatibility {
                    daemon_version: CURRENT_DAEMON_VERSION.to_owned(),
                    requirement: requirement.clone(),
                    is_compatible: requirement_matches_current_daemon(requirement),
                }
            }),
            name: bundle.name.clone(),
            summary: bundle.summary.clone(),
            manifest_entries,
            documents,
        }
    }
}

fn diff_changed_documents(
    previous: Option<&BundleLifecycleSnapshot>,
    current: Option<&BundleLifecycleSnapshot>,
) -> Vec<BundleDocumentLifecycleChange> {
    let previous_documents = previous
        .map(|snapshot| snapshot.documents_by_id())
        .unwrap_or_default();
    let current_documents = current
        .map(|snapshot| snapshot.documents_by_id())
        .unwrap_or_default();

    let document_ids: BTreeSet<String> = previous_documents
        .keys()
        .chain(current_documents.keys())
        .map(|document_id| (*document_id).to_owned())
        .collect();

    let mut changes = Vec::new();

    for document_id in document_ids {
        match (
            previous_documents.get(document_id.as_str()),
            current_documents.get(document_id.as_str()),
        ) {
            (None, Some(current_document)) => changes.push(BundleDocumentLifecycleChange {
                document_id: current_document.document_id.clone(),
                role: Some(current_document.role.clone()),
                kind: BundleDocumentLifecycleChangeKind::Added,
                previous_revision: None,
                current_revision: current_document.revision.clone(),
                source_url: current_document.source_url.clone(),
            }),
            (Some(previous_document), None) => changes.push(BundleDocumentLifecycleChange {
                document_id: previous_document.document_id.clone(),
                role: Some(previous_document.role.clone()),
                kind: BundleDocumentLifecycleChangeKind::Removed,
                previous_revision: previous_document.revision.clone(),
                current_revision: None,
                source_url: previous_document.source_url.clone(),
            }),
            (Some(previous_document), Some(current_document)) => {
                let is_entry_document = previous_document.role
                    == crate::model::BundleDocumentRole::Entry
                    && current_document.role == crate::model::BundleDocumentRole::Entry;
                if is_entry_document {
                    continue;
                }

                if previous_document.revision != current_document.revision {
                    changes.push(BundleDocumentLifecycleChange {
                        document_id: current_document.document_id.clone(),
                        role: Some(current_document.role.clone()),
                        kind: BundleDocumentLifecycleChangeKind::RevisionChanged,
                        previous_revision: previous_document.revision.clone(),
                        current_revision: current_document.revision.clone(),
                        source_url: current_document.source_url.clone(),
                    });
                }

                if previous_document.content_fingerprint != current_document.content_fingerprint
                    || previous_document.source_url != current_document.source_url
                    || previous_document.declared_revision != current_document.declared_revision
                    || previous_document.required != current_document.required
                    || previous_document.role != current_document.role
                {
                    changes.push(BundleDocumentLifecycleChange {
                        document_id: current_document.document_id.clone(),
                        role: Some(current_document.role.clone()),
                        kind: BundleDocumentLifecycleChangeKind::ContentChanged,
                        previous_revision: previous_document.revision.clone(),
                        current_revision: current_document.revision.clone(),
                        source_url: current_document.source_url.clone(),
                    });
                }
            }
            (None, None) => {}
        }
    }

    changes
}

fn collect_lifecycle_diagnostics(
    current: &BundleLoadOutcome,
    previous: Option<&BundleLoadOutcome>,
    previous_snapshot: Option<&BundleLifecycleSnapshot>,
    current_snapshot: Option<&BundleLifecycleSnapshot>,
) -> Vec<BundleLifecycleDiagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(snapshot) = current_snapshot {
        if let Some(compatibility) = &snapshot.daemon_compatibility {
            if !compatibility.is_compatible {
                diagnostics.push(BundleLifecycleDiagnostic {
                    code: BundleLifecycleDiagnosticCode::IncompatibleDaemon,
                    message: format!(
                        "bundle requires daemon '{}' but current daemon version is '{}'",
                        compatibility.requirement, compatibility.daemon_version
                    ),
                    document_id: Some("skill".to_owned()),
                    source_url: snapshot
                        .documents_by_id()
                        .get("skill")
                        .and_then(|document| document.source_url.clone()),
                });
            }
        }

        for manifest_entry in &snapshot.manifest_entries {
            let documents_by_id = snapshot.documents_by_id();
            let current_document = documents_by_id.get(manifest_entry.document_id.as_str());
            match current_document {
                Some(document)
                    if manifest_entry.declared_revision.is_some()
                        && manifest_entry.declared_revision != document.revision =>
                {
                    diagnostics.push(BundleLifecycleDiagnostic {
                        code: BundleLifecycleDiagnosticCode::ManifestDocumentRevisionMismatch,
                        message: format!(
                            "manifest revision {:?} does not match resolved document revision {:?} for '{}'",
                            manifest_entry.declared_revision,
                            document.revision,
                            manifest_entry.document_id
                        ),
                        document_id: Some(manifest_entry.document_id.clone()),
                        source_url: document.source_url.clone(),
                    });
                }
                None if manifest_entry.required => diagnostics.push(required_document_diagnostic(
                    manifest_entry,
                    previous_snapshot,
                )),
                _ => {}
            }
        }
    }

    diagnostics.extend(map_loader_diagnostics_to_lifecycle(
        current,
        previous,
        previous_snapshot,
        current_snapshot,
    ));

    dedupe_diagnostics(diagnostics)
}

fn required_document_diagnostic(
    manifest_entry: &BundleLifecycleManifestEntrySnapshot,
    previous_snapshot: Option<&BundleLifecycleSnapshot>,
) -> BundleLifecycleDiagnostic {
    let existed_previously = previous_snapshot
        .and_then(|snapshot| {
            snapshot
                .documents_by_id()
                .get(manifest_entry.document_id.as_str())
                .copied()
        })
        .is_some();

    BundleLifecycleDiagnostic {
        code: if existed_previously {
            BundleLifecycleDiagnosticCode::RemovedDocument
        } else {
            BundleLifecycleDiagnosticCode::MissingDocument
        },
        message: if existed_previously {
            format!(
                "required document '{}' disappeared from the bundle",
                manifest_entry.document_id
            )
        } else {
            format!(
                "required document '{}' is missing from the bundle",
                manifest_entry.document_id
            )
        },
        document_id: Some(manifest_entry.document_id.clone()),
        source_url: manifest_entry.resolved_source_url.clone(),
    }
}

fn map_loader_diagnostics_to_lifecycle(
    current: &BundleLoadOutcome,
    previous: Option<&BundleLoadOutcome>,
    previous_snapshot: Option<&BundleLifecycleSnapshot>,
    current_snapshot: Option<&BundleLifecycleSnapshot>,
) -> Vec<BundleLifecycleDiagnostic> {
    let previous_required_documents: BTreeSet<_> = previous_snapshot
        .map(|snapshot| {
            snapshot
                .manifest_entries
                .iter()
                .filter(|entry| entry.required)
                .map(|entry| entry.document_id.clone())
                .collect()
        })
        .unwrap_or_default();

    let mut diagnostics = Vec::new();

    for diagnostic in &current.diagnostics {
        let Some(document_id) = diagnostic.document_id.clone() else {
            continue;
        };

        match diagnostic.code {
            BundleDiagnosticCode::MissingRequiredDocument => {
                diagnostics.push(BundleLifecycleDiagnostic {
                    code: if previous_required_documents.contains(&document_id) {
                        BundleLifecycleDiagnosticCode::RemovedDocument
                    } else {
                        BundleLifecycleDiagnosticCode::MissingDocument
                    },
                    message: diagnostic.message.clone(),
                    document_id: Some(document_id),
                    source_url: diagnostic.source_url.clone(),
                });
            }
            BundleDiagnosticCode::FetchFailed
                if diagnostic.severity == BundleDiagnosticSeverity::Error =>
            {
                diagnostics.push(BundleLifecycleDiagnostic {
                    code: BundleLifecycleDiagnosticCode::UnreadableDocument,
                    message: diagnostic.message.clone(),
                    document_id: Some(document_id),
                    source_url: diagnostic.source_url.clone(),
                });
            }
            _ => {}
        }
    }

    if previous.is_some() && current_snapshot.is_none() {
        for document_id in previous_required_documents {
            diagnostics.push(BundleLifecycleDiagnostic {
                code: BundleLifecycleDiagnosticCode::RemovedDocument,
                message: format!(
                    "required document '{}' is no longer available in the current bundle outcome",
                    document_id
                ),
                source_url: previous_snapshot
                    .and_then(|snapshot| {
                        snapshot
                            .documents_by_id()
                            .get(document_id.as_str())
                            .copied()
                    })
                    .and_then(|document| document.source_url.clone()),
                document_id: Some(document_id),
            });
        }
    }

    diagnostics
}

fn dedupe_diagnostics(
    diagnostics: Vec<BundleLifecycleDiagnostic>,
) -> Vec<BundleLifecycleDiagnostic> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();

    for diagnostic in diagnostics {
        let key = (
            diagnostic.code,
            diagnostic.document_id.clone(),
            diagnostic.source_url.clone(),
            diagnostic.message.clone(),
        );
        if seen.insert(key) {
            deduped.push(diagnostic);
        }
    }

    deduped
}

fn metadata_changed(
    previous: Option<&BundleLifecycleSnapshot>,
    current: Option<&BundleLifecycleSnapshot>,
) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => {
            previous.bundle_id != current.bundle_id
                || previous.bundle_format != current.bundle_format
                || previous.bundle_version != current.bundle_version
                || previous.compatible_daemon != current.compatible_daemon
                || previous.name != current.name
                || previous.summary != current.summary
                || previous.daemon_compatibility != current.daemon_compatibility
        }
        (None, None) => false,
        _ => true,
    }
}

fn fingerprint_markdown(markdown: &str) -> String {
    let digest = Sha256::digest(markdown.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn requirement_matches_current_daemon(requirement: &str) -> bool {
    let current_version = match parse_version(CURRENT_DAEMON_VERSION) {
        Some(version) => version,
        None => return false,
    };

    requirement
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(|part| match parse_requirement_clause(part) {
            Some((operator, required_version)) => {
                compare_versions(&current_version, operator, &required_version)
            }
            None => false,
        })
}

fn parse_requirement_clause(input: &str) -> Option<(&str, Vec<u64>)> {
    for operator in [">=", "<=", ">", "<", "="] {
        if let Some(rest) = input.strip_prefix(operator) {
            return parse_version(rest.trim()).map(|version| (operator, version));
        }
    }

    parse_version(input).map(|version| ("=", version))
}

fn parse_version(input: &str) -> Option<Vec<u64>> {
    let core = input.trim().split(['-', '+']).next()?.trim();
    if core.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    for part in core.split('.') {
        parts.push(part.parse().ok()?);
    }
    Some(parts)
}

fn compare_versions(current: &[u64], operator: &str, required: &[u64]) -> bool {
    use std::cmp::Ordering;

    let ordering = compare_version_parts(current, required);
    match operator {
        ">=" => matches!(ordering, Ordering::Greater | Ordering::Equal),
        ">" => ordering == Ordering::Greater,
        "<=" => matches!(ordering, Ordering::Less | Ordering::Equal),
        "<" => ordering == Ordering::Less,
        "=" => ordering == Ordering::Equal,
        _ => false,
    }
}

fn compare_version_parts(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        match left
            .get(index)
            .copied()
            .unwrap_or(0)
            .cmp(&right.get(index).copied().unwrap_or(0))
        {
            std::cmp::Ordering::Equal => continue,
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_requirement_matching_handles_basic_comparators() {
        assert!(requirement_matches_current_daemon(">=0.1.0"));
        assert!(requirement_matches_current_daemon("<=0.1.0"));
        assert!(!requirement_matches_current_daemon(">999.0.0"));
        assert!(!requirement_matches_current_daemon("not-a-version"));
    }
}
