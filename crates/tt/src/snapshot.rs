use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tt_core::lane::{LanePaths, RepoManifest, WorkspaceManifest, read_toml, write_toml};
use tt_core::snapshot::{
    PromptBundle, SnapshotConfigRef, SnapshotContextSummary, SnapshotConversationSelection,
    SnapshotDiff, SnapshotLogEntry, SnapshotRecord, SnapshotSkillSelection, SnapshotStatus,
    SnapshotTurn, SnapshotTurnRange, SnapshotTurnRecord, SnapshotWorkspaceBinding,
};
use tt_skills::SkillApplyArgs;
use ttd::TTIpcClient;

#[derive(Debug, Clone, Args)]
pub struct SnapshotScopeArgs {
    #[arg(long)]
    pub lane: String,
    #[arg(long)]
    pub repo: String,
    #[arg(long)]
    pub workspace: String,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SnapshotSelectionArgs {
    #[arg(long = "include-turn-range")]
    pub include_turn_range: Vec<String>,
    #[arg(long = "exclude-turn-range")]
    pub exclude_turn_range: Vec<String>,
    #[arg(long = "include-turn")]
    pub include_turn: Vec<String>,
    #[arg(long = "exclude-turn")]
    pub exclude_turn: Vec<String>,
    #[arg(long = "pin-turn")]
    pub pin_turn: Vec<String>,
    #[arg(long = "pin-fact")]
    pub pin_fact: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotCreateArgs {
    #[command(flatten)]
    pub scope: SnapshotScopeArgs,
    #[arg(long)]
    pub thread: String,
    #[command(flatten)]
    pub selection: SnapshotSelectionArgs,
    #[arg(long)]
    pub summary: Option<String>,
    #[arg(long = "skill")]
    pub skills: Vec<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long)]
    pub created_by: Option<String>,
    #[arg(long)]
    pub note: Option<String>,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(long)]
    pub worktree: Option<PathBuf>,
    #[arg(long)]
    pub commit: Option<String>,
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotForkArgs {
    #[arg(long = "from")]
    pub from_snapshot: String,
    #[arg(long = "created-by")]
    pub created_by: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotRestoreArgs {
    #[arg(long = "snapshot")]
    pub snapshot_id: String,
    #[arg(long, default_value_t = false)]
    pub bind: bool,
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotDiffArgs {
    #[arg(long = "from")]
    pub from_snapshot: String,
    #[arg(long = "to")]
    pub to_snapshot: String,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotPruneArgs {
    #[arg(long = "snapshot")]
    pub snapshots: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotListArgs {
    #[arg(long)]
    pub lane: Option<String>,
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long)]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SnapshotGetArgs {
    #[arg(long = "snapshot")]
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ContextIncludeArgs {
    #[arg(long = "from")]
    pub from_snapshot: String,
    #[command(flatten)]
    pub selection: SnapshotSelectionArgs,
    #[arg(long = "summary")]
    pub summary: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long = "created-by")]
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ContextExcludeArgs {
    #[arg(long = "from")]
    pub from_snapshot: String,
    #[command(flatten)]
    pub selection: SnapshotSelectionArgs,
    #[arg(long = "summary")]
    pub summary: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long = "created-by")]
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ContextPinArgs {
    #[arg(long = "from")]
    pub from_snapshot: String,
    #[arg(long = "pin-turn")]
    pub pin_turn: Vec<String>,
    #[arg(long = "pin-fact")]
    pub pin_fact: Vec<String>,
    #[arg(long = "created-by")]
    pub created_by: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ContextSummarizeArgs {
    #[arg(long = "from")]
    pub from_snapshot: String,
    #[arg(long)]
    pub summary: String,
    #[arg(long = "source-turn")]
    pub source_turn: Vec<String>,
    #[arg(long = "created-by")]
    pub created_by: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct WorkspaceBindArgs {
    #[command(flatten)]
    pub scope: SnapshotScopeArgs,
    #[arg(long = "snapshot")]
    pub snapshot_id: Option<String>,
    #[arg(long = "commit")]
    pub commit: Option<String>,
    #[arg(long = "worktree")]
    pub worktree: Option<PathBuf>,
    #[arg(long = "branch")]
    pub branch: Option<String>,
    #[arg(long = "thread")]
    pub thread: Option<String>,
    #[arg(long, default_value_t = false)]
    pub canonical: bool,
}

#[derive(Debug, Clone, Args)]
pub struct WorkspacePromoteArgs {
    #[command(flatten)]
    pub scope: SnapshotScopeArgs,
    #[arg(long = "snapshot")]
    pub snapshot_id: String,
    #[arg(long = "commit")]
    pub commit: Option<String>,
    #[arg(long = "worktree")]
    pub worktree: Option<PathBuf>,
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum SnapshotCommand {
    Create(SnapshotCreateArgs),
    Fork(SnapshotForkArgs),
    Restore(SnapshotRestoreArgs),
    Diff(SnapshotDiffArgs),
    Prune(SnapshotPruneArgs),
    Compact(ContextSummarizeArgs),
    List(SnapshotListArgs),
    Get(SnapshotGetArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ContextCommand {
    Include(ContextIncludeArgs),
    Exclude(ContextExcludeArgs),
    Pin(ContextPinArgs),
    Summarize(ContextSummarizeArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum WorkspaceCommand {
    Bind(WorkspaceBindArgs),
    Promote(WorkspacePromoteArgs),
}

#[derive(Debug, Clone)]
struct SnapshotStore {
    lane_paths: LanePaths,
    repo_org: String,
    repo_name: String,
    workspace_slug: String,
}

impl SnapshotStore {
    fn new(scope: &SnapshotScopeArgs, paths: &tt_core::AppPaths) -> Result<Self> {
        let (repo_org, repo_name) = parse_repo_spec(&scope.repo)?;
        let lane_slug = LanePaths::slugify(&scope.lane);
        let lane_paths = LanePaths::from_app_paths(paths, &lane_slug);
        Ok(Self {
            lane_paths,
            repo_org,
            repo_name,
            workspace_slug: LanePaths::slugify(&scope.workspace),
        })
    }

    fn workspace_root(&self) -> PathBuf {
        self.lane_paths
            .workspace_root(&self.repo_org, &self.repo_name, &self.workspace_slug)
    }

    fn workspace_manifest_path(&self) -> PathBuf {
        self.lane_paths
            .workspace_manifest_file(&self.repo_org, &self.repo_name, &self.workspace_slug)
    }

    fn snapshot_log_path(&self) -> PathBuf {
        self.lane_paths
            .workspace_snapshot_log_file(&self.repo_org, &self.repo_name, &self.workspace_slug)
    }

    fn snapshot_db_path(&self) -> PathBuf {
        self.lane_paths
            .workspace_snapshot_db_file(&self.repo_org, &self.repo_name, &self.workspace_slug)
    }

    fn turn_log_path(&self) -> PathBuf {
        self.lane_paths
            .workspace_turn_log_file(&self.repo_org, &self.repo_name, &self.workspace_slug)
    }

    fn ensure_dirs(&self) -> Result<()> {
        self.lane_paths.ensure()?;
        fs::create_dir_all(self.lane_paths.repo_root(&self.repo_org, &self.repo_name))
            .with_context(|| {
                format!(
                    "create {}",
                    self.lane_paths.repo_root(&self.repo_org, &self.repo_name).display()
                )
            })?;
        fs::create_dir_all(self.workspace_root())
            .with_context(|| format!("create {}", self.workspace_root().display()))?;
        Ok(())
    }

    fn append_turn_history(&self, thread: &tt_core::ipc::ThreadView) -> Result<()> {
        self.ensure_dirs()?;
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.turn_log_path())
            .with_context(|| format!("open {}", self.turn_log_path().display()))?;
        for turn in &thread.turns {
            let entry = SnapshotTurnRecord {
                thread_id: thread.summary.id.clone(),
                turn: turn.clone(),
                recorded_at: Utc::now().to_rfc3339(),
            };
            writeln!(
                log,
                "{}",
                serde_json::to_string(&entry).expect("serialize turn log entry")
            )
            .with_context(|| format!("write {}", self.turn_log_path().display()))?;
        }
        Ok(())
    }

    fn load_turn_history(&self) -> Result<Vec<tt_core::ipc::TurnView>> {
        let path = self.turn_log_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut turns = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: SnapshotTurnRecord =
                serde_json::from_str(&line).with_context(|| format!("decode {}", path.display()))?;
            turns.push(entry.turn);
        }
        Ok(turns)
    }

    fn ensure_sqlite(&self) -> Result<Connection> {
        self.ensure_dirs()?;
        let connection = Connection::open(self.snapshot_db_path())
            .with_context(|| format!("open {}", self.snapshot_db_path().display()))?;
        connection
            .execute_batch(
                "pragma journal_mode = wal;
                 pragma synchronous = full;
                 create table if not exists snapshots (
                    snapshot_id text primary key,
                    parent_snapshot_id text,
                    lane_slug text not null,
                    repo_org text not null,
                    repo_name text not null,
                    workspace_slug text not null,
                    thread_id text not null,
                    status text not null,
                    tags_json text not null,
                    snapshot_json text not null,
                    prompt_hash text not null,
                    lineage_hash text not null,
                    created_at text not null,
                    recorded_at text not null
                 );
                 create index if not exists idx_snapshots_workspace
                    on snapshots(lane_slug, repo_org, repo_name, workspace_slug, created_at);
                 create index if not exists idx_snapshots_parent
                    on snapshots(parent_snapshot_id);",
            )
            .with_context(|| format!("migrate {}", self.snapshot_db_path().display()))?;
        Ok(connection)
    }

    fn record_path_for_snapshot(&self, snapshot_id: &str) -> PathBuf {
        self.workspace_root().join(format!("{snapshot_id}.json"))
    }

    fn load_workspace_manifest(&self) -> Result<Option<WorkspaceManifest>> {
        let path = self.workspace_manifest_path();
        if !path.exists() {
            return Ok(None);
        }
        Ok(read_toml(&path)?)
    }

    fn save_workspace_manifest(&self, manifest: &WorkspaceManifest) -> Result<()> {
        Ok(write_toml(&self.workspace_manifest_path(), manifest)?)
    }

    fn load_snapshot(&self, snapshot_id: &str) -> Result<Option<SnapshotRecord>> {
        let log_path = self.snapshot_log_path();
        let db_path = self.snapshot_db_path();
        if !log_path.exists() && !db_path.exists() {
            return Ok(None);
        }
        let connection = self.ensure_sqlite()?;
        let from_sqlite = connection
            .query_row(
                "select snapshot_json from snapshots where snapshot_id = ?1",
                [snapshot_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .with_context(|| format!("query snapshot {snapshot_id}"))?;
        if let Some(json) = from_sqlite {
            let snapshot = serde_json::from_str(&json)
                .with_context(|| format!("decode snapshot {snapshot_id}"))?;
            return Ok(Some(snapshot));
        }

        if !log_path.exists() {
            return Ok(None);
        }
        let file = File::open(&log_path).with_context(|| format!("open {}", log_path.display()))?;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: SnapshotLogEntry =
                serde_json::from_str(&line).with_context(|| format!("decode {}", log_path.display()))?;
            if entry.snapshot.snapshot_id == snapshot_id {
                return Ok(Some(entry.snapshot));
            }
        }
        Ok(None)
    }

    fn load_all_snapshots(&self) -> Result<Vec<SnapshotRecord>> {
        let log_path = self.snapshot_log_path();
        if !log_path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&log_path).with_context(|| format!("open {}", log_path.display()))?;
        let mut snapshots = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: SnapshotLogEntry = serde_json::from_str(&line)
                .with_context(|| format!("decode {}", log_path.display()))?;
            snapshots.push(entry.snapshot);
        }
        Ok(snapshots)
    }

    fn append_snapshot(&self, snapshot: &SnapshotRecord) -> Result<()> {
        self.ensure_dirs()?;
        let entry = SnapshotLogEntry {
            seq: self.next_sequence()?,
            event_kind: "snapshot_record".to_string(),
            snapshot: snapshot.clone(),
            recorded_at: Utc::now().to_rfc3339(),
        };
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.snapshot_log_path())
            .with_context(|| format!("open {}", self.snapshot_log_path().display()))?;
        writeln!(
            log,
            "{}",
            serde_json::to_string(&entry).expect("serialize snapshot log entry")
        )
        .with_context(|| format!("write {}", self.snapshot_log_path().display()))?;

        let connection = self.ensure_sqlite()?;
        connection
            .execute(
                "insert into snapshots (
                    snapshot_id, parent_snapshot_id, lane_slug, repo_org, repo_name,
                    workspace_slug, thread_id, status, tags_json, snapshot_json,
                    prompt_hash, lineage_hash, created_at, recorded_at
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 on conflict(snapshot_id) do update set
                    parent_snapshot_id = excluded.parent_snapshot_id,
                    lane_slug = excluded.lane_slug,
                    repo_org = excluded.repo_org,
                    repo_name = excluded.repo_name,
                    workspace_slug = excluded.workspace_slug,
                    thread_id = excluded.thread_id,
                    status = excluded.status,
                    tags_json = excluded.tags_json,
                    snapshot_json = excluded.snapshot_json,
                    prompt_hash = excluded.prompt_hash,
                    lineage_hash = excluded.lineage_hash,
                    created_at = excluded.created_at,
                    recorded_at = excluded.recorded_at",
                params![
                    snapshot.snapshot_id,
                    snapshot.parent_snapshot_id,
                    snapshot.workspace.lane_slug,
                    snapshot.workspace.repo_org,
                    snapshot.workspace.repo_name,
                    snapshot.workspace.workspace_slug,
                    snapshot.conversation.thread_id,
                    format!("{:?}", snapshot.status).to_lowercase(),
                    serde_json::to_string(&snapshot.tags).expect("serialize tags"),
                    serde_json::to_string(snapshot).expect("serialize snapshot"),
                    snapshot.prompt_hash,
                    snapshot.lineage_hash,
                    snapshot.created_at,
                    Utc::now().to_rfc3339(),
                ],
            )
            .with_context(|| format!("upsert {}", self.snapshot_db_path().display()))?;
        Ok(())
    }

    fn next_sequence(&self) -> Result<u64> {
        let connection = self.ensure_sqlite()?;
        let seq = connection
            .query_row("select coalesce(max(rowid), 0) + 1 from snapshots", [], |row| {
                row.get::<_, u64>(0)
            })
            .context("calculate next snapshot sequence")?;
        Ok(seq)
    }

    fn reindex(&self) -> Result<()> {
        let snapshots = self.load_all_snapshots()?;
        let connection = self.ensure_sqlite()?;
        connection
            .execute("delete from snapshots", [])
            .context("clear snapshot mirror")?;
        for snapshot in snapshots {
            connection
                .execute(
                    "insert into snapshots (
                        snapshot_id, parent_snapshot_id, lane_slug, repo_org, repo_name,
                        workspace_slug, thread_id, status, tags_json, snapshot_json,
                        prompt_hash, lineage_hash, created_at, recorded_at
                     ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        snapshot.snapshot_id,
                        snapshot.parent_snapshot_id,
                        snapshot.workspace.lane_slug,
                        snapshot.workspace.repo_org,
                        snapshot.workspace.repo_name,
                        snapshot.workspace.workspace_slug,
                        snapshot.conversation.thread_id,
                        format!("{:?}", snapshot.status).to_lowercase(),
                        serde_json::to_string(&snapshot.tags).expect("serialize tags"),
                        serde_json::to_string(&snapshot).expect("serialize snapshot"),
                        snapshot.prompt_hash,
                        snapshot.lineage_hash,
                        snapshot.created_at,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .context("reindex snapshot mirror")?;
        }
        Ok(())
    }

    fn list_snapshots(&self) -> Result<Vec<SnapshotRecord>> {
        self.load_all_snapshots()
    }

    fn prune(&self, snapshot_ids: &[String], force: bool) -> Result<usize> {
        let mut snapshots = self.load_all_snapshots()?;
        let prune_set: BTreeSet<String> = snapshot_ids.iter().cloned().collect();
        if prune_set.is_empty() {
            return Ok(0);
        }
        let has_children = snapshots
            .iter()
            .filter_map(|snapshot| snapshot.parent_snapshot_id.as_ref())
            .any(|parent| prune_set.contains(parent));
        if has_children && !force {
            bail!("refusing to prune snapshots that still have children; pass --force to rewrite the log");
        }
        let before = snapshots.len();
        snapshots.retain(|snapshot| !prune_set.contains(&snapshot.snapshot_id));
        let log_path = self.snapshot_log_path();
        let mut log = File::create(&log_path).with_context(|| format!("rewrite {}", log_path.display()))?;
        for (seq, snapshot) in snapshots.iter().enumerate() {
            let entry = SnapshotLogEntry {
                seq: (seq + 1) as u64,
                event_kind: "snapshot_record".to_string(),
                snapshot: snapshot.clone(),
                recorded_at: Utc::now().to_rfc3339(),
            };
            writeln!(
                log,
                "{}",
                serde_json::to_string(&entry).expect("serialize snapshot log entry")
            )
            .with_context(|| format!("write {}", log_path.display()))?;
        }
        self.reindex()?;
        Ok(before - snapshots.len())
    }
}

#[derive(Debug, Clone)]
struct SnapshotService {
    paths: tt_core::AppPaths,
}

impl SnapshotService {
    fn new(paths: tt_core::AppPaths) -> Self {
        Self { paths }
    }

    fn store(&self, scope: &SnapshotScopeArgs) -> Result<SnapshotStore> {
        SnapshotStore::new(scope, &self.paths)
    }

    async fn thread_history(&self, thread_id: &str) -> Result<tt_core::ipc::ThreadView> {
        let client = TTIpcClient::connect(&self.paths)
            .await
            .context("connect to TT daemon for snapshot history")?;
        let response = client
            .thread_read_history(&tt_core::ipc::ThreadReadHistoryRequest {
                thread_id: thread_id.to_string(),
            })
            .await
            .context("read thread history for snapshot")?;
        Ok(response.thread)
    }

    async fn create_snapshot(&self, args: &SnapshotCreateArgs) -> Result<SnapshotRecord> {
        let thread = self.thread_history(&args.thread).await?;
        let store = self.store(&args.scope)?;
        store.append_turn_history(&thread)?;
        let manifest = store.load_workspace_manifest()?.unwrap_or_else(|| {
            let lane_slug = LanePaths::slugify(&args.scope.lane);
            let lane_paths = LanePaths::from_app_paths(&self.paths, &lane_slug);
            WorkspaceManifest::new(
                lane_slug.clone(),
                lane_paths.root.display().to_string(),
                lane_paths
                    .repo_root(&store.repo_org, &store.repo_name)
                    .display()
                    .to_string(),
                args.scope.workspace.clone(),
                LanePaths::slugify(&args.scope.workspace),
                format!("{}/{}", store.repo_org, store.repo_name),
                store.workspace_root().display().to_string(),
                store.workspace_root().join("worktree").display().to_string(),
                store.workspace_root().join("worktree").display().to_string(),
                store.workspace_root().join("runtime").display().to_string(),
                store.workspace_root().join("home").display().to_string(),
                args.branch.clone().unwrap_or_else(|| "detached".to_string()),
            )
        });
        let workspace = workspace_binding_from_store(
            &store,
            &manifest,
            args.worktree.as_deref(),
            args.commit.as_deref(),
            args.branch.as_deref(),
        )?;
        let mut selection = build_selection(&thread.turns, &args.selection, &args.thread);
        let skills = SnapshotSkillSelection {
            skill_ids: args.skills.clone(),
            skill_versions: BTreeMap::new(),
            loaded_skill_ids: args.skills.clone(),
        };
        let config = SnapshotConfigRef {
            model: args.model.clone(),
            cwd: args
                .cwd
                .as_ref()
                .map(|path| path.display().to_string()),
            ..SnapshotConfigRef::default()
        };
        let summary = args.summary.as_ref().map(|summary| SnapshotContextSummary {
            summary_text: summary.clone(),
            source_turn_ids: selection.included_turn_ids.clone(),
            summary_version: 1,
            generated_at: Utc::now().to_rfc3339(),
        });
        if summary.is_some() {
            selection.summary_source_turn_ids = selection.included_turn_ids.clone();
        }
        let prompt_bundle = build_prompt_bundle(&workspace, &selection, &skills, &config, &summary);
        let snapshot = SnapshotRecord {
            snapshot_id: generate_snapshot_id(),
            parent_snapshot_id: None,
            tags: args.tags.clone(),
            status: SnapshotStatus::Active,
            created_at: Utc::now().to_rfc3339(),
            created_by: args.created_by.clone().unwrap_or_else(|| "tt".to_string()),
            workspace,
            conversation: selection,
            skills,
            config,
            summary,
            prompt_hash: prompt_bundle.prompt_hash.clone(),
            lineage_hash: hash_lineage(None, &prompt_bundle.prompt_hash),
            note: args.note.clone(),
        };
        store.append_snapshot(&snapshot)?;
        if args.worktree.is_some() || args.commit.is_some() || args.branch.is_some() {
            update_workspace_manifest_for_binding(&store, &manifest, &snapshot, true)?;
        }
        Ok(snapshot)
    }

    async fn fork_snapshot(&self, args: &SnapshotForkArgs) -> Result<SnapshotRecord> {
        let base = self.load_snapshot(&args.from_snapshot).await?;
        let store = self.store_for_record(&base)?;
        let _turns = store.load_turn_history()?;
        let mut fork = base.clone();
        fork.snapshot_id = generate_snapshot_id();
        fork.parent_snapshot_id = Some(base.snapshot_id.clone());
        fork.created_at = Utc::now().to_rfc3339();
        fork.created_by = args.created_by.clone().unwrap_or_else(|| "tt".to_string());
        fork.tags.extend(args.tags.clone());
        fork.note = args.note.clone();
        fork.lineage_hash = hash_lineage(Some(&base.lineage_hash), &fork.prompt_hash);
        fork.tags.sort();
        fork.tags.dedup();
        store.append_snapshot(&fork)?;
        Ok(fork)
    }

    async fn load_snapshot(&self, snapshot_id: &str) -> Result<SnapshotRecord> {
        for store in self.discover_stores()? {
            if let Some(snapshot) = store.load_snapshot(snapshot_id)? {
                return Ok(snapshot);
            }
        }
        bail!("snapshot `{snapshot_id}` not found")
    }

    fn discover_stores(&self) -> Result<Vec<SnapshotStore>> {
        let mut stores = Vec::new();
        let lanes_root = self.paths.data_dir.join("lanes");
        if !lanes_root.exists() {
            return Ok(stores);
        }
        for lane_entry in fs::read_dir(&lanes_root)
            .with_context(|| format!("read {}", lanes_root.display()))?
        {
            let lane_entry = lane_entry?;
            if !lane_entry.file_type()?.is_dir() {
                continue;
            }
            let lane_path = lane_entry.path();
            let repos_root = lane_path.join("repos");
            if !repos_root.exists() {
                continue;
            }
            for org_entry in fs::read_dir(&repos_root)
                .with_context(|| format!("read {}", repos_root.display()))?
            {
                let org_entry = org_entry?;
                if !org_entry.file_type()?.is_dir() {
                    continue;
                }
                let org_path = org_entry.path();
                for repo_entry in fs::read_dir(&org_path)
                    .with_context(|| format!("read {}", org_path.display()))?
                {
                    let repo_entry = repo_entry?;
                    if !repo_entry.file_type()?.is_dir() {
                        continue;
                    }
                    let repo_name = repo_entry.file_name().to_string_lossy().to_string();
                    let worktrees_root = lane_path
                        .join("worktrees")
                        .join(org_entry.file_name())
                        .join(&repo_name);
                    if !worktrees_root.exists() {
                        continue;
                    }
                    for workspace_entry in fs::read_dir(&worktrees_root)
                        .with_context(|| format!("read {}", worktrees_root.display()))?
                    {
                        let workspace_entry = workspace_entry?;
                        if !workspace_entry.file_type()?.is_dir() {
                            continue;
                        }
                        stores.push(SnapshotStore {
                            lane_paths: LanePaths {
                                root: lane_path.clone(),
                                manifest_file: lane_path.join("lane.toml"),
                                shared_dir: lane_path.join("shared"),
                                shared_home_dir: lane_path.join("shared/home"),
                                shared_tt_dir: lane_path.join("shared/home/.tt"),
                                shared_codex_dir: lane_path.join("shared/home/.codex"),
                                repos_dir: repos_root.clone(),
                                worktrees_dir: lane_path.join("worktrees"),
                                runtime_dir: lane_path.join("runtime"),
                            },
                            repo_org: org_entry.file_name().to_string_lossy().to_string(),
                            repo_name: repo_name.clone(),
                            workspace_slug: workspace_entry.file_name().to_string_lossy().to_string(),
                        });
                    }
                }
            }
        }
        Ok(stores)
    }

    fn store_for_record(&self, record: &SnapshotRecord) -> Result<SnapshotStore> {
        SnapshotStore::new(
            &SnapshotScopeArgs {
                lane: record.workspace.lane_label.clone(),
                repo: format!("{}/{}", record.workspace.repo_org, record.workspace.repo_name),
                workspace: record.workspace.workspace_slug.clone(),
            },
            &self.paths,
        )
    }

    async fn restore_snapshot(&self, args: &SnapshotRestoreArgs) -> Result<PromptBundle> {
        let snapshot = self.load_snapshot(&args.snapshot_id).await?;
        let prompt = build_prompt_bundle(
            &snapshot.workspace,
            &snapshot.conversation,
            &snapshot.skills,
            &snapshot.config,
            &snapshot.summary,
        );
        if let Some(out) = args.out.as_ref() {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
            }
            fs::write(out, &prompt.rendered_prompt)
                .with_context(|| format!("write {}", out.display()))?;
        } else {
            println!("{}", prompt.rendered_prompt);
        }
        if args.bind {
            let store = self.store_for_record(&snapshot)?;
            let manifest = store.load_workspace_manifest()?.unwrap_or_else(|| {
                WorkspaceManifest::new(
                    snapshot.workspace.lane_slug.clone(),
                    store.lane_paths.root.display().to_string(),
                    snapshot.workspace.repo_root_path.clone(),
                    snapshot.workspace.workspace_slug.clone(),
                    snapshot.workspace.workspace_slug.clone(),
                    format!("{}/{}", snapshot.workspace.repo_org, snapshot.workspace.repo_name),
                    store.workspace_root().display().to_string(),
                    snapshot.workspace.worktree_path.clone(),
                    snapshot.workspace.worktree_path.clone(),
                    store.workspace_root().join("runtime").display().to_string(),
                    store.workspace_root().join("home").display().to_string(),
                    snapshot.workspace.branch_name.clone().unwrap_or_default(),
                )
            });
            update_workspace_manifest_for_binding(&store, &manifest, &snapshot, snapshot.workspace.canonical)?;
        }
        Ok(prompt)
    }

    async fn diff_snapshots(&self, args: &SnapshotDiffArgs) -> Result<SnapshotDiff> {
        let left = self.load_snapshot(&args.from_snapshot).await?;
        let right = self.load_snapshot(&args.to_snapshot).await?;
        Ok(diff_snapshots(&left, &right))
    }

    fn update_context(
        &self,
        base: &SnapshotRecord,
        turns: &[tt_core::ipc::TurnView],
        selection: SnapshotSelectionArgs,
        summary: Option<String>,
        created_by: Option<String>,
        tags: Vec<String>,
        mode: ContextMutationMode,
    ) -> Result<SnapshotRecord> {
        let mut next = base.clone();
        next.snapshot_id = generate_snapshot_id();
        next.parent_snapshot_id = Some(base.snapshot_id.clone());
        next.created_at = Utc::now().to_rfc3339();
        next.created_by = created_by.unwrap_or_else(|| "tt".to_string());
        next.tags.extend(tags);
        next.conversation = mutate_selection(turns, &base.conversation, selection, summary.clone(), mode)?;
        if let Some(summary_text) = summary {
            next.summary = Some(SnapshotContextSummary {
                summary_text,
                source_turn_ids: next.conversation.summary_source_turn_ids.clone(),
                summary_version: base.summary.as_ref().map(|value| value.summary_version + 1).unwrap_or(1),
                generated_at: Utc::now().to_rfc3339(),
            });
        }
        next.prompt_hash = compute_prompt_hash(&next.workspace, &next.conversation, &next.skills, &next.config, &next.summary);
        next.lineage_hash = hash_lineage(Some(&base.lineage_hash), &next.prompt_hash);
        next.tags.sort();
        next.tags.dedup();
        Ok(next)
    }

    fn prune_snapshots(&self, snapshot_ids: &[String], force: bool) -> Result<usize> {
        let stores = self.discover_stores()?;
        let mut pruned = 0usize;
        for store in stores {
            let store_snapshots = store.list_snapshots()?;
            let relevant: Vec<String> = snapshot_ids
                .iter()
                .filter(|id| store_snapshots.iter().any(|snapshot| &snapshot.snapshot_id == *id))
                .cloned()
                .collect();
            if relevant.is_empty() {
                continue;
            }
            pruned += store.prune(&relevant, force)?;
        }
        Ok(pruned)
    }
}

#[derive(Debug, Clone, Copy)]
enum ContextMutationMode {
    Include,
    Exclude,
    Pin,
    Compact,
}

fn parse_repo_spec(spec: &str) -> Result<(String, String)> {
    let mut parts = spec.splitn(2, '/');
    let org = parts.next().unwrap_or_default().trim();
    let repo = parts.next().unwrap_or_default().trim();
    if org.is_empty() || repo.is_empty() {
        bail!("repo must be formatted as org/repo");
    }
    Ok((org.to_string(), repo.to_string()))
}

fn generate_snapshot_id() -> String {
    format!("snapshot-{}", uuid::Uuid::now_v7())
}

fn hash_json<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_vec(value).expect("serialize for hash");
    let mut hasher = Sha256::new();
    hasher.update(json);
    format!("{:x}", hasher.finalize())
}

fn hash_lineage(parent: Option<&str>, prompt_hash: &str) -> String {
    let mut hasher = Sha256::new();
    if let Some(parent) = parent {
        hasher.update(parent.as_bytes());
    }
    hasher.update(prompt_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn compute_prompt_hash(
    workspace: &SnapshotWorkspaceBinding,
    conversation: &SnapshotConversationSelection,
    skills: &SnapshotSkillSelection,
    config: &SnapshotConfigRef,
    summary: &Option<SnapshotContextSummary>,
) -> String {
    hash_json(&PromptBundle {
        snapshot_id: String::new(),
        prompt_hash: String::new(),
        token_estimate: 0,
        workspace: workspace.clone(),
        conversation: conversation.clone(),
        skills: skills.clone(),
        config: config.clone(),
        pinned_facts: conversation.pinned_facts.clone(),
        included_turn_ids: conversation.included_turn_ids.clone(),
        turns: conversation.selected_turns.clone(),
        summary: summary.clone(),
        rendered_prompt: String::new(),
        assembled_at: String::new(),
    })
}

fn build_prompt_bundle(
    workspace: &SnapshotWorkspaceBinding,
    conversation: &SnapshotConversationSelection,
    skills: &SnapshotSkillSelection,
    config: &SnapshotConfigRef,
    summary: &Option<SnapshotContextSummary>,
) -> PromptBundle {
    let mut rendered = String::new();
    rendered.push_str(&format!("# snapshot {}\n", workspace.workspace_slug));
    rendered.push_str(&format!("lane: {}\n", workspace.lane_slug));
    rendered.push_str(&format!("repo: {}/{}\n", workspace.repo_org, workspace.repo_name));
    rendered.push_str(&format!("workspace: {}\n", workspace.workspace_slug));
    rendered.push_str(&format!("commit: {}\n", workspace.commit_sha));
    rendered.push_str(&format!("thread: {}\n", conversation.thread_id));
    if !conversation.pinned_facts.is_empty() {
        rendered.push_str("pinned_facts:\n");
        for fact in &conversation.pinned_facts {
            rendered.push_str(&format!("- {fact}\n"));
        }
    }
    if let Some(summary) = summary {
        rendered.push_str("summary:\n");
        rendered.push_str(&summary.summary_text);
        rendered.push('\n');
    }
    if !conversation.selected_turns.is_empty() {
        rendered.push_str("turns:\n");
        for turn in &conversation.selected_turns {
            rendered.push_str(&format!("- {} [{}]\n", turn.id, turn.status));
            if !turn.text.trim().is_empty() {
                rendered.push_str(&turn.text);
                rendered.push('\n');
            }
        }
    }
    if !skills.skill_ids.is_empty() {
        rendered.push_str("skills:\n");
        for skill in &skills.skill_ids {
            rendered.push_str(&format!("- {skill}\n"));
        }
    }
    let token_estimate = rendered.len().div_ceil(4);
    let mut bundle = PromptBundle {
        snapshot_id: String::new(),
        prompt_hash: String::new(),
        token_estimate,
        workspace: workspace.clone(),
        conversation: conversation.clone(),
        skills: skills.clone(),
        config: config.clone(),
        pinned_facts: conversation.pinned_facts.clone(),
        included_turn_ids: conversation.included_turn_ids.clone(),
        turns: conversation.selected_turns.clone(),
        summary: summary.clone(),
        rendered_prompt: rendered,
        assembled_at: Utc::now().to_rfc3339(),
    };
    bundle.prompt_hash = compute_prompt_hash(workspace, conversation, skills, config, summary);
    bundle
}

fn build_selection(
    turns: &[tt_core::ipc::TurnView],
    selection: &SnapshotSelectionArgs,
    thread_id: &str,
) -> SnapshotConversationSelection {
    let mut included_turn_ids = Vec::new();
    let mut excluded_turn_ids = Vec::new();
    let mut pinned_turn_ids = Vec::new();
    let mut pinned_facts = selection.pin_fact.clone();
    let mut included_ranges = Vec::new();
    let mut excluded_ranges = Vec::new();

    if selection.include_turn_range.is_empty() && selection.include_turn.is_empty() {
        included_turn_ids.extend(turns.iter().map(|turn| turn.id.clone()));
    } else {
        included_turn_ids.extend(selection.include_turn.iter().cloned());
        for spec in &selection.include_turn_range {
            let range = parse_turn_range(thread_id, spec).unwrap_or_else(|_| SnapshotTurnRange {
                thread_id: thread_id.to_string(),
                start_turn_id: spec.clone(),
                end_turn_id: spec.clone(),
            });
            included_ranges.push(range.clone());
            if let Some(expanded) = expand_range_ids(turns, thread_id, spec) {
                included_turn_ids.extend(expanded);
            }
        }
    }
    excluded_turn_ids.extend(selection.exclude_turn.iter().cloned());
    for spec in &selection.exclude_turn_range {
        let range = parse_turn_range(thread_id, spec).unwrap_or_else(|_| SnapshotTurnRange {
            thread_id: thread_id.to_string(),
            start_turn_id: spec.clone(),
            end_turn_id: spec.clone(),
        });
        excluded_ranges.push(range.clone());
        if let Some(expanded) = expand_range_ids(turns, thread_id, spec) {
            excluded_turn_ids.extend(expanded);
        }
    }
    pinned_turn_ids.extend(selection.pin_turn.iter().cloned());
    if included_turn_ids.is_empty() {
        included_turn_ids.extend(turns.iter().map(|turn| turn.id.clone()));
    }
    included_turn_ids.retain(|turn_id| !excluded_turn_ids.contains(turn_id));
    let history_hash = hash_json(&(included_turn_ids.clone(), excluded_turn_ids.clone(), pinned_turn_ids.clone(), pinned_facts.clone()));
    let selected_turns = materialize_selected_turns_from_ids(turns, &included_turn_ids);

    SnapshotConversationSelection {
        thread_id: thread_id.to_string(),
        included_turn_ranges: included_ranges,
        excluded_turn_ranges: excluded_ranges,
        included_turn_ids,
        excluded_turn_ids,
        pinned_turn_ids,
        pinned_facts,
        summary_source_turn_ids: Vec::new(),
        selected_turns,
        summary: None,
        history_hash,
    }
}

fn materialize_selected_turns_from_ids(
    turns: &[tt_core::ipc::TurnView],
    selected_turn_ids: &[String],
) -> Vec<SnapshotTurn> {
    let mut selected = Vec::new();
    let mut turn_ids = BTreeSet::new();
    if selected_turn_ids.is_empty() {
        turn_ids.extend(turns.iter().map(|turn| turn.id.clone()));
    } else {
        turn_ids.extend(selected_turn_ids.iter().cloned());
    }
    for turn in turns {
        if !turn_ids.contains(&turn.id) {
            continue;
        }
        let text = turn
            .items
            .iter()
            .filter_map(|item| item.text.as_ref())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        selected.push(SnapshotTurn {
            id: turn.id.clone(),
            status: turn.status.clone(),
            text,
        });
    }
    selected
}

fn snapshot_turns_or_fallback(
    turns: Vec<tt_core::ipc::TurnView>,
    selected_turns: &[SnapshotTurn],
    thread_id: &str,
) -> Vec<tt_core::ipc::TurnView> {
    if !turns.is_empty() {
        return turns;
    }
    selected_turns
        .iter()
        .map(|turn| tt_core::ipc::TurnView {
            id: turn.id.clone(),
            status: turn.status.clone(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            latest_plan: None,
            token_usage: None,
            items: vec![tt_core::ipc::ItemView {
                id: format!("{thread_id}-{}", turn.id),
                item_type: "message".to_string(),
                status: None,
                text: Some(turn.text.clone()),
                summary: None,
                payload: None,
                detail_kind: None,
                detail: None,
            }],
        })
        .collect()
}

fn expand_range_ids(
    turns: &[tt_core::ipc::TurnView],
    thread_id: &str,
    spec: &str,
) -> Option<Vec<String>> {
    let range = parse_turn_range(thread_id, spec).ok()?;
    let mut ids = Vec::new();
    let mut recording = false;
    for turn in turns {
        if turn.id == range.start_turn_id {
            recording = true;
        }
        if recording {
            ids.push(turn.id.clone());
        }
        if turn.id == range.end_turn_id {
            break;
        }
    }
    if ids.is_empty() { None } else { Some(ids) }
}

fn parse_turn_range(thread_id: &str, spec: &str) -> Result<SnapshotTurnRange> {
    let (turn_spec_thread, turn_spec) = if let Some((thread, range)) = spec.split_once(':') {
        (thread, range)
    } else {
        (thread_id, spec)
    };
    let (start_turn_id, end_turn_id) = turn_spec
        .split_once("..")
        .ok_or_else(|| anyhow!("turn range must be formatted as start..end or thread:start..end"))?;
    Ok(SnapshotTurnRange {
        thread_id: turn_spec_thread.to_string(),
        start_turn_id: start_turn_id.to_string(),
        end_turn_id: end_turn_id.to_string(),
    })
}

fn mutate_selection(
    turns: &[tt_core::ipc::TurnView],
    base: &SnapshotConversationSelection,
    selection: SnapshotSelectionArgs,
    summary: Option<String>,
    mode: ContextMutationMode,
) -> Result<SnapshotConversationSelection> {
    let mut next = base.clone();
    match mode {
        ContextMutationMode::Include => {
            next.included_turn_ids.extend(selection.include_turn);
            for spec in selection.include_turn_range {
                next.included_turn_ranges.push(parse_turn_range(&base.thread_id, &spec)?);
            }
        }
        ContextMutationMode::Exclude => {
            next.excluded_turn_ids.extend(selection.exclude_turn);
            for spec in selection.exclude_turn_range {
                next.excluded_turn_ranges.push(parse_turn_range(&base.thread_id, &spec)?);
            }
        }
        ContextMutationMode::Pin => {
            next.pinned_turn_ids.extend(selection.pin_turn);
            next.pinned_facts.extend(selection.pin_fact);
        }
        ContextMutationMode::Compact => {
            let mut summarized_turn_ids = selection.include_turn;
            for spec in selection.include_turn_range {
                if let Some(expanded) = expand_range_ids(turns, &base.thread_id, &spec) {
                    summarized_turn_ids.extend(expanded);
                }
            }
            summarized_turn_ids.sort();
            summarized_turn_ids.dedup();
            next.summary_source_turn_ids = summarized_turn_ids.clone();
            next.excluded_turn_ids.extend(summarized_turn_ids.clone());
            next.excluded_turn_ranges.extend(
                summarized_turn_ids
                    .iter()
                    .map(|turn_id| SnapshotTurnRange {
                        thread_id: base.thread_id.clone(),
                        start_turn_id: turn_id.clone(),
                        end_turn_id: turn_id.clone(),
                    }),
            );
        }
    }
    next.included_turn_ids.retain(|turn_id| !next.excluded_turn_ids.contains(turn_id));
    next.pinned_turn_ids.sort();
    next.pinned_turn_ids.dedup();
    next.pinned_facts.sort();
    next.pinned_facts.dedup();
    next.selected_turns = materialize_selected_turns_from_ids(turns, &next.included_turn_ids);
    if let Some(summary_text) = summary {
        if !matches!(mode, ContextMutationMode::Compact) {
            next.summary_source_turn_ids = next.included_turn_ids.clone();
        }
        next.summary = Some(SnapshotContextSummary {
            summary_text,
            source_turn_ids: match mode {
                ContextMutationMode::Compact => next.summary_source_turn_ids.clone(),
                _ => next.included_turn_ids.clone(),
            },
            summary_version: base.summary.as_ref().map(|value| value.summary_version + 1).unwrap_or(1),
            generated_at: Utc::now().to_rfc3339(),
        });
    }
    next.history_hash = hash_json(&(next.included_turn_ids.clone(), next.excluded_turn_ids.clone(), next.pinned_turn_ids.clone(), next.pinned_facts.clone()));
    Ok(next)
}

fn diff_snapshots(left: &SnapshotRecord, right: &SnapshotRecord) -> SnapshotDiff {
    let left_tags: BTreeSet<_> = left.tags.iter().cloned().collect();
    let right_tags: BTreeSet<_> = right.tags.iter().cloned().collect();
    SnapshotDiff {
        left_snapshot_id: left.snapshot_id.clone(),
        right_snapshot_id: right.snapshot_id.clone(),
        changed_fields: {
            let mut fields = Vec::new();
            if left.parent_snapshot_id != right.parent_snapshot_id {
                fields.push("parent_snapshot_id".to_string());
            }
            if left.workspace != right.workspace {
                fields.push("workspace".to_string());
            }
            if left.conversation != right.conversation {
                fields.push("conversation".to_string());
            }
            if left.skills != right.skills {
                fields.push("skills".to_string());
            }
            if left.config != right.config {
                fields.push("config".to_string());
            }
            if left.summary != right.summary {
                fields.push("summary".to_string());
            }
            if left.prompt_hash != right.prompt_hash {
                fields.push("prompt_hash".to_string());
            }
            if left.lineage_hash != right.lineage_hash {
                fields.push("lineage_hash".to_string());
            }
            fields
        },
        added_tags: right_tags.difference(&left_tags).cloned().collect(),
        removed_tags: left_tags.difference(&right_tags).cloned().collect(),
        prompt_hash_changed: left.prompt_hash != right.prompt_hash,
        lineage_changed: left.lineage_hash != right.lineage_hash,
        workspace_changed: left.workspace != right.workspace,
        conversation_changed: left.conversation != right.conversation,
        skills_changed: left.skills != right.skills,
        config_changed: left.config != right.config,
    }
}

fn update_workspace_manifest_for_binding(
    store: &SnapshotStore,
    manifest: &WorkspaceManifest,
    snapshot: &SnapshotRecord,
    canonical: bool,
) -> Result<()> {
    let mut updated = manifest.clone();
    updated.bound_snapshot_id = Some(snapshot.snapshot_id.clone());
    updated.canonical_snapshot_id = if canonical {
        Some(snapshot.snapshot_id.clone())
    } else {
        updated.canonical_snapshot_id.clone()
    };
    updated.bound_commit_sha = Some(snapshot.workspace.commit_sha.clone());
    updated.bound_worktree_path = Some(snapshot.workspace.worktree_path.clone());
    updated.bound_thread_id = Some(snapshot.conversation.thread_id.clone());
    updated.bound_at = Some(Utc::now().to_rfc3339());
    if canonical {
        updated.promoted_at = Some(Utc::now().to_rfc3339());
    }
    store.save_workspace_manifest(&updated)
}

fn workspace_binding_from_store(
    store: &SnapshotStore,
    manifest: &WorkspaceManifest,
    worktree: Option<&Path>,
    commit: Option<&str>,
    branch: Option<&str>,
) -> Result<SnapshotWorkspaceBinding> {
    let worktree_path = worktree
        .map(|path| path.display().to_string())
        .or_else(|| manifest.bound_worktree_path.clone())
        .unwrap_or_else(|| store.workspace_root().join("worktree").display().to_string());
    let commit_sha = commit
        .map(str::to_string)
        .or_else(|| manifest.bound_commit_sha.clone())
        .unwrap_or_else(|| git_head_commit(Path::new(&worktree_path)).unwrap_or_else(|_| "unknown".to_string()));
    Ok(SnapshotWorkspaceBinding {
        lane_label: manifest.label.clone(),
        lane_slug: manifest.lane_slug.clone(),
        repo_org: store.repo_org.clone(),
        repo_name: store.repo_name.clone(),
        workspace_slug: store.workspace_slug.clone(),
        repo_root_path: manifest.repo_root_path.clone(),
        worktree_path,
        branch_name: branch
            .map(str::to_string)
            .or_else(|| manifest.bound_commit_sha.clone().map(|_| manifest.branch_name.clone()))
            .or_else(|| Some(manifest.branch_name.clone())),
        commit_sha,
        dirty_state_hash: Some(git_dirty_hash(Path::new(&manifest.repo_root_path))?),
        canonical: false,
        promoted_from_snapshot_id: manifest.canonical_snapshot_id.clone(),
        bound_at: Utc::now().to_rfc3339(),
    })
}

fn git_head_commit(worktree: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["-C", worktree.to_str().unwrap_or("."), "rev-parse", "HEAD"])
        .output()
        .context("run git rev-parse HEAD")?;
    if !output.status.success() {
        bail!("git rev-parse HEAD failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_dirty_hash(repo_root: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["-C", repo_root.to_str().unwrap_or("."), "status", "--porcelain"])
        .output()
        .context("run git status --porcelain")?;
    if !output.status.success() {
        bail!("git status --porcelain failed");
    }
    Ok(hash_json(&String::from_utf8_lossy(&output.stdout).to_string()))
}

pub async fn snapshot_command(paths: tt_core::AppPaths, command: SnapshotCommand) -> Result<()> {
    let service = SnapshotService::new(paths);
    match command {
        SnapshotCommand::Create(args) => {
            let snapshot = service.create_snapshot(&args).await?;
            print_snapshot_record(&snapshot);
        }
        SnapshotCommand::Fork(args) => {
            let snapshot = service.fork_snapshot(&args).await?;
            print_snapshot_record(&snapshot);
        }
        SnapshotCommand::Restore(args) => {
            let prompt = service.restore_snapshot(&args).await?;
            println!("snapshot_id: {}", prompt.snapshot_id);
            println!("prompt_hash: {}", prompt.prompt_hash);
            println!("token_estimate: {}", prompt.token_estimate);
        }
        SnapshotCommand::Diff(args) => {
            let diff = service.diff_snapshots(&args).await?;
            print_snapshot_diff(&diff);
        }
        SnapshotCommand::Prune(args) => {
            let pruned = service.prune_snapshots(&args.snapshots, args.force)?;
            println!("pruned: {pruned}");
        }
        SnapshotCommand::Compact(args) => {
            let base = service.load_snapshot(&args.from_snapshot).await?;
            let turns = snapshot_turns_or_fallback(
                service.store_for_record(&base)?.load_turn_history()?,
                &base.conversation.selected_turns,
                &base.conversation.thread_id,
            );
            let next = service.update_context(
                &base,
                &turns,
                SnapshotSelectionArgs {
                    include_turn: args.source_turn.clone(),
                    ..SnapshotSelectionArgs::default()
                },
                Some(args.summary),
                args.created_by,
                args.tags,
                ContextMutationMode::Compact,
            )?;
            service.store_for_record(&base)?.append_snapshot(&next)?;
            print_snapshot_record(&next);
        }
        SnapshotCommand::List(args) => {
            let snapshots = if let (Some(lane), Some(repo), Some(workspace)) =
                (args.lane, args.repo, args.workspace)
            {
                let scope = SnapshotScopeArgs {
                    lane,
                    repo,
                    workspace,
                };
                service
                    .store(&scope)?
                    .list_snapshots()
                    .context("list scoped snapshots")?
            } else {
                let mut all = Vec::new();
                for store in service.discover_stores()? {
                    all.extend(store.list_snapshots()?);
                }
                all
            };
            for snapshot in snapshots {
                print_snapshot_summary(&snapshot);
            }
        }
        SnapshotCommand::Get(args) => {
            let snapshot = service.load_snapshot(&args.snapshot_id).await?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
    }
    Ok(())
}

pub async fn context_command(
    paths: tt_core::AppPaths,
    command: ContextCommand,
) -> Result<()> {
    let service = SnapshotService::new(paths);
    match command {
        ContextCommand::Include(args) => {
            let base = service.load_snapshot(&args.from_snapshot).await?;
            let turns = snapshot_turns_or_fallback(
                service.store_for_record(&base)?.load_turn_history()?,
                &base.conversation.selected_turns,
                &base.conversation.thread_id,
            );
            let next = service.update_context(
                &base,
                &turns,
                args.selection,
                args.summary,
                args.created_by,
                args.tags,
                ContextMutationMode::Include,
            )?;
            service.store_for_record(&base)?.append_snapshot(&next)?;
            print_snapshot_record(&next);
        }
        ContextCommand::Exclude(args) => {
            let base = service.load_snapshot(&args.from_snapshot).await?;
            let turns = snapshot_turns_or_fallback(
                service.store_for_record(&base)?.load_turn_history()?,
                &base.conversation.selected_turns,
                &base.conversation.thread_id,
            );
            let next = service.update_context(
                &base,
                &turns,
                args.selection,
                args.summary,
                args.created_by,
                args.tags,
                ContextMutationMode::Exclude,
            )?;
            service.store_for_record(&base)?.append_snapshot(&next)?;
            print_snapshot_record(&next);
        }
        ContextCommand::Pin(args) => {
            let base = service.load_snapshot(&args.from_snapshot).await?;
            let turns = snapshot_turns_or_fallback(
                service.store_for_record(&base)?.load_turn_history()?,
                &base.conversation.selected_turns,
                &base.conversation.thread_id,
            );
            let next = service.update_context(
                &base,
                &turns,
                SnapshotSelectionArgs {
                    pin_turn: args.pin_turn,
                    pin_fact: args.pin_fact,
                    ..SnapshotSelectionArgs::default()
                },
                None,
                args.created_by,
                args.tags,
                ContextMutationMode::Pin,
            )?;
            service.store_for_record(&base)?.append_snapshot(&next)?;
            print_snapshot_record(&next);
        }
        ContextCommand::Summarize(args) => {
            let base = service.load_snapshot(&args.from_snapshot).await?;
            let turns = snapshot_turns_or_fallback(
                service.store_for_record(&base)?.load_turn_history()?,
                &base.conversation.selected_turns,
                &base.conversation.thread_id,
            );
            let next = service.update_context(
                &base,
                &turns,
                SnapshotSelectionArgs {
                    include_turn: args.source_turn.clone(),
                    ..SnapshotSelectionArgs::default()
                },
                Some(args.summary),
                args.created_by,
                args.tags,
                ContextMutationMode::Compact,
            )?;
            service.store_for_record(&base)?.append_snapshot(&next)?;
            print_snapshot_record(&next);
        }
    }
    Ok(())
}

pub async fn workspace_command(
    paths: tt_core::AppPaths,
    command: WorkspaceCommand,
) -> Result<()> {
    let service = SnapshotService::new(paths);
    match command {
        WorkspaceCommand::Bind(args) => {
            let store = service.store(&args.scope)?;
            let manifest = store
                .load_workspace_manifest()?
                .unwrap_or_else(|| create_workspace_manifest(&store, &args.scope));
            let mut updated = manifest.clone();
            if let Some(snapshot_id) = args.snapshot_id.as_ref() {
                let snapshot = service.load_snapshot(snapshot_id).await?;
                updated.bound_snapshot_id = Some(snapshot.snapshot_id.clone());
                updated.canonical_snapshot_id = if args.canonical {
                    Some(snapshot.snapshot_id.clone())
                } else {
                    updated.canonical_snapshot_id.clone()
                };
                updated.bound_commit_sha = Some(snapshot.workspace.commit_sha.clone());
                updated.bound_worktree_path = Some(snapshot.workspace.worktree_path.clone());
                updated.bound_thread_id = Some(snapshot.conversation.thread_id.clone());
                updated.bound_at = Some(Utc::now().to_rfc3339());
            }
            if let Some(commit) = args.commit.as_ref() {
                updated.bound_commit_sha = Some(commit.clone());
            }
            if let Some(worktree) = args.worktree.as_ref() {
                updated.bound_worktree_path = Some(worktree.display().to_string());
            }
            if let Some(thread) = args.thread.as_ref() {
                updated.bound_thread_id = Some(thread.clone());
            }
            if let Some(branch) = args.branch.as_ref() {
                updated.branch_name = branch.clone();
            }
            if args.canonical {
                updated.canonical_snapshot_id = updated.bound_snapshot_id.clone();
                updated.promoted_at = Some(Utc::now().to_rfc3339());
            }
            store.save_workspace_manifest(&updated)?;
            println!("{}", serde_json::to_string_pretty(&updated)?);
        }
        WorkspaceCommand::Promote(args) => {
            let store = service.store(&args.scope)?;
            let snapshot = service.load_snapshot(&args.snapshot_id).await?;
            let manifest = store
                .load_workspace_manifest()?
                .unwrap_or_else(|| create_workspace_manifest(&store, &args.scope));
            let mut updated = manifest.clone();
            updated.canonical_snapshot_id = Some(snapshot.snapshot_id.clone());
            updated.bound_snapshot_id = Some(snapshot.snapshot_id.clone());
            updated.bound_commit_sha = Some(
                args.commit
                    .or_else(|| Some(snapshot.workspace.commit_sha.clone()))
                    .unwrap_or(snapshot.workspace.commit_sha.clone()),
            );
            updated.bound_worktree_path = Some(
                args.worktree
                    .map(|value| value.display().to_string())
                    .unwrap_or_else(|| snapshot.workspace.worktree_path.clone()),
            );
            updated.promoted_at = Some(Utc::now().to_rfc3339());
            store.save_workspace_manifest(&updated)?;
            println!("{}", serde_json::to_string_pretty(&updated)?);
        }
    }
    Ok(())
}

pub async fn skill_apply_command(paths: tt_core::AppPaths, args: &SkillApplyArgs) -> Result<()> {
    let service = SnapshotService::new(paths);
    let snapshot = service.load_snapshot(&args.snapshot_id).await?;
    let mut skills = snapshot.skills.clone();
    if !args.skills.is_empty() {
        skills.skill_ids = args.skills.clone();
        skills.loaded_skill_ids = args.skills.clone();
    }
    let prompt = build_prompt_bundle(
        &snapshot.workspace,
        &snapshot.conversation,
        &skills,
        &snapshot.config,
        &snapshot.summary,
    );
    if let Some(out) = args.out.as_ref() {
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(out, &prompt.rendered_prompt)
            .with_context(|| format!("write {}", out.display()))?;
    } else {
        println!("{}", prompt.rendered_prompt);
    }
    println!("snapshot_id: {}", prompt.snapshot_id);
    println!("prompt_hash: {}", prompt.prompt_hash);
    println!("token_estimate: {}", prompt.token_estimate);
    Ok(())
}

fn create_workspace_manifest(store: &SnapshotStore, scope: &SnapshotScopeArgs) -> WorkspaceManifest {
    WorkspaceManifest::new(
        scope.lane.clone(),
        store.lane_paths.root.display().to_string(),
        store
            .lane_paths
            .repo_root(&store.repo_org, &store.repo_name)
            .display()
            .to_string(),
        scope.workspace.clone(),
        LanePaths::slugify(&scope.workspace),
        format!("{}/{}", store.repo_org, store.repo_name),
        store.workspace_root().display().to_string(),
        store.workspace_root().join("worktree").display().to_string(),
        store.workspace_root().join("worktree").display().to_string(),
        store.workspace_root().join("runtime").display().to_string(),
        store.workspace_root().join("home").display().to_string(),
        "detached".to_string(),
    )
}

fn print_snapshot_summary(snapshot: &SnapshotRecord) {
    println!(
        "{}\t{}\t{}\t{}\t{}",
        snapshot.snapshot_id,
        snapshot.workspace.lane_slug,
        snapshot.workspace.workspace_slug,
        snapshot.conversation.thread_id,
        snapshot.prompt_hash
    );
}

fn print_snapshot_record(snapshot: &SnapshotRecord) {
    println!("snapshot_id: {}", snapshot.snapshot_id);
    println!(
        "parent_snapshot_id: {}",
        snapshot.parent_snapshot_id.as_deref().unwrap_or("-")
    );
    println!("lane: {}", snapshot.workspace.lane_slug);
    println!("repo: {}/{}", snapshot.workspace.repo_org, snapshot.workspace.repo_name);
    println!("workspace: {}", snapshot.workspace.workspace_slug);
    println!("thread: {}", snapshot.conversation.thread_id);
    println!("status: {:?}", snapshot.status);
    println!("prompt_hash: {}", snapshot.prompt_hash);
    println!("lineage_hash: {}", snapshot.lineage_hash);
    if !snapshot.tags.is_empty() {
        println!("tags: {}", snapshot.tags.join(","));
    }
    if let Some(note) = snapshot.note.as_ref() {
        println!("note: {note}");
    }
    if let Some(summary) = snapshot.summary.as_ref() {
        println!("summary: {}", summary.summary_text);
    }
}

fn print_snapshot_diff(diff: &SnapshotDiff) {
    println!("left_snapshot_id: {}", diff.left_snapshot_id);
    println!("right_snapshot_id: {}", diff.right_snapshot_id);
    println!("prompt_hash_changed: {}", diff.prompt_hash_changed);
    println!("lineage_changed: {}", diff.lineage_changed);
    println!("workspace_changed: {}", diff.workspace_changed);
    println!("conversation_changed: {}", diff.conversation_changed);
    println!("skills_changed: {}", diff.skills_changed);
    println!("config_changed: {}", diff.config_changed);
    if !diff.changed_fields.is_empty() {
        println!("changed_fields: {}", diff.changed_fields.join(","));
    }
    if !diff.added_tags.is_empty() {
        println!("added_tags: {}", diff.added_tags.join(","));
    }
    if !diff.removed_tags.is_empty() {
        println!("removed_tags: {}", diff.removed_tags.join(","));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use tt_core::ipc::{
        ItemView, ThreadLoadedStatus, ThreadManagementState, ThreadMonitorState, ThreadSummary,
        ThreadView, TurnView,
    };
    use tt_core::AppPaths;

    fn sample_thread_view() -> ThreadView {
        ThreadView {
            summary: ThreadSummary {
                id: "thread-1".to_string(),
                preview: "preview".to_string(),
                name: Some("sample".to_string()),
                model_provider: "openai".to_string(),
                cwd: "/tmp".to_string(),
                endpoint: None,
                runtime_workstream_id: None,
                owner_workstream_id: None,
                status: "active".to_string(),
                created_at: 1,
                updated_at: 2,
                scope: "scope".to_string(),
                archived: false,
                loaded_status: ThreadLoadedStatus::default(),
                active_flags: Vec::new(),
                active_turn_id: None,
                last_seen_turn_id: None,
                recent_output: None,
                recent_event: None,
                turn_in_flight: false,
                monitor_state: ThreadMonitorState::default(),
                last_sync_at: Utc::now(),
                management_state: ThreadManagementState::default(),
                source_kind: None,
                raw_summary: None,
            },
            history_loaded: true,
            turns: vec![
                TurnView {
                    id: "turn-1".to_string(),
                    status: "completed".to_string(),
                    error_message: None,
                    error_summary: None,
                    started_at: None,
                    completed_at: None,
                    latest_diff: None,
                    latest_plan_snapshot: None,
                    token_usage_snapshot: None,
                    latest_plan: None,
                    token_usage: None,
                    items: vec![ItemView {
                        id: "item-1".to_string(),
                        item_type: "message".to_string(),
                        status: None,
                        text: Some("first".to_string()),
                        summary: None,
                        payload: None,
                        detail_kind: None,
                        detail: None,
                    }],
                },
                TurnView {
                    id: "turn-2".to_string(),
                    status: "completed".to_string(),
                    error_message: None,
                    error_summary: None,
                    started_at: None,
                    completed_at: None,
                    latest_diff: None,
                    latest_plan_snapshot: None,
                    token_usage_snapshot: None,
                    latest_plan: None,
                    token_usage: None,
                    items: vec![ItemView {
                        id: "item-2".to_string(),
                        item_type: "message".to_string(),
                        status: None,
                        text: Some("second".to_string()),
                        summary: None,
                        payload: None,
                        detail_kind: None,
                        detail: None,
                    }],
                },
            ],
        }
    }

    fn sample_snapshot_record() -> SnapshotRecord {
        let thread = sample_thread_view();
        let turns = thread.turns.clone();
        let selection = build_selection(&turns, &SnapshotSelectionArgs::default(), "thread-1");
        SnapshotRecord {
            snapshot_id: "snapshot-1".to_string(),
            parent_snapshot_id: None,
            tags: Vec::new(),
            status: SnapshotStatus::Active,
            created_at: Utc::now().to_rfc3339(),
            created_by: "tt".to_string(),
            workspace: SnapshotWorkspaceBinding {
                lane_label: "lane".to_string(),
                lane_slug: "lane".to_string(),
                repo_org: "openai".to_string(),
                repo_name: "codex".to_string(),
                workspace_slug: "default".to_string(),
                repo_root_path: "/tmp/repo".to_string(),
                worktree_path: "/tmp/worktree".to_string(),
                branch_name: Some("main".to_string()),
                commit_sha: "abc123".to_string(),
                dirty_state_hash: Some("dirty".to_string()),
                canonical: false,
                promoted_from_snapshot_id: None,
                bound_at: Utc::now().to_rfc3339(),
            },
            conversation: selection,
            skills: SnapshotSkillSelection {
                skill_ids: vec!["chat".to_string()],
                skill_versions: BTreeMap::new(),
                loaded_skill_ids: vec!["chat".to_string()],
            },
            config: SnapshotConfigRef::default(),
            summary: None,
            prompt_hash: "prompt-hash".to_string(),
            lineage_hash: "lineage-hash".to_string(),
            note: None,
        }
    }

    #[test]
    fn build_selection_includes_requested_turns_and_ranges() {
        let thread = sample_thread_view();
        let selection = SnapshotSelectionArgs {
            include_turn_range: vec!["thread-1:turn-1..turn-2".to_string()],
            ..SnapshotSelectionArgs::default()
        };
        let built = build_selection(&thread.turns, &selection, "thread-1");
        assert_eq!(built.included_turn_ids, vec!["turn-1", "turn-2"]);
        assert_eq!(built.selected_turns.len(), 2);
        assert_eq!(built.selected_turns[0].text, "first");
        assert_eq!(built.selected_turns[1].text, "second");
    }

    #[test]
    fn prompt_hash_is_stable_for_identical_input() {
        let thread = sample_thread_view();
        let selection = build_selection(&thread.turns, &SnapshotSelectionArgs::default(), "thread-1");
        let workspace = SnapshotWorkspaceBinding {
            lane_label: "lane".to_string(),
            lane_slug: "lane".to_string(),
            repo_org: "openai".to_string(),
            repo_name: "codex".to_string(),
            workspace_slug: "default".to_string(),
            repo_root_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: Some("main".to_string()),
            commit_sha: "abc123".to_string(),
            dirty_state_hash: Some("dirty".to_string()),
            canonical: false,
            promoted_from_snapshot_id: None,
            bound_at: Utc::now().to_rfc3339(),
        };
        let skills = SnapshotSkillSelection {
            skill_ids: vec!["chat".to_string()],
            skill_versions: BTreeMap::new(),
            loaded_skill_ids: vec!["chat".to_string()],
        };
        let config = SnapshotConfigRef {
            model: Some("gpt-5.4".to_string()),
            ..SnapshotConfigRef::default()
        };
        let summary = Some(SnapshotContextSummary {
            summary_text: "hello".to_string(),
            source_turn_ids: vec!["turn-1".to_string()],
            summary_version: 1,
            generated_at: Utc::now().to_rfc3339(),
        });
        let left = build_prompt_bundle(&workspace, &selection, &skills, &config, &summary);
        let right = build_prompt_bundle(&workspace, &selection, &skills, &config, &summary);
        assert_eq!(left.prompt_hash, right.prompt_hash);
    }

    #[test]
    fn diff_detects_workspace_and_prompt_changes() {
        let thread = sample_thread_view();
        let selection = build_selection(&thread.turns, &SnapshotSelectionArgs::default(), "thread-1");
        let workspace = SnapshotWorkspaceBinding {
            lane_label: "lane".to_string(),
            lane_slug: "lane".to_string(),
            repo_org: "openai".to_string(),
            repo_name: "codex".to_string(),
            workspace_slug: "default".to_string(),
            repo_root_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: Some("main".to_string()),
            commit_sha: "abc123".to_string(),
            dirty_state_hash: Some("dirty".to_string()),
            canonical: false,
            promoted_from_snapshot_id: None,
            bound_at: Utc::now().to_rfc3339(),
        };
        let skills = SnapshotSkillSelection {
            skill_ids: vec!["chat".to_string()],
            skill_versions: BTreeMap::new(),
            loaded_skill_ids: vec!["chat".to_string()],
        };
        let config = SnapshotConfigRef::default();
        let prompt = build_prompt_bundle(&workspace, &selection, &skills, &config, &None);
        let left = SnapshotRecord {
            snapshot_id: "left".to_string(),
            parent_snapshot_id: None,
            tags: vec!["a".to_string()],
            status: SnapshotStatus::Active,
            created_at: Utc::now().to_rfc3339(),
            created_by: "tt".to_string(),
            workspace: workspace.clone(),
            conversation: selection.clone(),
            skills: skills.clone(),
            config: config.clone(),
            summary: None,
            prompt_hash: prompt.prompt_hash.clone(),
            lineage_hash: "lineage-a".to_string(),
            note: None,
        };
        let mut workspace2 = workspace.clone();
        workspace2.commit_sha = "def456".to_string();
        let right = SnapshotRecord {
            snapshot_id: "right".to_string(),
            parent_snapshot_id: Some("left".to_string()),
            tags: vec!["b".to_string()],
            status: SnapshotStatus::Active,
            created_at: Utc::now().to_rfc3339(),
            created_by: "tt".to_string(),
            workspace: workspace2,
            conversation: selection,
            skills,
            config,
            summary: None,
            prompt_hash: "prompt-2".to_string(),
            lineage_hash: "lineage-b".to_string(),
            note: None,
        };
        let diff = diff_snapshots(&left, &right);
        assert!(diff.workspace_changed);
        assert!(diff.prompt_hash_changed);
        assert!(diff.lineage_changed);
    }

    #[test]
    fn compact_excludes_summarized_turns_and_tracks_sources() {
        let turns = sample_thread_view().turns;
        let base = sample_snapshot_record();
        let selection = SnapshotSelectionArgs {
            include_turn: vec!["turn-1".to_string()],
            ..SnapshotSelectionArgs::default()
        };
        let updated = mutate_selection(
            &turns,
            &base.conversation,
            selection,
            Some("summary".to_string()),
            ContextMutationMode::Compact,
        )
        .expect("compact selection");

        assert_eq!(updated.summary_source_turn_ids, vec!["turn-1"]);
        assert!(!updated.included_turn_ids.contains(&"turn-1".to_string()));
        assert_eq!(
            updated.summary.as_ref().expect("summary").source_turn_ids,
            vec!["turn-1"]
        );
    }

    #[test]
    fn restore_from_compacted_snapshot_keeps_prompt_hash_stable() {
        let turns = sample_thread_view().turns;
        let base = sample_snapshot_record();
        let selection = SnapshotSelectionArgs {
            include_turn: vec!["turn-1".to_string()],
            ..SnapshotSelectionArgs::default()
        };
        let compacted_conversation = mutate_selection(
            &turns,
            &base.conversation,
            selection,
            Some("summary".to_string()),
            ContextMutationMode::Compact,
        )
        .expect("compact selection");
        let summary = Some(SnapshotContextSummary {
            summary_text: "summary".to_string(),
            source_turn_ids: vec!["turn-1".to_string()],
            summary_version: 2,
            generated_at: Utc::now().to_rfc3339(),
        });
        let snapshot = SnapshotRecord {
            snapshot_id: "compact-1".to_string(),
            conversation: compacted_conversation.clone(),
            summary: summary.clone(),
            ..base.clone()
        };
        let prompt_hash = compute_prompt_hash(
            &snapshot.workspace,
            &snapshot.conversation,
            &snapshot.skills,
            &snapshot.config,
            &snapshot.summary,
        );
        let snapshot = SnapshotRecord {
            prompt_hash: prompt_hash.clone(),
            lineage_hash: hash_lineage(None, &prompt_hash),
            ..snapshot
        };
        let root = std::env::temp_dir().join(format!("tt-snapshot-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::from_home(root.join(".tt"));
        let service = SnapshotService::new(paths);
        let scope = SnapshotScopeArgs {
            lane: snapshot.workspace.lane_label.clone(),
            repo: format!("{}/{}", snapshot.workspace.repo_org, snapshot.workspace.repo_name),
            workspace: snapshot.workspace.workspace_slug.clone(),
        };
        let store = service.store(&scope).expect("store");
        store
            .append_turn_history(&sample_thread_view())
            .expect("append turns");
        store.append_snapshot(&snapshot).expect("append snapshot");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let restored = rt
            .block_on(service.restore_snapshot(&SnapshotRestoreArgs {
                snapshot_id: snapshot.snapshot_id.clone(),
                bind: false,
                out: Some(root.join("restored.txt")),
            }))
            .expect("restore snapshot");

        assert_eq!(
            restored.prompt_hash,
            compute_prompt_hash(
                &snapshot.workspace,
                &snapshot.conversation,
                &snapshot.skills,
                &snapshot.config,
                &snapshot.summary
            )
        );
        assert!(restored.rendered_prompt.contains("summary"));
        assert!(!restored.rendered_prompt.contains("first"));
        assert_eq!(restored.prompt_hash, snapshot.prompt_hash);
    }

    #[test]
    fn excluded_turns_do_not_reappear_in_prompt_projection() {
        let thread = sample_thread_view();
        let selection = SnapshotSelectionArgs {
            exclude_turn: vec!["turn-1".to_string()],
            ..SnapshotSelectionArgs::default()
        };
        let built = build_selection(&thread.turns, &selection, "thread-1");
        let workspace = SnapshotWorkspaceBinding {
            lane_label: "lane".to_string(),
            lane_slug: "lane".to_string(),
            repo_org: "openai".to_string(),
            repo_name: "codex".to_string(),
            workspace_slug: "default".to_string(),
            repo_root_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: Some("main".to_string()),
            commit_sha: "abc123".to_string(),
            dirty_state_hash: Some("dirty".to_string()),
            canonical: false,
            promoted_from_snapshot_id: None,
            bound_at: Utc::now().to_rfc3339(),
        };
        let skills = SnapshotSkillSelection {
            skill_ids: vec!["chat".to_string()],
            skill_versions: BTreeMap::new(),
            loaded_skill_ids: vec!["chat".to_string()],
        };
        let config = SnapshotConfigRef::default();
        let prompt = build_prompt_bundle(&workspace, &built, &skills, &config, &None);

        assert!(!prompt.rendered_prompt.contains("turn-1 ["));
        assert!(!prompt.rendered_prompt.contains("first"));
        assert!(prompt.rendered_prompt.contains("turn-2"));
    }

    #[test]
    fn raw_turn_log_and_snapshot_log_round_trip_independently() {
        let root = std::env::temp_dir().join(format!("tt-snapshot-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::from_home(root.join(".tt"));
        let service = SnapshotService::new(paths);
        let scope = SnapshotScopeArgs {
            lane: "lane".to_string(),
            repo: "openai/codex".to_string(),
            workspace: "default".to_string(),
        };
        let store = service.store(&scope).expect("store");
        let thread = sample_thread_view();
        let snapshot = sample_snapshot_record();

        store.append_turn_history(&thread).expect("append raw turns");
        let turn_log_before = fs::read_to_string(store.turn_log_path()).expect("read turn log");
        store.append_snapshot(&snapshot).expect("append snapshot");
        let turn_log_after = fs::read_to_string(store.turn_log_path()).expect("read turn log again");
        let snapshots = store.list_snapshots().expect("list snapshots");

        assert_eq!(turn_log_before, turn_log_after);
        assert!(store.turn_log_path().exists());
        assert!(store.snapshot_log_path().exists());
        assert_ne!(store.turn_log_path(), store.snapshot_log_path());
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].snapshot_id, snapshot.snapshot_id);
        assert_eq!(store.load_turn_history().expect("load raw turns").len(), 2);
    }
}
