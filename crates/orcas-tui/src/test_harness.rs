use std::sync::Arc;

use anyhow::Result;

use crate::app::{Action, AppState, UserAction};
use crate::backend::{BackendCommand, FakeBackend};
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
        Ok(Self { backend, runtime })
    }

    pub fn state(&self) -> &AppState {
        self.runtime.state()
    }

    pub async fn dispatch(&mut self, action: UserAction) {
        self.runtime.dispatch(Action::User(action));
        self.runtime.process_all().await;
    }

    pub async fn inject_event(&mut self, event: ipc::DaemonEventEnvelope) -> Result<()> {
        self.backend.inject_event(event).await?;
        self.runtime.process_all().await;
        Ok(())
    }

    pub async fn process(&mut self) {
        self.runtime.process_all().await;
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

    pub fn prompt_box_vm(&self) -> view_model::PromptBoxViewModel {
        view_model::prompt_box(self.runtime.state())
    }

    pub fn connection_vm(&self) -> view_model::ConnectionStatusViewModel {
        view_model::connection_status(self.runtime.state())
    }

    pub fn thread_detail_vm(&self) -> view_model::ThreadDetailViewModel {
        view_model::thread_detail(self.runtime.state())
    }
}
