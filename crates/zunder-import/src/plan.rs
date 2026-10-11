//! `zunder-import plan`: reads the ClickUp structure and proposes a mapping.

use anyhow::Result;

use crate::clickup::ClickUpApi;
use crate::fetch;
use crate::mapping::{self, Mapping};

/// A proposed mapping, rendered and as data.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub mapping: Mapping,
    /// Commented TOML, ready to be written to `mapping.toml`.
    pub toml: String,
    /// Number of list statuses without a confident proposal.
    pub open_statuses: usize,
}

/// Reads spaces, folders, lists and their statuses and proposes a mapping.
pub async fn plan(
    api: &impl ClickUpApi,
    team_id: Option<&str>,
    generated_on: &str,
) -> Result<Proposal> {
    let team = fetch::select_team(api, team_id).await?;
    eprintln!(
        "Lese Struktur von Workspace {} ({}) …",
        team.item.name, team.item.id
    );
    let team_name = team.item.name.clone();
    let hierarchy = fetch::fetch_hierarchy(api, team, true).await?;
    let mapping = mapping::propose(&hierarchy);
    let open_statuses = mapping
        .lists
        .values()
        .flat_map(|l| l.statuses.values())
        .filter(|s| s.is_empty())
        .count();
    let toml = mapping::render(&mapping, &team_name, generated_on);
    Ok(Proposal {
        mapping,
        toml,
        open_statuses,
    })
}
