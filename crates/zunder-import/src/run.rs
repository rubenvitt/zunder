//! `zunder-import run`: writes the converted tasks to Zunder.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::Serialize;
use zunder_core::hlc::system_time_ms;
use zunder_core::{validate_push, EntityKind, State};

use crate::convert::Conversion;
use crate::ops::{self, DiffStats};
use crate::zunder::{PushWarning, RejectedOp, ZunderClient};

/// Default number of operations per push.
pub const DEFAULT_BATCH_SIZE: usize = 500;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub dry_run: bool,
    pub keep_going: bool,
    pub batch_size: usize,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            keep_going: false,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

/// A rejected operation with the entity it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rejected {
    pub entity: String,
    pub field: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunSummary {
    pub dry_run: bool,
    /// Operations that were (or would be) pushed.
    pub ops: usize,
    pub entities_created: usize,
    pub entities_updated: usize,
    pub entities_unchanged: usize,
    pub entities_deleted_in_zunder: usize,
    /// Tasks recurring in Zunder (labels); none of their fields is written.
    pub recurring_in_zunder: Vec<String>,
    pub batches: usize,
    pub accepted: usize,
    /// Rejections predicted by the local check (same rules as the server).
    pub predicted_rejections: Vec<Rejected>,
    /// Entities too large for a push body; never sent.
    pub oversized: Vec<String>,
    pub rejected: Vec<Rejected>,
    pub warnings: Vec<PushWarning>,
}

/// Runs the import.
///
/// `zunder` may only be `None` for a dry run; then the state in Zunder is
/// assumed to be empty.
pub async fn run(
    conversion: &Conversion,
    zunder: Option<&ZunderClient>,
    options: &RunOptions,
) -> Result<RunSummary> {
    let diag = &conversion.diagnostics;
    if diag.is_blocking() {
        bail!(
            "Die Zuordnung ist unvollständig ({} Listen ohne Ziel, {} nicht zugeordnete Status). \
             Details mit `zunder-import report`.",
            diag.without_target.len(),
            diag.unmapped_statuses.len()
        );
    }
    if zunder.is_none() && !options.dry_run {
        bail!("Ohne Zunder-Server (--server, --token) ist nur ein Probelauf (--dry-run) möglich");
    }

    let state = match zunder {
        Some(client) => {
            eprintln!("Lese aktuellen Zustand aus Zunder …");
            let snapshot = client.snapshot().await?;
            Some(State::from_snapshot(&snapshot.entities))
        }
        None => None,
    };
    if let Some(state) = &state {
        let missing: Vec<String> = conversion
            .existing_refs
            .iter()
            .filter(|(kind, id, _)| state.live_entity(*kind, *id).is_none())
            .map(|(kind, id, name)| format!("{} „{name}“ ({id})", kind.label_de()))
            .collect();
        if !missing.is_empty() {
            bail!(
                "existing_id verweist auf Einträge, die es in Zunder nicht (mehr) gibt: {}",
                missing.join(", ")
            );
        }
        check_existing_project_kinds(conversion, state)?;
    }

    let now = system_time_ms();
    let mut clock = ops::import_clock(state.as_ref());
    let (groups, stats) = ops::plan_ops(conversion.entities(), state.as_ref(), &mut clock, now);
    let index = OpIndex::new(&groups);

    let mut summary = RunSummary {
        dry_run: options.dry_run,
        ..summary_from(stats)
    };

    // Entities too large for any push body are left out (the server would
    // answer 413); the check below then also predicts what depends on them.
    let (oversized, groups): (Vec<_>, Vec<_>) = groups
        .into_iter()
        .partition(|g| ops::group_bytes(g) > ops::MAX_PUSH_BYTES);
    summary.oversized = oversized
        .iter()
        .map(|g| format!("{} ({} KiB)", g.label, ops::group_bytes(g) / 1024))
        .collect();
    let batches = ops::batches(&groups, options.batch_size, ops::MAX_PUSH_BYTES);
    summary.batches = batches.len();
    summary.ops = batches.iter().map(Vec::len).sum();

    // Local check with the same rules as the server, before anything is
    // written: avoids half-finished imports. The server validates every
    // batch on its own against its state at that moment, so the check does
    // the same, batch by batch on a copy of the snapshot.
    let mut simulated = state.clone().unwrap_or_default();
    for batch in &batches {
        let outcome = validate_push(&simulated, batch, now);
        summary.predicted_rejections.extend(
            outcome
                .rejected
                .iter()
                .map(|r| index.describe(&r.op_id.to_string(), r.code.as_str(), &r.message)),
        );
        for op in &outcome.new_ops {
            simulated.apply(op);
        }
    }

    if options.dry_run {
        return Ok(summary);
    }
    if !summary.oversized.is_empty() && !options.keep_going {
        bail!(
            "{} Einträge sind zu groß für eine Anfrage an Zunder (höchstens {} MiB), es wurde \
             nichts geschrieben:\n{}\nMeist ist es eine sehr lange Beschreibung: in ClickUp \
             kürzen oder die Liste überspringen. Mit --keep-going werden sie ausgelassen.",
            summary.oversized.len(),
            ops::MAX_PUSH_BYTES / (1024 * 1024),
            list_labels(&summary.oversized)
        );
    }
    if !summary.predicted_rejections.is_empty() && !options.keep_going {
        bail!(
            "Die lokale Prüfung lehnt {} Operationen ab, es wurde nichts geschrieben:\n{}\n\
             Mit --keep-going wird trotzdem geschrieben (abgelehnte Einträge fehlen dann).",
            summary.predicted_rejections.len(),
            list_rejections(&summary.predicted_rejections)
        );
    }

    let client = zunder.expect("checked above");
    let total = batches.len();
    for (n, batch) in batches.iter().enumerate() {
        let response = client.push(batch).await?;
        summary.accepted += response.accepted.len();
        summary.warnings.extend(response.warnings);
        let rejected: Vec<Rejected> = response
            .rejected
            .iter()
            .map(|r: &RejectedOp| index.describe(&r.op_id, &r.code, &r.message))
            .collect();
        eprintln!(
            "Paket {}/{total}: {} Operationen, {} angenommen, {} abgelehnt",
            n + 1,
            batch.len(),
            response.accepted.len(),
            rejected.len()
        );
        let failed = !rejected.is_empty();
        summary.rejected.extend(rejected);
        if failed && !options.keep_going {
            bail!(
                "Zunder hat Operationen abgelehnt, Abbruch nach Paket {}/{total}:\n{}\n\
                 Bereits geschriebene Pakete bleiben erhalten; ein erneuter Lauf setzt fort.",
                n + 1,
                list_rejections(&summary.rejected)
            );
        }
    }
    Ok(summary)
}

/// The conversion applies the dev rules (bug/feature/review only in dev
/// projects) with the `kind` from the mapping file. For projects mapped via
/// `existing_id` nothing is written, so a `kind` that differs from the real
/// project would silently downgrade (or wrongly keep) dev types and
/// statuses. Refuse instead.
fn check_existing_project_kinds(conversion: &Conversion, state: &State) -> Result<()> {
    let mismatched: Vec<String> = conversion
        .existing_project_kinds
        .iter()
        .filter_map(|(id, name, kind)| {
            let actual = state.live_entity(EntityKind::Project, *id)?.project_kind();
            (actual != *kind).then(|| {
                format!(
                    "Projekt „{name}“ ({id}) ist in Zunder kind = \"{actual}\", \
                     in der Zuordnung kind = \"{kind}\""
                )
            })
        })
        .collect();
    if !mismatched.is_empty() {
        bail!(
            "Bei Projekten mit existing_id muss kind der Art des Zunder-Projekts entsprechen \
             (davon hängt ab, ob bug/feature/review erhalten bleiben). Bitte in der \
             Zuordnungsdatei anpassen:\n  - {}",
            mismatched.join("\n  - ")
        );
    }
    Ok(())
}

fn summary_from(stats: DiffStats) -> RunSummary {
    RunSummary {
        entities_created: stats.created,
        entities_updated: stats.updated,
        entities_unchanged: stats.unchanged,
        entities_deleted_in_zunder: stats.deleted_in_zunder,
        recurring_in_zunder: stats.recurring_in_zunder,
        ..RunSummary::default()
    }
}

/// op_id → (entity label, field) for messages.
struct OpIndex(HashMap<String, (String, String)>);

impl OpIndex {
    fn new(groups: &[ops::EntityOps]) -> Self {
        Self(
            groups
                .iter()
                .flat_map(|g| {
                    g.ops
                        .iter()
                        .map(|op| (op.op_id.to_string(), (g.label.clone(), op.field.clone())))
                })
                .collect(),
        )
    }

    fn describe(&self, op_id: &str, code: &str, message: &str) -> Rejected {
        let (entity, field) = self
            .0
            .get(op_id)
            .cloned()
            .unwrap_or_else(|| (format!("Operation {op_id}"), String::new()));
        Rejected {
            entity,
            field,
            code: code.to_owned(),
            message: message.to_owned(),
        }
    }
}

const MAX_LISTED: usize = 30;

/// One line per entity (first rejection), at most [`MAX_LISTED`].
pub fn list_rejections(rejected: &[Rejected]) -> String {
    let mut seen = std::collections::HashSet::new();
    let lines: Vec<String> = rejected
        .iter()
        .filter(|r| r.code != "entity_rejected" && seen.insert(r.entity.clone()))
        .map(|r| format!("  - {} [{}]: {} ({})", r.entity, r.field, r.message, r.code))
        .collect();
    let mut out = lines
        .iter()
        .take(MAX_LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > MAX_LISTED {
        out.push_str(&format!("\n  … und {} weitere", lines.len() - MAX_LISTED));
    }
    out
}

fn list_labels(labels: &[String]) -> String {
    let mut out = labels
        .iter()
        .take(MAX_LISTED)
        .map(|label| format!("  - {label}"))
        .collect::<Vec<_>>()
        .join("\n");
    if labels.len() > MAX_LISTED {
        out.push_str(&format!("\n  … und {} weitere", labels.len() - MAX_LISTED));
    }
    out
}

/// German text summary of a run.
pub fn render_summary(s: &RunSummary) -> String {
    let mut out = String::new();
    if s.dry_run {
        out.push_str(&format!(
            "Probelauf: {} Operationen in {} Paketen würden geschrieben.\n",
            s.ops, s.batches
        ));
    } else {
        out.push_str(&format!(
            "Import abgeschlossen: {} Operationen in {} Paketen, {} angenommen, {} abgelehnt.\n",
            s.ops,
            s.batches,
            s.accepted,
            s.rejected.len()
        ));
    }
    out.push_str(&format!(
        "Einträge: {} neu, {} geändert, {} unverändert",
        s.entities_created, s.entities_updated, s.entities_unchanged
    ));
    if s.entities_deleted_in_zunder > 0 {
        out.push_str(&format!(
            ", {} in Zunder gelöscht (übersprungen)",
            s.entities_deleted_in_zunder
        ));
    }
    out.push('\n');
    if !s.recurring_in_zunder.is_empty() {
        out.push_str(&format!(
            "In Zunder wiederkehrend, übersprungen ({}):\n{}\n",
            s.recurring_in_zunder.len(),
            list_labels(&s.recurring_in_zunder)
        ));
    }
    if !s.oversized.is_empty() {
        out.push_str(&format!(
            "Zu groß für eine Anfrage, {}:\n{}\n",
            if s.dry_run {
                "würden ausgelassen"
            } else {
                "ausgelassen"
            },
            list_labels(&s.oversized)
        ));
    }
    if s.dry_run && !s.predicted_rejections.is_empty() {
        out.push_str(&format!(
            "Die lokale Prüfung würde {} Operationen ablehnen:\n{}\n",
            s.predicted_rejections.len(),
            list_rejections(&s.predicted_rejections)
        ));
    }
    if !s.rejected.is_empty() {
        out.push_str(&format!("Abgelehnt:\n{}\n", list_rejections(&s.rejected)));
    }
    if !s.warnings.is_empty() {
        out.push_str(&format!("Hinweise von Zunder ({}):\n", s.warnings.len()));
        for w in s.warnings.iter().take(MAX_LISTED) {
            out.push_str(&format!(
                "  - {} ({}): {}\n",
                w.entity_id, w.code, w.message
            ));
        }
    }
    out
}
