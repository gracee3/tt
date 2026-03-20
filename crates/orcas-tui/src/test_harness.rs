use std::sync::Arc;

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{Action, AppState, CollaborationFocus, TopLevelView, UiEvent, UserAction};
use crate::backend::{BackendCommand, FakeBackend};
use crate::render;
use crate::runtime::AppRuntime;
use crate::view_model;
use orcas_core::ipc;

pub struct AppHarness {
    backend: FakeBackend,
    runtime: AppRuntime<FakeBackend>,
}

impl AppHarness {
    pub async fn new(snapshot: ipc::StateSnapshot) -> Result<Self> {
        let backend = FakeBackend::new(snapshot);
        let mut runtime = AppRuntime::new(Arc::new(backend.clone()));
        runtime.bootstrap().await;
        runtime.process_until_idle(20).await;
        Ok(Self { backend, runtime })
    }

    async fn flush_runtime(&mut self) {
        self.runtime.process_until_idle(20).await;
    }

    pub fn state(&self) -> &AppState {
        self.runtime.state()
    }

    pub async fn dispatch(&mut self, action: UserAction) {
        self.runtime.dispatch(Action::User(action));
        self.flush_runtime().await;
    }

    pub fn dispatch_no_wait(&mut self, action: UserAction) {
        self.runtime.dispatch(Action::User(action));
    }

    pub async fn inject_ui_event(&mut self, event: UiEvent) {
        self.runtime.dispatch(Action::Event(event));
        self.flush_runtime().await;
    }

    pub fn inject_ui_event_no_wait(&mut self, event: UiEvent) {
        self.runtime.dispatch(Action::Event(event));
    }

    pub async fn inject_event(&mut self, event: ipc::DaemonEventEnvelope) -> Result<()> {
        self.backend.inject_event(event).await?;
        self.flush_runtime().await;
        Ok(())
    }

    pub async fn process(&mut self) {
        self.flush_runtime().await;
    }

    pub fn force_reconnect_now(&mut self) {
        self.runtime.force_reconnect_now();
    }

    pub async fn set_thread(&self, thread: ipc::ThreadView) {
        self.backend.set_thread(thread).await;
    }

    pub async fn set_turn(&self, turn: ipc::TurnAttachResponse) {
        self.backend.set_turn(turn).await;
    }

    pub async fn set_active_turns(&self, turns: Vec<ipc::TurnStateView>) {
        self.backend.set_active_turns(turns).await;
    }

    pub async fn set_workunit_detail(&self, detail: ipc::WorkunitGetResponse) {
        self.backend.set_workunit_detail(detail).await;
    }

    pub async fn set_proposal_artifact_summary(
        &self,
        summary: ipc::SupervisorProposalArtifactSummary,
    ) {
        self.backend.set_proposal_artifact_summary(summary).await;
    }

    pub async fn set_proposal_artifact_detail(
        &self,
        detail: ipc::SupervisorProposalArtifactDetail,
    ) {
        self.backend.set_proposal_artifact_detail(detail).await;
    }

    pub async fn replace_snapshot(&self, snapshot: ipc::StateSnapshot) {
        self.backend.replace_snapshot(snapshot).await;
    }

    pub async fn fail_next_command(&self, message: impl Into<String>) {
        self.backend.fail_next_command(message).await;
    }

    pub async fn fail_snapshot_once(&self, message: impl Into<String>) {
        self.backend.fail_snapshot_once(message).await;
    }

    pub async fn fail_subscribe_once(&self, message: impl Into<String>) {
        self.backend.fail_subscribe_once(message).await;
    }

    pub async fn disconnect_events(&self) {
        self.backend.disconnect_events().await;
    }

    pub async fn recorded_commands(&self) -> Vec<BackendCommand> {
        self.backend.recorded_commands().await
    }

    pub async fn snapshot_requests(&self) -> usize {
        self.backend.snapshot_requests().await
    }

    pub async fn subscribe_requests(&self) -> usize {
        self.backend.subscribe_requests().await
    }

    pub fn thread_list_vm(&self) -> view_model::ThreadListViewModel {
        view_model::thread_list(self.runtime.state())
    }

    pub fn event_log_vm(&self) -> view_model::EventLogViewModel {
        view_model::event_log(self.runtime.state())
    }

    pub fn overview_vm(&self) -> view_model::OverviewViewModel {
        view_model::overview_view(self.runtime.state())
    }

    pub fn main_vm(&self) -> view_model::MainViewModel {
        view_model::main_view(self.runtime.state())
    }

    pub fn main_hierarchy_vm(&self) -> view_model::MainHierarchyListViewModel {
        view_model::main_hierarchy_list(self.runtime.state())
    }

    pub fn review_vm(&self) -> view_model::ReviewViewModel {
        view_model::review_view(self.runtime.state())
    }

    pub fn review_queue_vm(&self) -> view_model::ReviewQueueViewModel {
        view_model::review_queue(self.runtime.state())
    }

    pub fn connection_vm(&self) -> view_model::ConnectionStatusViewModel {
        view_model::connection_status(self.runtime.state())
    }

    pub fn current_view(&self) -> TopLevelView {
        self.runtime.state().current_view
    }

    pub fn collaboration_focus(&self) -> CollaborationFocus {
        self.runtime.state().collaboration_focus
    }

    pub fn prompt_in_flight(&self) -> bool {
        self.runtime.state().prompt_in_flight
    }

    pub fn selected_thread_id(&self) -> Option<&str> {
        self.runtime.state().selected_thread_id.as_deref()
    }

    pub fn selected_workstream_id(&self) -> Option<&str> {
        self.runtime.state().selected_workstream_id.as_deref()
    }

    pub fn selected_work_unit_id(&self) -> Option<&str> {
        self.runtime.state().selected_work_unit_id.as_deref()
    }

    pub fn thread_detail_vm(&self) -> view_model::ThreadDetailViewModel {
        view_model::thread_detail(self.runtime.state())
    }

    pub fn thread_summary_vm(&self) -> view_model::PanelViewModel {
        view_model::thread_summary(self.runtime.state())
    }

    pub fn threads_vm(&self) -> view_model::ThreadsViewModel {
        view_model::threads_view(self.runtime.state())
    }

    pub fn workstream_list_vm(&self) -> view_model::WorkstreamListViewModel {
        view_model::workstream_list(self.runtime.state())
    }

    pub fn workstream_detail_vm(&self) -> view_model::WorkstreamDetailViewModel {
        view_model::workstream_detail(self.runtime.state())
    }

    pub fn work_unit_list_vm(&self) -> view_model::WorkUnitListViewModel {
        view_model::work_unit_list(self.runtime.state())
    }

    pub fn assignment_list_vm(&self) -> view_model::AssignmentListViewModel {
        view_model::assignment_list(self.runtime.state())
    }

    pub fn collaboration_detail_vm(&self) -> view_model::CollaborationDetailViewModel {
        view_model::collaboration_detail(self.runtime.state())
    }

    pub fn collaboration_status_vm(&self) -> view_model::CollaborationStatusViewModel {
        view_model::collaboration_status(self.runtime.state())
    }

    pub fn collaboration_history_vm(&self) -> view_model::CollaborationHistoryViewModel {
        view_model::collaboration_history(self.runtime.state())
    }

    pub fn collaboration_vm(&self) -> view_model::CollaborationViewModel {
        view_model::collaboration_view(self.runtime.state())
    }

    pub fn render_text(&self, width: u16, height: u16) -> String {
        self.render_lines(width, height).join("\n")
    }

    pub fn render_lines(&self, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render::render(frame, self.runtime.state()))
            .expect("render");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|row| {
                let mut line = String::new();
                for col in 0..width {
                    line.push_str(buffer[(col, row)].symbol());
                }
                line.trim_end().to_string()
            })
            .collect()
    }
}
