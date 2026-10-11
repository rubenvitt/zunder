//! `zunder-import report`: what a run would do, in numbers.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use anyhow::Result;
use serde::Serialize;

use crate::clickup::ClickUpApi;
use crate::convert::{self, Conversion, Diagnostics};
use crate::fetch::{self, Hierarchy};
use crate::mapping::{Resolved, Target};

/// Lists in the mapping that ClickUp no longer has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleList {
    pub list_id: String,
    pub path: String,
}

/// The report (also the `--json` output).
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub team_id: String,
    pub team_name: String,
    /// Lists in ClickUp.
    pub lists_total: usize,
    /// Lists with target `skip` (their tasks are not loaded).
    pub lists_skipped: usize,
    pub lists_missing_in_clickup: Vec<StaleList>,
    #[serde(flatten)]
    pub diagnostics: Diagnostics,
    /// `run` refuses to start while this is true.
    pub blocking: bool,
}

/// Everything loaded and converted for `report` and `run`.
pub struct Loaded {
    pub hierarchy: Hierarchy,
    pub conversion: Conversion,
    pub report: Report,
}

/// Loads hierarchy and tasks according to the mapping and converts them.
pub async fn load(api: &impl ClickUpApi, mapping: &Resolved) -> Result<Loaded> {
    eprintln!("Lese ClickUp-Struktur …");
    let team = fetch::select_team(api, Some(&mapping.team_id)).await?;
    let hierarchy = fetch::fetch_hierarchy(api, team, false).await?;
    let lists = hierarchy.lists();
    let in_clickup: HashSet<&str> = lists.iter().map(|l| l.list.item.id.as_str()).collect();

    let mut to_load = Vec::new();
    let mut skipped = 0;
    let mut paths = HashMap::new();
    for l in &lists {
        let id = l.list.item.id.clone();
        paths.insert(id.clone(), l.path());
        let skip = mapping
            .lists
            .get(&id)
            .is_some_and(|m| m.target == Some(Target::Skip));
        if skip {
            skipped += 1;
        } else {
            to_load.push((id, l.path()));
        }
    }
    let mut stale: Vec<StaleList> = mapping
        .lists
        .iter()
        .filter(|(id, _)| !in_clickup.contains(id.as_str()))
        .map(|(id, l)| StaleList {
            list_id: id.clone(),
            path: l.path.clone(),
        })
        .collect();
    stale.sort_by(|a, b| a.list_id.cmp(&b.list_id));

    eprintln!("Lese Aufgaben aus {} Listen …", to_load.len());
    let tasks = fetch::fetch_tasks(api, &to_load, mapping.import_archived_tasks).await?;
    let conversion = convert::convert(mapping, &tasks, &paths);
    let report = Report {
        team_id: hierarchy.team.item.id.clone(),
        team_name: hierarchy.team.item.name.clone(),
        lists_total: lists.len(),
        lists_skipped: skipped,
        lists_missing_in_clickup: stale,
        diagnostics: conversion.diagnostics.clone(),
        blocking: conversion.diagnostics.is_blocking(),
    };
    Ok(Loaded {
        hierarchy,
        conversion,
        report,
    })
}

/// Maximum number of individual tasks listed per section in the text form.
const MAX_LISTED: usize = 25;

/// German text form of the report.
pub fn render_text(r: &Report) -> String {
    let d = &r.diagnostics;
    let mut o = String::new();
    let w = &mut o;
    let _ = writeln!(
        w,
        "Bericht für ClickUp-Workspace {} ({})",
        r.team_name, r.team_id
    );
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "Listen: {} in ClickUp, davon {} übersprungen (skip)",
        r.lists_total, r.lists_skipped
    );
    let _ = writeln!(
        w,
        "Aufgaben: {} geladen, {} werden importiert, {} in übersprungenen Listen",
        d.tasks_loaded, d.tasks_imported, d.tasks_skipped
    );

    let _ = writeln!(
        w,
        "\nMengen je Ziel (Aufgaben + Unteraufgaben; offen / erledigt+verworfen):"
    );
    if d.targets.is_empty() {
        let _ = writeln!(w, "  (keine)");
    }
    for t in &d.targets {
        let kind = match t.kind.as_str() {
            "area" => "Bereich",
            "project" => "Projekt",
            _ => "",
        };
        let label = if kind.is_empty() {
            t.name.clone()
        } else {
            format!("{kind} {}", t.name)
        };
        let _ = writeln!(
            w,
            "  {label:<50} {:>5} + {:>4}   offen {:>5} / geschlossen {:>5}",
            t.tasks, t.subtasks, t.open, t.closed
        );
    }

    section(
        w,
        "Aufgaben ohne Ziel (Liste fehlt in der Zuordnung oder target leer) – blockiert den Import",
        d.without_target
            .iter()
            .map(|l| format!("{} [{}]: {} Aufgaben", l.path, l.list_id, l.tasks)),
    );
    section(
        w,
        "Nicht zugeordnete Status – blockiert den Import",
        d.unmapped_statuses.iter().map(|s| {
            format!(
                "{} [{}]: „{}“ ({} Aufgaben)",
                s.path, s.list_id, s.status, s.tasks
            )
        }),
    );
    section(
        w,
        "Unteraufgaben ohne importierbare Eltern-Aufgabe",
        d.orphans
            .iter()
            .map(|t| format!("{} „{}“: {}", t.clickup_id, t.title, t.detail)),
    );
    section(
        w,
        "Dev-Typen/-Status außerhalb von Dev-Projekten (werden angepasst)",
        d.dev_downgrades
            .iter()
            .map(|t| format!("{} „{}“: {}", t.clickup_id, t.title, t.detail)),
    );
    section(
        w,
        "Angepasste Titel",
        d.title_fixes
            .iter()
            .map(|t| format!("{} „{}“: {}", t.clickup_id, short(&t.title), t.detail)),
    );
    section(
        w,
        "Listen aus der Zuordnung, die es in ClickUp nicht mehr gibt",
        r.lists_missing_in_clickup
            .iter()
            .map(|l| format!("{} [{}]", l.path, l.list_id)),
    );

    let _ = writeln!(w);
    if r.blocking {
        let _ = writeln!(
            w,
            "Ergebnis: Die Zuordnung ist noch unvollständig. Bitte mapping.toml ergänzen und den Bericht wiederholen."
        );
    } else {
        let _ = writeln!(
            w,
            "Ergebnis: Die Zuordnung ist vollständig, `zunder-import run` kann starten."
        );
    }
    o
}

fn short(s: &str) -> String {
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s.to_owned()
    }
}

fn section(w: &mut String, title: &str, lines: impl ExactSizeIterator<Item = String>) {
    let total = lines.len();
    if total == 0 {
        return;
    }
    let _ = writeln!(w, "\n{title} ({total}):");
    for line in lines.take(MAX_LISTED) {
        let _ = writeln!(w, "  - {line}");
    }
    if total > MAX_LISTED {
        let _ = writeln!(
            w,
            "  … und {} weitere (vollständig mit --json)",
            total - MAX_LISTED
        );
    }
}
