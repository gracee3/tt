use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStatus {
    #[default]
    Active,
    Frozen,
    Superseded,
    Pruned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotTurnRange {
    pub thread_id: String,
    pub start_turn_id: String,
    pub end_turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotConversationSelection {
    pub thread_id: String,
    #[serde(default)]
    pub included_turn_ranges: Vec<SnapshotTurnRange>,
    #[serde(default)]
    pub excluded_turn_ranges: Vec<SnapshotTurnRange>,
    #[serde(default)]
    pub included_turn_ids: Vec<String>,
    #[serde(default)]
    pub excluded_turn_ids: Vec<String>,
    #[serde(default)]
    pub pinned_turn_ids: Vec<String>,
    #[serde(default)]
    pub pinned_facts: Vec<String>,
    #[serde(default)]
    pub summary_source_turn_ids: Vec<String>,
    #[serde(default)]
    pub selected_turns: Vec<SnapshotTurn>,
    #[serde(default)]
    pub summary: Option<SnapshotContextSummary>,
    #[serde(default)]
    pub history_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotTurn {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTurnRecord {
    pub thread_id: String,
    pub turn: crate::ipc::TurnView,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotWorkspaceBinding {
    pub lane_label: String,
    pub lane_slug: String,
    pub repo_org: String,
    pub repo_name: String,
    pub workspace_slug: String,
    pub repo_root_path: String,
    pub worktree_path: String,
    #[serde(default)]
    pub branch_name: Option<String>,
    pub commit_sha: String,
    #[serde(default)]
    pub dirty_state_hash: Option<String>,
    #[serde(default)]
    pub canonical: bool,
    #[serde(default)]
    pub promoted_from_snapshot_id: Option<String>,
    pub bound_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotSkillSelection {
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub skill_versions: BTreeMap<String, String>,
    #[serde(default)]
    pub loaded_skill_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotConfigRef {
    #[serde(default)]
    pub sandbox_mode: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub read_roots: Vec<String>,
    #[serde(default)]
    pub write_roots: Vec<String>,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default)]
    pub env_blocklist: Vec<String>,
    #[serde(default)]
    pub network_access: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotContextSummary {
    pub summary_text: String,
    #[serde(default)]
    pub source_turn_ids: Vec<String>,
    pub summary_version: u64,
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    #[serde(default)]
    pub parent_snapshot_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: SnapshotStatus,
    pub created_at: String,
    pub created_by: String,
    pub workspace: SnapshotWorkspaceBinding,
    pub conversation: SnapshotConversationSelection,
    pub skills: SnapshotSkillSelection,
    pub config: SnapshotConfigRef,
    #[serde(default)]
    pub summary: Option<SnapshotContextSummary>,
    pub prompt_hash: String,
    pub lineage_hash: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptBundle {
    pub snapshot_id: String,
    pub prompt_hash: String,
    pub token_estimate: usize,
    pub workspace: SnapshotWorkspaceBinding,
    pub conversation: SnapshotConversationSelection,
    pub skills: SnapshotSkillSelection,
    pub config: SnapshotConfigRef,
    pub pinned_facts: Vec<String>,
    pub included_turn_ids: Vec<String>,
    #[serde(default)]
    pub turns: Vec<SnapshotTurn>,
    #[serde(default)]
    pub summary: Option<SnapshotContextSummary>,
    pub rendered_prompt: String,
    pub assembled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotDiff {
    pub left_snapshot_id: String,
    pub right_snapshot_id: String,
    #[serde(default)]
    pub changed_fields: Vec<String>,
    #[serde(default)]
    pub added_tags: Vec<String>,
    #[serde(default)]
    pub removed_tags: Vec<String>,
    #[serde(default)]
    pub prompt_hash_changed: bool,
    #[serde(default)]
    pub lineage_changed: bool,
    #[serde(default)]
    pub workspace_changed: bool,
    #[serde(default)]
    pub conversation_changed: bool,
    #[serde(default)]
    pub skills_changed: bool,
    #[serde(default)]
    pub config_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotLogEntry {
    pub seq: u64,
    pub event_kind: String,
    pub snapshot: SnapshotRecord,
    pub recorded_at: String,
}

impl SnapshotRecord {
    #[must_use]
    pub fn new(
        snapshot_id: impl Into<String>,
        created_by: impl Into<String>,
        workspace: SnapshotWorkspaceBinding,
        conversation: SnapshotConversationSelection,
        skills: SnapshotSkillSelection,
        config: SnapshotConfigRef,
        prompt_hash: impl Into<String>,
        lineage_hash: impl Into<String>,
    ) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            parent_snapshot_id: None,
            tags: Vec::new(),
            status: SnapshotStatus::Active,
            created_at: Utc::now().to_rfc3339(),
            created_by: created_by.into(),
            workspace,
            conversation,
            skills,
            config,
            summary: None,
            prompt_hash: prompt_hash.into(),
            lineage_hash: lineage_hash.into(),
            note: None,
        }
    }
}
