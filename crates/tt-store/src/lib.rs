//! TT v2 overlay storage.
//!
//! This crate owns the `.tt` SQLite schema and persistence layer for TT-owned
//! metadata. It does not store Codex rollout or transcript truth.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use tt_domain::{
    MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, MergeRun, Project,
    ProjectStatus, ThreadBinding, ThreadBindingStatus, ThreadRole, WorkUnit, WorkUnitStatus,
    WorkspaceBinding, WorkspaceCleanupPolicy, WorkspaceStatus, WorkspaceStrategy,
    WorkspaceSyncPolicy,
};

pub const TT_OVERLAY_DB_FILENAME: &str = "overlay.db";
const SCHEMA_VERSION: i32 = 1;

const INIT_SCHEMA: &str = include_str!("../migrations/0001_init.sql");

#[derive(Debug)]
pub struct OverlayStore {
    connection: Mutex<Connection>,
}

impl OverlayStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path.as_ref())
            .with_context(|| format!("open overlay db at {}", path.as_ref().display()))?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn open_in_dir(root: impl AsRef<Path>) -> Result<Self> {
        let path = root.as_ref().join(".tt").join(TT_OVERLAY_DB_FILENAME);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create overlay db parent {}", parent.display()))?;
        }
        Self::open(path)
    }

    pub fn path_for(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(".tt").join(TT_OVERLAY_DB_FILENAME)
    }

    fn initialize(&self) -> Result<()> {
        let connection = self.connection.lock().expect("overlay db lock poisoned");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .context("enable foreign keys")?;
        let version: i32 = connection
            .query_row("PRAGMA user_version;", [], |row| row.get(0))
            .context("read overlay schema version")?;
        if version == 0 {
            connection
                .execute_batch(INIT_SCHEMA)
                .context("initialize overlay schema")?;
            connection
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .context("set overlay schema version")?;
        } else if version != SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported tt overlay schema version {} (expected {})",
                version,
                SCHEMA_VERSION
            );
        }
        Ok(())
    }

    fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self.connection.lock().expect("overlay db lock poisoned");
        f(&connection)
    }

    fn with_transaction<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self.connection.lock().expect("overlay db lock poisoned");
        let transaction = connection
            .transaction()
            .context("start overlay transaction")?;
        let result = f(&transaction)?;
        transaction.commit().context("commit overlay transaction")?;
        Ok(result)
    }

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        self.with_transaction(|tx| {
            tx.execute(
                r#"
                insert into projects (
                    id, slug, title, objective, status, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                on conflict(id) do update set
                    slug = excluded.slug,
                    title = excluded.title,
                    objective = excluded.objective,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                "#,
                params![
                    project.id,
                    project.slug,
                    project.title,
                    project.objective,
                    project_status_to_str(project.status),
                    project.created_at.to_rfc3339(),
                    project.updated_at.to_rfc3339(),
                ],
            )
            .context("upsert project")?;
            Ok(())
        })
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, slug, title, objective, status, created_at, updated_at
                     from projects order by updated_at desc, id desc",
                )
                .context("prepare list projects")?;
            let rows = statement
                .query_map([], read_project_row)
                .context("query projects")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect projects")
        })
    }

    pub fn count_projects(&self) -> Result<usize> {
        self.with_connection(|connection| {
            let count: i64 = connection
                .query_row("select count(*) from projects", [], |row| row.get(0))
                .context("count projects")?;
            Ok(count as usize)
        })
    }

    pub fn get_project(&self, id_or_slug: &str) -> Result<Option<Project>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, slug, title, objective, status, created_at, updated_at
                     from projects where id = ?1 or slug = ?1",
                )
                .context("prepare get project")?;
            statement
                .query_row(params![id_or_slug], read_project_row)
                .optional()
                .context("get project")
        })
    }

    pub fn delete_project(&self, id_or_slug: &str) -> Result<usize> {
        self.with_transaction(|tx| {
            let affected = tx
                .execute(
                    "delete from projects where id = ?1 or slug = ?1",
                    params![id_or_slug],
                )
                .context("delete project")?;
            Ok(affected)
        })
    }

    pub fn set_project_status(&self, id_or_slug: &str, status: ProjectStatus) -> Result<usize> {
        self.with_transaction(|tx| {
            let Some(mut project) = tx
                .query_row(
                    "select id, slug, title, objective, status, created_at, updated_at
                     from projects where id = ?1 or slug = ?1",
                    params![id_or_slug],
                    read_project_row,
                )
                .optional()
                .context("get project for status update")?
            else {
                return Ok(0);
            };
            project.status = status;
            project.updated_at = Utc::now();
            tx.execute(
                r#"
                insert into projects (
                    id, slug, title, objective, status, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                on conflict(id) do update set
                    slug = excluded.slug,
                    title = excluded.title,
                    objective = excluded.objective,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                "#,
                params![
                    project.id,
                    project.slug,
                    project.title,
                    project.objective,
                    project_status_to_str(project.status),
                    project.created_at.to_rfc3339(),
                    project.updated_at.to_rfc3339(),
                ],
            )
            .context("update project status")?;
            Ok(1)
        })
    }

    pub fn upsert_work_unit(&self, work_unit: &WorkUnit) -> Result<()> {
        self.with_transaction(|tx| {
            tx.execute(
                r#"
                insert into work_units (
                    id, project_id, slug, title, task, status, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                on conflict(id) do update set
                    project_id = excluded.project_id,
                    slug = excluded.slug,
                    title = excluded.title,
                    task = excluded.task,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                "#,
                params![
                    work_unit.id,
                    work_unit.project_id,
                    work_unit.slug,
                    work_unit.title,
                    work_unit.task,
                    work_unit_status_to_str(work_unit.status),
                    work_unit.created_at.to_rfc3339(),
                    work_unit.updated_at.to_rfc3339(),
                ],
            )
            .context("upsert work unit")?;
            Ok(())
        })
    }

    pub fn list_work_units(&self, project_id: Option<&str>) -> Result<Vec<WorkUnit>> {
        self.with_connection(|connection| {
            let mut statement = if project_id.is_some() {
                connection
                    .prepare(
                        "select id, project_id, slug, title, task, status, created_at, updated_at
                         from work_units where project_id = ?1
                         order by updated_at desc, id desc",
                    )
                    .context("prepare list work units")?
            } else {
                connection
                    .prepare(
                        "select id, project_id, slug, title, task, status, created_at, updated_at
                         from work_units order by updated_at desc, id desc",
                    )
                    .context("prepare list work units")?
            };
            let rows = if let Some(project_id) = project_id {
                statement
                    .query_map(params![project_id], read_work_unit_row)
                    .context("query work units")?
            } else {
                statement
                    .query_map([], read_work_unit_row)
                    .context("query work units")?
            };
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect work units")
        })
    }

    pub fn count_work_units(&self) -> Result<usize> {
        self.with_connection(|connection| {
            let count: i64 = connection
                .query_row("select count(*) from work_units", [], |row| row.get(0))
                .context("count work units")?;
            Ok(count as usize)
        })
    }

    pub fn get_work_unit(&self, id_or_slug: &str) -> Result<Option<WorkUnit>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, project_id, slug, title, task, status, created_at, updated_at
                     from work_units where id = ?1 or slug = ?1",
                )
                .context("prepare get work unit")?;
            statement
                .query_row(params![id_or_slug], read_work_unit_row)
                .optional()
                .context("get work unit")
        })
    }

    pub fn delete_work_unit(&self, id_or_slug: &str) -> Result<usize> {
        self.with_transaction(|tx| {
            let affected = tx
                .execute(
                    "delete from work_units where id = ?1 or slug = ?1",
                    params![id_or_slug],
                )
                .context("delete work unit")?;
            Ok(affected)
        })
    }

    pub fn set_work_unit_status(&self, id_or_slug: &str, status: WorkUnitStatus) -> Result<usize> {
        self.with_transaction(|tx| {
            let Some(mut work_unit) = tx
                .query_row(
                    "select id, project_id, slug, title, task, status, created_at, updated_at
                     from work_units where id = ?1 or slug = ?1",
                    params![id_or_slug],
                    read_work_unit_row,
                )
                .optional()
                .context("get work unit for status update")?
            else {
                return Ok(0);
            };
            work_unit.status = status;
            work_unit.updated_at = Utc::now();
            tx.execute(
                r#"
                insert into work_units (
                    id, project_id, slug, title, task, status, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                on conflict(id) do update set
                    project_id = excluded.project_id,
                    slug = excluded.slug,
                    title = excluded.title,
                    task = excluded.task,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                "#,
                params![
                    work_unit.id,
                    work_unit.project_id,
                    work_unit.slug,
                    work_unit.title,
                    work_unit.task,
                    work_unit_status_to_str(work_unit.status),
                    work_unit.created_at.to_rfc3339(),
                    work_unit.updated_at.to_rfc3339(),
                ],
            )
            .context("update work unit status")?;
            Ok(1)
        })
    }

    pub fn upsert_thread_binding(&self, binding: &ThreadBinding) -> Result<()> {
        self.with_transaction(|tx| {
            tx.execute(
                r#"
                insert into thread_bindings (
                    codex_thread_id, work_unit_id, role, status, notes, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                on conflict(codex_thread_id) do update set
                    work_unit_id = excluded.work_unit_id,
                    role = excluded.role,
                    status = excluded.status,
                    notes = excluded.notes,
                    updated_at = excluded.updated_at
                "#,
                params![
                    binding.codex_thread_id,
                    binding.work_unit_id,
                    thread_role_to_str(binding.role),
                    thread_binding_status_to_str(binding.status),
                    binding.notes,
                    binding.created_at.to_rfc3339(),
                    binding.updated_at.to_rfc3339(),
                ],
            )
            .context("upsert thread binding")?;
            Ok(())
        })
    }

    pub fn list_thread_bindings_for_work_unit(
        &self,
        work_unit_id: &str,
    ) -> Result<Vec<ThreadBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select codex_thread_id, work_unit_id, role, status, notes, created_at, updated_at
                     from thread_bindings where work_unit_id = ?1
                     order by updated_at desc, codex_thread_id desc",
                )
                .context("prepare list thread bindings")?;
            let rows = statement
                .query_map(params![work_unit_id], read_thread_binding_row)
                .context("query thread bindings")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect thread bindings")
        })
    }

    pub fn list_thread_bindings(&self) -> Result<Vec<ThreadBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select codex_thread_id, work_unit_id, role, status, notes, created_at, updated_at
                     from thread_bindings order by updated_at desc, codex_thread_id desc",
                )
                .context("prepare list thread bindings")?;
            let rows = statement
                .query_map([], read_thread_binding_row)
                .context("query thread bindings")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect thread bindings")
        })
    }

    pub fn get_thread_binding(&self, codex_thread_id: &str) -> Result<Option<ThreadBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select codex_thread_id, work_unit_id, role, status, notes, created_at, updated_at
                     from thread_bindings where codex_thread_id = ?1",
                )
                .context("prepare get thread binding")?;
            statement
                .query_row(params![codex_thread_id], read_thread_binding_row)
                .optional()
                .context("get thread binding")
        })
    }

    pub fn delete_thread_binding(&self, codex_thread_id: &str) -> Result<usize> {
        self.with_transaction(|tx| {
            let affected = tx
                .execute(
                    "delete from thread_bindings where codex_thread_id = ?1",
                    params![codex_thread_id],
                )
                .context("delete thread binding")?;
            Ok(affected)
        })
    }

    pub fn set_thread_binding_status(
        &self,
        codex_thread_id: &str,
        status: ThreadBindingStatus,
    ) -> Result<usize> {
        self.with_transaction(|tx| {
            let Some(mut binding) = tx
                .query_row(
                    "select codex_thread_id, work_unit_id, role, status, notes, created_at, updated_at
                     from thread_bindings where codex_thread_id = ?1",
                    params![codex_thread_id],
                    read_thread_binding_row,
                )
                .optional()
                .context("get thread binding for status update")?
            else {
                return Ok(0);
            };
            binding.status = status;
            binding.updated_at = Utc::now();
            tx.execute(
                r#"
                insert into thread_bindings (
                    codex_thread_id, work_unit_id, role, status, notes, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                on conflict(codex_thread_id) do update set
                    work_unit_id = excluded.work_unit_id,
                    role = excluded.role,
                    status = excluded.status,
                    notes = excluded.notes,
                    updated_at = excluded.updated_at
                "#,
                params![
                    binding.codex_thread_id,
                    binding.work_unit_id,
                    thread_role_to_str(binding.role),
                    thread_binding_status_to_str(binding.status),
                    binding.notes,
                    binding.created_at.to_rfc3339(),
                    binding.updated_at.to_rfc3339(),
                ],
            )
            .context("update thread binding status")?;
            Ok(1)
        })
    }

    pub fn count_bound_threads(&self) -> Result<usize> {
        self.with_connection(|connection| {
            let count: i64 = connection
                .query_row(
                    "select count(*) from thread_bindings where status = 'bound'",
                    [],
                    |row| row.get(0),
                )
                .context("count bound threads")?;
            Ok(count as usize)
        })
    }

    pub fn upsert_workspace_binding(&self, binding: &WorkspaceBinding) -> Result<()> {
        self.with_transaction(|tx| {
            tx.execute(
                r#"
                insert into workspace_bindings (
                    id, codex_thread_id, repo_root, worktree_path, branch_name, base_ref, base_commit,
                    landing_target, strategy, sync_policy, cleanup_policy, status, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                on conflict(id) do update set
                    codex_thread_id = excluded.codex_thread_id,
                    repo_root = excluded.repo_root,
                    worktree_path = excluded.worktree_path,
                    branch_name = excluded.branch_name,
                    base_ref = excluded.base_ref,
                    base_commit = excluded.base_commit,
                    landing_target = excluded.landing_target,
                    strategy = excluded.strategy,
                    sync_policy = excluded.sync_policy,
                    cleanup_policy = excluded.cleanup_policy,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                "#,
                params![
                    binding.id,
                    binding.codex_thread_id,
                    binding.repo_root,
                    binding.worktree_path,
                    binding.branch_name,
                    binding.base_ref,
                    binding.base_commit,
                    binding.landing_target,
                    workspace_strategy_to_str(binding.strategy),
                    workspace_sync_policy_to_str(binding.sync_policy),
                    workspace_cleanup_policy_to_str(binding.cleanup_policy),
                    workspace_status_to_str(binding.status),
                    binding.created_at.to_rfc3339(),
                    binding.updated_at.to_rfc3339(),
                ],
            )
            .context("upsert workspace binding")?;
            Ok(())
        })
    }

    pub fn list_workspace_bindings_for_thread(
        &self,
        codex_thread_id: &str,
    ) -> Result<Vec<WorkspaceBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, codex_thread_id, repo_root, worktree_path, branch_name, base_ref,
                            base_commit, landing_target, strategy, sync_policy, cleanup_policy, status,
                            created_at, updated_at
                     from workspace_bindings where codex_thread_id = ?1
                     order by updated_at desc, id desc",
                )
                .context("prepare list workspace bindings")?;
            let rows = statement
                .query_map(params![codex_thread_id], read_workspace_binding_row)
                .context("query workspace bindings")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect workspace bindings")
        })
    }

    pub fn list_workspace_bindings(&self) -> Result<Vec<WorkspaceBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, codex_thread_id, repo_root, worktree_path, branch_name, base_ref,
                            base_commit, landing_target, strategy, sync_policy, cleanup_policy, status,
                            created_at, updated_at
                     from workspace_bindings order by updated_at desc, id desc",
                )
                .context("prepare list workspace bindings")?;
            let rows = statement
                .query_map([], read_workspace_binding_row)
                .context("query workspace bindings")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect workspace bindings")
        })
    }

    pub fn get_workspace_binding(&self, id: &str) -> Result<Option<WorkspaceBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, codex_thread_id, repo_root, worktree_path, branch_name, base_ref,
                            base_commit, landing_target, strategy, sync_policy, cleanup_policy, status,
                            created_at, updated_at
                     from workspace_bindings where id = ?1",
                )
                .context("prepare get workspace binding")?;
            statement
                .query_row(params![id], read_workspace_binding_row)
                .optional()
                .context("get workspace binding")
        })
    }

    pub fn delete_workspace_binding(&self, id: &str) -> Result<usize> {
        self.with_transaction(|tx| {
            let affected = tx
                .execute("delete from workspace_bindings where id = ?1", params![id])
                .context("delete workspace binding")?;
            Ok(affected)
        })
    }

    pub fn set_workspace_binding_status(&self, id: &str, status: WorkspaceStatus) -> Result<usize> {
        self.with_transaction(|tx| {
            let Some(mut binding) = tx
                .query_row(
                    "select id, codex_thread_id, repo_root, worktree_path, branch_name, base_ref,
                            base_commit, landing_target, strategy, sync_policy, cleanup_policy, status,
                            created_at, updated_at
                     from workspace_bindings where id = ?1",
                    params![id],
                    read_workspace_binding_row,
                )
                .optional()
                .context("get workspace binding for status update")?
            else {
                return Ok(0);
            };
            binding.status = status;
            binding.updated_at = Utc::now();
            tx.execute(
                r#"
                insert into workspace_bindings (
                    id, codex_thread_id, repo_root, worktree_path, branch_name, base_ref, base_commit,
                    landing_target, strategy, sync_policy, cleanup_policy, status, created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                on conflict(id) do update set
                    codex_thread_id = excluded.codex_thread_id,
                    repo_root = excluded.repo_root,
                    worktree_path = excluded.worktree_path,
                    branch_name = excluded.branch_name,
                    base_ref = excluded.base_ref,
                    base_commit = excluded.base_commit,
                    landing_target = excluded.landing_target,
                    strategy = excluded.strategy,
                    sync_policy = excluded.sync_policy,
                    cleanup_policy = excluded.cleanup_policy,
                    status = excluded.status,
                    updated_at = excluded.updated_at
                "#,
                params![
                    binding.id,
                    binding.codex_thread_id,
                    binding.repo_root,
                    binding.worktree_path,
                    binding.branch_name,
                    binding.base_ref,
                    binding.base_commit,
                    binding.landing_target,
                    workspace_strategy_to_str(binding.strategy),
                    workspace_sync_policy_to_str(binding.sync_policy),
                    workspace_cleanup_policy_to_str(binding.cleanup_policy),
                    workspace_status_to_str(binding.status),
                    binding.created_at.to_rfc3339(),
                    binding.updated_at.to_rfc3339(),
                ],
            )
            .context("update workspace binding status")?;
            Ok(1)
        })
    }

    pub fn count_ready_workspaces(&self) -> Result<usize> {
        self.with_connection(|connection| {
            let count: i64 = connection
                .query_row(
                    "select count(*) from workspace_bindings where status = 'ready'",
                    [],
                    |row| row.get(0),
                )
                .context("count ready workspaces")?;
            Ok(count as usize)
        })
    }

    pub fn upsert_merge_run(&self, run: &MergeRun) -> Result<()> {
        self.with_transaction(|tx| {
            tx.execute(
                r#"
                insert into merge_runs (
                    id, workspace_binding_id, readiness, authorization, execution, head_commit,
                    created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                on conflict(id) do update set
                    workspace_binding_id = excluded.workspace_binding_id,
                    readiness = excluded.readiness,
                    authorization = excluded.authorization,
                    execution = excluded.execution,
                    head_commit = excluded.head_commit,
                    updated_at = excluded.updated_at
                "#,
                params![
                    run.id,
                    run.workspace_binding_id,
                    merge_readiness_to_str(run.readiness),
                    merge_authorization_to_str(run.authorization),
                    merge_execution_to_str(run.execution),
                    run.head_commit,
                    run.created_at.to_rfc3339(),
                    run.updated_at.to_rfc3339(),
                ],
            )
            .context("upsert merge run")?;
            Ok(())
        })
    }

    pub fn list_merge_runs(&self) -> Result<Vec<MergeRun>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, workspace_binding_id, readiness, authorization, execution, head_commit,
                            created_at, updated_at
                     from merge_runs order by updated_at desc, id desc",
                )
                .context("prepare list merge runs")?;
            let rows = statement
                .query_map([], read_merge_run_row)
                .context("query merge runs")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
                .context("collect merge runs")
        })
    }

    pub fn get_merge_run(&self, id: &str) -> Result<Option<MergeRun>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "select id, workspace_binding_id, readiness, authorization, execution, head_commit,
                            created_at, updated_at
                     from merge_runs where id = ?1",
                )
                .context("prepare get merge run")?;
            statement
                .query_row(params![id], read_merge_run_row)
                .optional()
                .context("get merge run")
        })
    }

    pub fn delete_merge_run(&self, id: &str) -> Result<usize> {
        self.with_transaction(|tx| {
            let affected = tx
                .execute("delete from merge_runs where id = ?1", params![id])
                .context("delete merge run")?;
            Ok(affected)
        })
    }

    pub fn set_merge_run_status(
        &self,
        id: &str,
        readiness: MergeReadiness,
        authorization: MergeAuthorizationStatus,
        execution: MergeExecutionStatus,
        head_commit: Option<String>,
    ) -> Result<usize> {
        self.with_transaction(|tx| {
            let Some(mut run) = tx
                .query_row(
                    "select id, workspace_binding_id, readiness, authorization, execution, head_commit,
                            created_at, updated_at
                     from merge_runs where id = ?1",
                    params![id],
                    read_merge_run_row,
                )
                .optional()
                .context("get merge run for status update")?
            else {
                return Ok(0);
            };
            run.readiness = readiness;
            run.authorization = authorization;
            run.execution = execution;
            run.head_commit = head_commit;
            run.updated_at = Utc::now();
            tx.execute(
                r#"
                insert into merge_runs (
                    id, workspace_binding_id, readiness, authorization, execution, head_commit,
                    created_at, updated_at
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                on conflict(id) do update set
                    workspace_binding_id = excluded.workspace_binding_id,
                    readiness = excluded.readiness,
                    authorization = excluded.authorization,
                    execution = excluded.execution,
                    head_commit = excluded.head_commit,
                    updated_at = excluded.updated_at
                "#,
                params![
                    run.id,
                    run.workspace_binding_id,
                    merge_readiness_to_str(run.readiness),
                    merge_authorization_to_str(run.authorization),
                    merge_execution_to_str(run.execution),
                    run.head_commit,
                    run.created_at.to_rfc3339(),
                    run.updated_at.to_rfc3339(),
                ],
            )
            .context("update merge run status")?;
            Ok(1)
        })
    }
}

fn read_project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        slug: row.get(1)?,
        title: row.get(2)?,
        objective: row.get(3)?,
        status: project_status_from_str(row.get::<_, String>(4)?.as_str())?,
        created_at: parse_ts(row.get::<_, String>(5)?, 5)?,
        updated_at: parse_ts(row.get::<_, String>(6)?, 6)?,
    })
}

fn read_work_unit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkUnit> {
    Ok(WorkUnit {
        id: row.get(0)?,
        project_id: row.get(1)?,
        slug: row.get(2)?,
        title: row.get(3)?,
        task: row.get(4)?,
        status: work_unit_status_from_str(row.get::<_, String>(5)?.as_str())?,
        created_at: parse_ts(row.get::<_, String>(6)?, 6)?,
        updated_at: parse_ts(row.get::<_, String>(7)?, 7)?,
    })
}

fn read_thread_binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadBinding> {
    Ok(ThreadBinding {
        codex_thread_id: row.get(0)?,
        work_unit_id: row.get(1)?,
        role: thread_role_from_str(row.get::<_, String>(2)?.as_str())?,
        status: thread_binding_status_from_str(row.get::<_, String>(3)?.as_str())?,
        notes: row.get(4)?,
        created_at: parse_ts(row.get::<_, String>(5)?, 5)?,
        updated_at: parse_ts(row.get::<_, String>(6)?, 6)?,
    })
}

fn read_workspace_binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceBinding> {
    Ok(WorkspaceBinding {
        id: row.get(0)?,
        codex_thread_id: row.get(1)?,
        repo_root: row.get(2)?,
        worktree_path: row.get(3)?,
        branch_name: row.get(4)?,
        base_ref: row.get(5)?,
        base_commit: row.get(6)?,
        landing_target: row.get(7)?,
        strategy: workspace_strategy_from_str(row.get::<_, String>(8)?.as_str())?,
        sync_policy: workspace_sync_policy_from_str(row.get::<_, String>(9)?.as_str())?,
        cleanup_policy: workspace_cleanup_policy_from_str(row.get::<_, String>(10)?.as_str())?,
        status: workspace_status_from_str(row.get::<_, String>(11)?.as_str())?,
        created_at: parse_ts(row.get::<_, String>(12)?, 12)?,
        updated_at: parse_ts(row.get::<_, String>(13)?, 13)?,
    })
}

fn read_merge_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MergeRun> {
    Ok(MergeRun {
        id: row.get(0)?,
        workspace_binding_id: row.get(1)?,
        readiness: merge_readiness_from_str(row.get::<_, String>(2)?.as_str())?,
        authorization: merge_authorization_from_str(row.get::<_, String>(3)?.as_str())?,
        execution: merge_execution_from_str(row.get::<_, String>(4)?.as_str())?,
        head_commit: row.get(5)?,
        created_at: parse_ts(row.get::<_, String>(6)?, 6)?,
        updated_at: parse_ts(row.get::<_, String>(7)?, 7)?,
    })
}

fn parse_ts(raw: String, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn project_status_to_str(status: ProjectStatus) -> &'static str {
    match status {
        ProjectStatus::Active => "active",
        ProjectStatus::Blocked => "blocked",
        ProjectStatus::Completed => "completed",
    }
}

fn project_status_from_str(raw: &str) -> rusqlite::Result<ProjectStatus> {
    match raw {
        "active" => Ok(ProjectStatus::Active),
        "blocked" => Ok(ProjectStatus::Blocked),
        "completed" => Ok(ProjectStatus::Completed),
        other => Err(rusqlite::Error::InvalidColumnType(
            4,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn work_unit_status_to_str(status: WorkUnitStatus) -> &'static str {
    match status {
        WorkUnitStatus::Ready => "ready",
        WorkUnitStatus::Blocked => "blocked",
        WorkUnitStatus::Running => "running",
        WorkUnitStatus::Review => "review",
        WorkUnitStatus::Completed => "completed",
    }
}

fn work_unit_status_from_str(raw: &str) -> rusqlite::Result<WorkUnitStatus> {
    match raw {
        "ready" => Ok(WorkUnitStatus::Ready),
        "blocked" => Ok(WorkUnitStatus::Blocked),
        "running" => Ok(WorkUnitStatus::Running),
        "review" => Ok(WorkUnitStatus::Review),
        "completed" => Ok(WorkUnitStatus::Completed),
        other => Err(rusqlite::Error::InvalidColumnType(
            5,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn thread_role_to_str(role: ThreadRole) -> &'static str {
    match role {
        ThreadRole::Develop => "develop",
        ThreadRole::Review => "review",
        ThreadRole::Test => "test",
        ThreadRole::Integrate => "integrate",
        ThreadRole::Todo => "todo",
        ThreadRole::Chat => "chat",
        ThreadRole::Learn => "learn",
        ThreadRole::Handoff => "handoff",
        ThreadRole::Custom => "custom",
    }
}

fn thread_role_from_str(raw: &str) -> rusqlite::Result<ThreadRole> {
    match raw {
        "develop" => Ok(ThreadRole::Develop),
        "review" => Ok(ThreadRole::Review),
        "test" => Ok(ThreadRole::Test),
        "integrate" => Ok(ThreadRole::Integrate),
        "todo" => Ok(ThreadRole::Todo),
        "chat" => Ok(ThreadRole::Chat),
        "learn" => Ok(ThreadRole::Learn),
        "handoff" => Ok(ThreadRole::Handoff),
        "custom" => Ok(ThreadRole::Custom),
        other => Err(rusqlite::Error::InvalidColumnType(
            2,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn thread_binding_status_to_str(status: ThreadBindingStatus) -> &'static str {
    match status {
        ThreadBindingStatus::Proposed => "proposed",
        ThreadBindingStatus::Bound => "bound",
        ThreadBindingStatus::Detached => "detached",
        ThreadBindingStatus::Closed => "closed",
    }
}

fn thread_binding_status_from_str(raw: &str) -> rusqlite::Result<ThreadBindingStatus> {
    match raw {
        "proposed" => Ok(ThreadBindingStatus::Proposed),
        "bound" => Ok(ThreadBindingStatus::Bound),
        "detached" => Ok(ThreadBindingStatus::Detached),
        "closed" => Ok(ThreadBindingStatus::Closed),
        other => Err(rusqlite::Error::InvalidColumnType(
            3,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn workspace_strategy_to_str(strategy: WorkspaceStrategy) -> &'static str {
    match strategy {
        WorkspaceStrategy::Shared => "shared",
        WorkspaceStrategy::DedicatedWorktree => "dedicated-worktree",
        WorkspaceStrategy::Ephemeral => "ephemeral",
    }
}

fn workspace_strategy_from_str(raw: &str) -> rusqlite::Result<WorkspaceStrategy> {
    match raw {
        "shared" => Ok(WorkspaceStrategy::Shared),
        "dedicated-worktree" => Ok(WorkspaceStrategy::DedicatedWorktree),
        "ephemeral" => Ok(WorkspaceStrategy::Ephemeral),
        other => Err(rusqlite::Error::InvalidColumnType(
            8,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn workspace_sync_policy_to_str(policy: WorkspaceSyncPolicy) -> &'static str {
    match policy {
        WorkspaceSyncPolicy::Manual => "manual",
        WorkspaceSyncPolicy::RebaseBeforeReview => "rebase-before-review",
        WorkspaceSyncPolicy::RebaseBeforeLanding => "rebase-before-landing",
    }
}

fn workspace_sync_policy_from_str(raw: &str) -> rusqlite::Result<WorkspaceSyncPolicy> {
    match raw {
        "manual" => Ok(WorkspaceSyncPolicy::Manual),
        "rebase-before-review" => Ok(WorkspaceSyncPolicy::RebaseBeforeReview),
        "rebase-before-landing" => Ok(WorkspaceSyncPolicy::RebaseBeforeLanding),
        other => Err(rusqlite::Error::InvalidColumnType(
            9,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn workspace_cleanup_policy_to_str(policy: WorkspaceCleanupPolicy) -> &'static str {
    match policy {
        WorkspaceCleanupPolicy::KeepUntilClosed => "keep-until-closed",
        WorkspaceCleanupPolicy::PruneAfterLanding => "prune-after-landing",
        WorkspaceCleanupPolicy::KeepForAudit => "keep-for-audit",
    }
}

fn workspace_cleanup_policy_from_str(raw: &str) -> rusqlite::Result<WorkspaceCleanupPolicy> {
    match raw {
        "keep-until-closed" => Ok(WorkspaceCleanupPolicy::KeepUntilClosed),
        "prune-after-landing" => Ok(WorkspaceCleanupPolicy::PruneAfterLanding),
        "keep-for-audit" => Ok(WorkspaceCleanupPolicy::KeepForAudit),
        other => Err(rusqlite::Error::InvalidColumnType(
            10,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn workspace_status_to_str(status: WorkspaceStatus) -> &'static str {
    match status {
        WorkspaceStatus::Requested => "requested",
        WorkspaceStatus::Ready => "ready",
        WorkspaceStatus::Dirty => "dirty",
        WorkspaceStatus::Ahead => "ahead",
        WorkspaceStatus::Behind => "behind",
        WorkspaceStatus::Conflicted => "conflicted",
        WorkspaceStatus::Merged => "merged",
        WorkspaceStatus::Abandoned => "abandoned",
        WorkspaceStatus::Pruned => "pruned",
    }
}

fn workspace_status_from_str(raw: &str) -> rusqlite::Result<WorkspaceStatus> {
    match raw {
        "requested" => Ok(WorkspaceStatus::Requested),
        "ready" => Ok(WorkspaceStatus::Ready),
        "dirty" => Ok(WorkspaceStatus::Dirty),
        "ahead" => Ok(WorkspaceStatus::Ahead),
        "behind" => Ok(WorkspaceStatus::Behind),
        "conflicted" => Ok(WorkspaceStatus::Conflicted),
        "merged" => Ok(WorkspaceStatus::Merged),
        "abandoned" => Ok(WorkspaceStatus::Abandoned),
        "pruned" => Ok(WorkspaceStatus::Pruned),
        other => Err(rusqlite::Error::InvalidColumnType(
            11,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn merge_readiness_to_str(readiness: MergeReadiness) -> &'static str {
    match readiness {
        MergeReadiness::Unknown => "unknown",
        MergeReadiness::Ready => "ready",
        MergeReadiness::Blocked => "blocked",
    }
}

fn merge_readiness_from_str(raw: &str) -> rusqlite::Result<MergeReadiness> {
    match raw {
        "unknown" => Ok(MergeReadiness::Unknown),
        "ready" => Ok(MergeReadiness::Ready),
        "blocked" => Ok(MergeReadiness::Blocked),
        other => Err(rusqlite::Error::InvalidColumnType(
            2,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn merge_authorization_to_str(status: MergeAuthorizationStatus) -> &'static str {
    match status {
        MergeAuthorizationStatus::NotRequested => "not-requested",
        MergeAuthorizationStatus::Authorized => "authorized",
        MergeAuthorizationStatus::Rejected => "rejected",
    }
}

fn merge_authorization_from_str(raw: &str) -> rusqlite::Result<MergeAuthorizationStatus> {
    match raw {
        "not-requested" => Ok(MergeAuthorizationStatus::NotRequested),
        "authorized" => Ok(MergeAuthorizationStatus::Authorized),
        "rejected" => Ok(MergeAuthorizationStatus::Rejected),
        other => Err(rusqlite::Error::InvalidColumnType(
            3,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn merge_execution_to_str(status: MergeExecutionStatus) -> &'static str {
    match status {
        MergeExecutionStatus::NotStarted => "not-started",
        MergeExecutionStatus::Running => "running",
        MergeExecutionStatus::Succeeded => "succeeded",
        MergeExecutionStatus::Failed => "failed",
    }
}

fn merge_execution_from_str(raw: &str) -> rusqlite::Result<MergeExecutionStatus> {
    match raw {
        "not-started" => Ok(MergeExecutionStatus::NotStarted),
        "running" => Ok(MergeExecutionStatus::Running),
        "succeeded" => Ok(MergeExecutionStatus::Succeeded),
        "failed" => Ok(MergeExecutionStatus::Failed),
        other => Err(rusqlite::Error::InvalidColumnType(
            4,
            other.to_string(),
            rusqlite::types::Type::Text,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn open_creates_schema() {
        let dir = tempdir().expect("tempdir");
        let store = OverlayStore::open_in_dir(dir.path()).expect("open overlay store");

        assert!(store.list_projects().expect("list").is_empty());
    }

    #[test]
    fn project_work_unit_binding_and_workspace_round_trip() {
        let dir = tempdir().expect("tempdir");
        let store = OverlayStore::open_in_dir(dir.path()).expect("open overlay store");

        let project = Project {
            id: "proj-1".to_string(),
            slug: "alpha".to_string(),
            title: "Alpha".to_string(),
            objective: "Ship v2".to_string(),
            status: ProjectStatus::Active,
            created_at: ts(),
            updated_at: ts(),
        };
        store.upsert_project(&project).expect("upsert project");

        let work_unit = WorkUnit {
            id: "wu-1".to_string(),
            project_id: project.id.clone(),
            slug: Some("chunk-a".to_string()),
            title: "Chunk A".to_string(),
            task: "Build the store".to_string(),
            status: WorkUnitStatus::Ready,
            created_at: ts(),
            updated_at: ts(),
        };
        store
            .upsert_work_unit(&work_unit)
            .expect("upsert work unit");

        let binding = ThreadBinding {
            codex_thread_id: "thread-1".to_string(),
            work_unit_id: Some(work_unit.id.clone()),
            role: ThreadRole::Develop,
            status: ThreadBindingStatus::Bound,
            notes: Some("primary".to_string()),
            created_at: ts(),
            updated_at: ts(),
        };
        store
            .upsert_thread_binding(&binding)
            .expect("upsert binding");

        let workspace = WorkspaceBinding {
            id: "wsb-1".to_string(),
            codex_thread_id: binding.codex_thread_id.clone(),
            repo_root: "/repo".to_string(),
            worktree_path: Some("/repo/.worktrees/a".to_string()),
            branch_name: Some("tt/alpha".to_string()),
            base_ref: Some("main".to_string()),
            base_commit: Some("abc123".to_string()),
            landing_target: Some("main".to_string()),
            strategy: WorkspaceStrategy::DedicatedWorktree,
            sync_policy: WorkspaceSyncPolicy::RebaseBeforeLanding,
            cleanup_policy: WorkspaceCleanupPolicy::PruneAfterLanding,
            status: WorkspaceStatus::Ready,
            created_at: ts(),
            updated_at: ts(),
        };
        store
            .upsert_workspace_binding(&workspace)
            .expect("upsert workspace");

        let merge_run = MergeRun {
            id: "merge-1".to_string(),
            workspace_binding_id: workspace.id.clone(),
            readiness: MergeReadiness::Ready,
            authorization: MergeAuthorizationStatus::Authorized,
            execution: MergeExecutionStatus::NotStarted,
            head_commit: Some("abc123".to_string()),
            created_at: ts(),
            updated_at: ts(),
        };
        store
            .upsert_merge_run(&merge_run)
            .expect("upsert merge run");

        assert_eq!(
            store.get_project("alpha").expect("get project").unwrap().id,
            project.id
        );
        assert_eq!(store.list_projects().expect("list projects").len(), 1);
        assert_eq!(
            store
                .get_work_unit("chunk-a")
                .expect("get work unit")
                .unwrap()
                .project_id,
            project.id
        );
        assert_eq!(
            store
                .list_work_units(Some(&project.id))
                .expect("list work units")
                .len(),
            1
        );
        assert_eq!(
            store
                .list_thread_bindings_for_work_unit(&work_unit.id)
                .expect("list thread bindings")
                .len(),
            1
        );
        assert_eq!(
            store
                .list_workspace_bindings_for_thread(&binding.codex_thread_id)
                .expect("list workspace bindings")
                .len(),
            1
        );
    }
}
