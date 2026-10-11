//! Command line of `zunder-import`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate, Utc};
use clap::{Args, Parser, Subcommand};

use zunder_import::backup::{self, BackupOptions};
use zunder_import::clickup::{HttpClickUp, DEFAULT_BASE_URL};
use zunder_import::mapping::Mapping;
use zunder_import::run::{RunOptions, DEFAULT_BATCH_SIZE};
use zunder_import::zunder::ZunderClient;
use zunder_import::{plan, report, run};

/// Import aus ClickUp nach Zunder: Probelauf, Bericht, Import, Sicherung.
#[derive(Debug, Parser)]
#[command(name = "zunder-import", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Liest die ClickUp-Struktur und schlägt eine Zuordnungsdatei vor.
    Plan {
        /// Ziel der Zuordnungsdatei.
        #[arg(long, default_value = "mapping.toml")]
        out: PathBuf,
        /// ClickUp-Workspace-ID (nur nötig, wenn der Token mehrere sieht).
        #[arg(long)]
        team: Option<String>,
        /// Vorhandene Datei überschreiben.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        clickup: ClickUpArgs,
    },
    /// Zeigt, was ein Import mit dieser Zuordnung täte (Mengen, Lücken).
    Report {
        /// Zuordnungsdatei (von `plan` erzeugt, danach bearbeitet).
        #[arg(long, default_value = "mapping.toml")]
        mapping: PathBuf,
        /// Maschinenlesbare Ausgabe (JSON).
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        clickup: ClickUpArgs,
    },
    /// Importiert alle Aufgaben nach Zunder (idempotent, erneuter Lauf aktualisiert).
    Run {
        /// Zuordnungsdatei (von `plan` erzeugt, danach bearbeitet).
        #[arg(long, default_value = "mapping.toml")]
        mapping: PathBuf,
        /// URL des Zunder-Servers.
        #[arg(long, env = "ZUNDER_URL")]
        server: Option<String>,
        /// Zunder-API-Token (zt_…).
        #[arg(long, env = "ZUNDER_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Nur zählen, nichts schreiben.
        #[arg(long)]
        dry_run: bool,
        /// Bei abgelehnten Operationen weitermachen statt abzubrechen.
        #[arg(long)]
        keep_going: bool,
        /// Operationen je Push.
        #[arg(long, default_value_t = DEFAULT_BATCH_SIZE)]
        batch_size: usize,
        /// Persönlicher ClickUp-API-Token.
        #[arg(long = "clickup-token", env = "CLICKUP_TOKEN", hide_env_values = true)]
        clickup_token: String,
        #[arg(long, env = "CLICKUP_API_URL", default_value = DEFAULT_BASE_URL, hide = true)]
        clickup_url: String,
    },
    /// Sichert alle ClickUp-Daten als Roh-JSON (inkl. Zeiteinträgen).
    Backup {
        /// Basisverzeichnis; die Sicherung landet in einem Unterordner mit Zeitstempel.
        /// Standard: ~/zunder-import-backup
        #[arg(long)]
        out: Option<PathBuf>,
        /// Auch innerhalb eines Git-Repositorys schreiben.
        #[arg(long)]
        force: bool,
        /// Zeiteinträge ab diesem Datum (JJJJ-MM-TT).
        #[arg(long, default_value = "2017-01-01")]
        since: NaiveDate,
        /// Zeitfenster je Abfrage der Zeiteinträge in Tagen.
        #[arg(long, default_value_t = 30)]
        window_days: u32,
        #[command(flatten)]
        clickup: ClickUpArgs,
    },
}

#[derive(Debug, Args)]
struct ClickUpArgs {
    /// Persönlicher ClickUp-API-Token.
    #[arg(
        long = "token",
        visible_alias = "clickup-token",
        env = "CLICKUP_TOKEN",
        hide_env_values = true
    )]
    token: String,
    #[arg(long, env = "CLICKUP_API_URL", default_value = DEFAULT_BASE_URL, hide = true)]
    clickup_url: String,
}

impl ClickUpArgs {
    fn client(&self) -> HttpClickUp {
        HttpClickUp::new(&self.token, &self.clickup_url)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Fehler: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn execute(command: Command) -> Result<()> {
    match command {
        Command::Plan {
            out,
            team,
            force,
            clickup,
        } => {
            if out.exists() && !force {
                bail!(
                    "{} existiert bereits (überschreiben mit --force)",
                    out.display()
                );
            }
            let today = Local::now().format("%Y-%m-%d").to_string();
            let proposal = plan::plan(&clickup.client(), team.as_deref(), &today).await?;
            std::fs::write(&out, &proposal.toml)
                .with_context(|| format!("{} nicht schreibbar", out.display()))?;
            let m = &proposal.mapping;
            println!(
                "Zuordnung geschrieben: {} ({} Bereiche, {} Projekte, {} Listen, {} offene Status)",
                out.display(),
                m.areas.len(),
                m.projects.len(),
                m.lists.len(),
                proposal.open_statuses
            );
            println!("Nächster Schritt: Datei prüfen, dann `zunder-import report`.");
        }
        Command::Report {
            mapping,
            json,
            clickup,
        } => {
            let resolved = Mapping::load(&mapping)?.resolve()?;
            let loaded = report::load(&clickup.client(), &resolved).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&loaded.report)?);
            } else {
                print!("{}", report::render_text(&loaded.report));
            }
        }
        Command::Run {
            mapping,
            server,
            token,
            dry_run,
            keep_going,
            batch_size,
            clickup_token,
            clickup_url,
        } => {
            let resolved = Mapping::load(&mapping)?.resolve()?;
            let zunder = match (server, token) {
                (Some(server), Some(token)) => Some(ZunderClient::new(&server, token)),
                (None, None) if dry_run => None,
                _ if dry_run => bail!("--server und --token nur gemeinsam angeben"),
                _ => bail!("--server und --token (bzw. ZUNDER_URL, ZUNDER_TOKEN) sind nötig"),
            };
            let api = HttpClickUp::new(clickup_token, &clickup_url);
            let loaded = report::load(&api, &resolved).await?;
            let options = RunOptions {
                dry_run,
                keep_going,
                batch_size,
            };
            let summary = run::run(&loaded.conversion, zunder.as_ref(), &options).await?;
            print!("{}", run::render_summary(&summary));
        }
        Command::Backup {
            out,
            force,
            since,
            window_days,
            clickup,
        } => {
            let base_dir = match out {
                Some(dir) => dir,
                None => backup::default_base_dir()
                    .context("HOME ist nicht gesetzt, bitte --out angeben")?,
            };
            let since_ms = since
                .and_hms_opt(0, 0, 0)
                .expect("midnight exists")
                .and_utc()
                .timestamp_millis();
            let options = BackupOptions {
                base_dir,
                force,
                since_ms,
                until_ms: Utc::now().timestamp_millis(),
                window_days,
                stamp: Local::now().format("%Y-%m-%d_%H-%M-%S").to_string(),
            };
            let root = backup::backup(&clickup.client(), &options).await?;
            println!("Sicherung geschrieben: {}", root.display());
        }
    }
    Ok(())
}
