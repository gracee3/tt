#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

mod api;
mod push;
mod pwa;
mod storage;
mod watch;
mod workstreams;
mod workspace;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_arch = "wasm32")]
use leptos::mount::mount_to_body;
use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use leptos::task::spawn_local;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::hooks::{use_navigate, use_params_map};
use leptos_router::path;

use orcas_core::authority;
use orcas_core::ipc::{OperatorInboxActionKind, OperatorRemoteActionRequestStatus};
use orcas_core::{WorkUnitStatus, WorkstreamStatus};
use orcas_operator_core::{
    DeliveryJobView, DeliveryPageView, InboxDetailPageView, InboxItemCardView, InboxPageView,
    NotificationCandidateView, NotificationPageView, OperatorServerSettings, RemoteActionPageView,
    RemoteActionRequestView, ViewChangeSummary, action_kind_label, delivery_status_hint,
    inbox_status_hint, inbox_status_label, notification_status_hint,
    pending_remote_action_request_for_item_action, remote_action_status_hint,
    remote_action_status_label, source_kind_label, summarize_delivery_page_change,
    summarize_inbox_page_change, summarize_notification_page_change,
    summarize_remote_action_request_change,
};
use workstreams::{
    WorkstreamsDashboardData, humanize_snake_case, inferred_live_thread_for_assignment, live_thread_linkage,
    tracked_thread_runtime_status,
};
use workspace::{WorkspaceFocus, WorkspaceSection, WorkspaceState};

pub fn mount_app() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        pwa::register_service_worker();
        mount_to_body(App);
    }
}

fn format_timestamp(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_optional_timestamp(timestamp: Option<chrono::DateTime<chrono::Utc>>) -> String {
    timestamp
        .map(format_timestamp)
        .unwrap_or_else(|| "Unknown".to_string())
}

fn format_unix_millis(timestamp_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(format_timestamp)
        .unwrap_or_else(|| timestamp_ms.to_string())
}

fn proposal_status_summary(proposal: &orcas_core::ipc::WorkUnitProposalSummary) -> String {
    if proposal.has_generation_failed {
        let stage = proposal
            .latest_failure_stage
            .as_ref()
            .map(|stage| {
                humanize_snake_case(
                    serde_json::to_string(stage)
                        .unwrap_or_default()
                        .trim_matches('"'),
                )
            })
            .unwrap_or_else(|| "Generation failed".to_string());
        format!("proposal failed · {stage}")
    } else if proposal.has_open_proposal {
        let decision = proposal
            .open_proposed_decision_type
            .or(proposal.latest_proposed_decision_type)
            .map(|decision| {
                humanize_snake_case(
                    serde_json::to_string(&decision)
                        .unwrap_or_default()
                        .trim_matches('"'),
                )
            })
            .unwrap_or_else(|| "Review ready".to_string());
        format!("proposal ready · {decision}")
    } else {
        let status = humanize_snake_case(
            serde_json::to_string(&proposal.latest_status)
                .unwrap_or_default()
                .trim_matches('"'),
        );
        format!("proposal {status}")
    }
}

fn lane_activity_summary(
    report: Option<&orcas_core::ipc::ReportSummary>,
    proposal: Option<&orcas_core::ipc::WorkUnitProposalSummary>,
    decision: Option<&orcas_core::ipc::DecisionSummary>,
) -> String {
    let mut parts = Vec::new();
    if let Some(report) = report {
        parts.push(if report.needs_supervisor_review {
            "report submitted".to_string()
        } else {
            "report recorded".to_string()
        });
        parts.push(format!(
            "parse {}",
            humanize_snake_case(
                serde_json::to_string(&report.parse_result)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
    }
    if let Some(proposal) = proposal {
        parts.push(proposal_status_summary(proposal));
    }
    if let Some(decision) = decision {
        parts.push(format!(
            "decision {}",
            humanize_snake_case(
                serde_json::to_string(&decision.decision_type)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
    }
    if parts.is_empty() {
        "No supervisor activity yet".to_string()
    } else {
        parts.join(" · ")
    }
}

fn is_message_like_item(item_type: &str) -> bool {
    let lowered = item_type.to_ascii_lowercase();
    lowered.contains("message") || lowered.contains("reasoning") || lowered.contains("comment")
}

fn humanize_optional_kind(kind: Option<&str>, fallback: &str) -> String {
    kind.map(humanize_snake_case)
        .unwrap_or_else(|| fallback.to_string())
}

#[component]
fn TurnPlanCard(plan: orcas_core::ipc::TurnPlanView) -> impl IntoView {
    let explanation = plan.explanation.clone();
    let steps = plan.plan.clone();
    view! {
        <div class="json-panel">
            <p class="eyebrow">"Plan"</p>
            {explanation
                .filter(|value| !value.trim().is_empty())
                .map(|value| view! { <p class="item-summary">{value}</p> })}
            {if steps.is_empty() {
                view! { <p class="item-meta">"No structured plan steps recorded."</p> }.into_any()
            } else {
                view! {
                    <div class="detail-panel">
                        {steps
                            .into_iter()
                            .map(|step| view! {
                                <div class="detail-block">
                                    <div class="item-card-topline">
                                        <span class="status-pill">{humanize_snake_case(&step.status)}</span>
                                    </div>
                                    <p class="item-summary">{step.step}</p>
                                </div>
                            })
                            .collect_view()}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

#[component]
fn TokenUsageCard(token_usage: orcas_core::ipc::ThreadTokenUsageView) -> impl IntoView {
    view! {
        <div class="json-panel">
            <p class="eyebrow">"Token usage"</p>
            <dl class="detail-grid">
                <div>
                    <dt>"Total"</dt>
                    <dd>{token_usage.total_tokens.to_string()}</dd>
                </div>
                <div>
                    <dt>"Input"</dt>
                    <dd>{token_usage.input_tokens.to_string()}</dd>
                </div>
                <div>
                    <dt>"Cached input"</dt>
                    <dd>{token_usage.cached_input_tokens.to_string()}</dd>
                </div>
                <div>
                    <dt>"Output"</dt>
                    <dd>{token_usage.output_tokens.to_string()}</dd>
                </div>
                <div>
                    <dt>"Reasoning output"</dt>
                    <dd>{token_usage.reasoning_output_tokens.to_string()}</dd>
                </div>
            </dl>
        </div>
    }
}

#[component]
fn JsonValueTree(value: serde_json::Value) -> impl IntoView {
    match value {
        serde_json::Value::Null => view! { <span class="muted">"null"</span> }.into_any(),
        serde_json::Value::Bool(value) => view! { <span>{value.to_string()}</span> }.into_any(),
        serde_json::Value::Number(value) => view! { <span>{value.to_string()}</span> }.into_any(),
        serde_json::Value::String(value) => view! { <span class="json-string">{value}</span> }.into_any(),
        serde_json::Value::Array(values) => view! {
            <ul class="json-list">
                {values
                    .into_iter()
                    .map(|value| view! { <li><JsonValueTree value /></li> })
                    .collect_view()}
            </ul>
        }
        .into_any(),
        serde_json::Value::Object(entries) => view! {
            <dl class="json-object">
                {entries
                    .into_iter()
                    .map(|(key, value)| view! {
                        <dt>{key}</dt>
                        <dd><JsonValueTree value /></dd>
                    })
                    .collect_view()}
            </dl>
        }
        .into_any(),
    }
}

#[component]
fn TurnItemCard(item: orcas_core::ipc::ItemView) -> impl IntoView {
    let message_like = is_message_like_item(&item.item_type);
    let detail_kind = humanize_optional_kind(item.detail_kind.as_deref(), "Detail");
    view! {
        <div class="detail-block">
            <div class="item-card-topline">
                <span class="status-pill">{humanize_snake_case(&item.item_type)}</span>
                <span class="muted">
                    {item
                        .status
                        .clone()
                        .map(|status| humanize_snake_case(&status))
                        .unwrap_or_else(|| "No status".to_string())}
                </span>
            </div>
            {item
                .summary
                .clone()
                .map(|summary| view! { <p class="item-summary">{summary}</p> })}
            {item
                .text
                .clone()
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    if message_like {
                        view! { <div class="message-block">{text}</div> }.into_any()
                    } else {
                        view! { <pre class="code-block">{text}</pre> }.into_any()
                    }
                })}
            {item.payload.clone().map(|payload| view! {
                <div class="json-panel">
                    <p class="eyebrow">"Payload"</p>
                    <details>
                        <summary>"Show payload"</summary>
                        <JsonValueTree value=payload />
                    </details>
                </div>
            })}
            {item.detail.clone().map(|detail| view! {
                <div class="json-panel">
                    <p class="eyebrow">{detail_kind.clone()}</p>
                    <details>
                        <summary>"Show typed detail"</summary>
                        <JsonValueTree value=detail />
                    </details>
                </div>
            })}
        </div>
    }
}

#[component]
fn ThreadTurnCard(turn: orcas_core::ipc::TurnView) -> impl IntoView {
    view! {
        <div class="detail-block">
            <div class="item-card-topline">
                <span class="status-pill">{humanize_snake_case(&turn.status)}</span>
                <span class="muted">{turn.id.clone()}</span>
            </div>
            <p class="item-meta">
                {format!(
                    "{}{}",
                    turn.started_at
                        .map(|started_at| format!("started {}", format_timestamp(started_at)))
                        .unwrap_or_else(|| "start unknown".to_string()),
                    turn.completed_at
                        .map(|completed_at| format!(" · completed {}", format_timestamp(completed_at)))
                        .unwrap_or_default()
                )}
            </p>
            {turn
                .error_summary
                .clone()
                .or(turn.error_message.clone())
                .map(|summary| view! { <p class="item-summary">{summary}</p> })}
            {turn.latest_diff.clone().map(|diff| view! {
                <div class="json-panel">
                    <p class="eyebrow">"Latest diff"</p>
                    <pre class="code-block">{diff}</pre>
                </div>
            })}
            {match turn.latest_plan.clone() {
                Some(plan) => view! { <TurnPlanCard plan /> }.into_any(),
                None => turn
                    .latest_plan_snapshot
                    .clone()
                    .map(|snapshot| view! {
                        <div class="json-panel">
                            <p class="eyebrow">"Plan snapshot"</p>
                            <JsonValueTree value=snapshot />
                        </div>
                    }
                    .into_any())
                    .unwrap_or_else(|| view! {}.into_any()),
            }}
            {match turn.token_usage.clone() {
                Some(token_usage) => view! { <TokenUsageCard token_usage /> }.into_any(),
                None => turn
                    .token_usage_snapshot
                    .clone()
                    .map(|snapshot| view! {
                        <div class="json-panel">
                            <p class="eyebrow">"Token usage"</p>
                            <JsonValueTree value=snapshot />
                        </div>
                    }
                    .into_any())
                    .unwrap_or_else(|| view! {}.into_any()),
            }}
            <div class="detail-panel">
                {turn.items.into_iter().map(|item| view! { <TurnItemCard item /> }).collect_view()}
            </div>
        </div>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let settings = RwSignal::new(storage::load_settings());
    let workspace = RwSignal::new(storage::load_workspace_state());
    provide_context(settings);
    provide_context(workspace);

    Effect::new(move |_| {
        storage::save_settings(&settings.get());
    });
    Effect::new(move |_| {
        storage::save_workspace_state(&workspace.get());
    });

    view! {
        <Router>
            <div class="app-shell">
                <header class="shell-header">
                    <div>
                        <p class="eyebrow">"Orcas operator web"</p>
                        <h1>"Mirrored operator control plane"</h1>
                        <p class="settings-status">
                            {move || {
                                let state = workspace.get();
                                let focus = state.focus.as_ref().map(|focus| {
                                    format!("Current focus: {} · {}", focus.kind_label, focus.status_label)
                                });
                                focus.unwrap_or_else(|| {
                                    format!("Active section: {}", state.active_section.label())
                                })
                            }}
                        </p>
                    </div>
                </header>
                <div class="workspace-grid">
                    <aside class="workspace-sidebar">
                        <WorkspaceShell />
                        <SettingsPanel />
                    </aside>
                    <main class="shell-main">
                        <Routes fallback=|| view! { <NotFoundPage /> }>
                            <Route path=path!("") view=WorkstreamsRoute />
                            <Route path=path!("workstreams") view=WorkstreamsRoute />
                            <Route path=path!("threads") view=ThreadsRoute />
                            <Route path=path!("inbox") view=InboxRoute />
                            <Route path=path!("inbox/:item_id") view=InboxDetailRoute />
                            <Route path=path!("notifications") view=NotificationsRoute />
                            <Route path=path!("deliveries") view=DeliveriesRoute />
                            <Route path=path!("actions") view=ActionListRoute />
                            <Route path=path!("actions/:request_id") view=ActionRoute />
                        </Routes>
                    </main>
                </div>
            </div>
        </Router>
    }
}

#[component]
fn WorkspaceShell() -> impl IntoView {
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");

    view! {
        <section class="workspace-panel">
            <div class="workspace-panel-header">
                <div>
                    <p class="eyebrow">"Workspace"</p>
                    <h2>"Navigation"</h2>
                </div>
                <p class="settings-status">
                    {move || {
                        let state = workspace.get();
                        format!("Active section: {}", state.active_section.label())
                    }}
                </p>
            </div>
            <nav class="shell-nav shell-nav-vertical">
                <A
                    href=WorkspaceSection::Workstreams.href()
                    class:active=move || {
                        workspace.get().active_section == WorkspaceSection::Workstreams
                    }
                >
                    "Workstreams"
                </A>
                <A
                    href=WorkspaceSection::Threads.href()
                    class:active=move || workspace.get().active_section == WorkspaceSection::Threads
                >
                    "Threads"
                </A>
                <A
                    href=WorkspaceSection::Inbox.href()
                    class:active=move || workspace.get().active_section == WorkspaceSection::Inbox
                >
                    "Inbox"
                </A>
                <A
                    href=WorkspaceSection::Notifications.href()
                    class:active=move || workspace.get().active_section == WorkspaceSection::Notifications
                >
                    "Notifications"
                </A>
                <A
                    href=WorkspaceSection::Deliveries.href()
                    class:active=move || workspace.get().active_section == WorkspaceSection::Deliveries
                >
                    "Deliveries"
                </A>
                <A
                    href=WorkspaceSection::Actions.href()
                    class:active=move || workspace.get().active_section == WorkspaceSection::Actions
                >
                    "Actions"
                </A>
            </nav>
            <WorkspaceFocusPanel />
        </section>
    }
}

#[component]
fn WorkspaceFocusPanel() -> impl IntoView {
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");

    view! {
        <section class="workspace-focus-panel">
            <p class="eyebrow">"Current focus"</p>
            {move || {
                let state = workspace.get();
                match state.focus.as_ref() {
                    Some(focus) => {
                        let status_label = focus.status_label.clone();
                        let kind_label = focus.kind_label.clone();
                        let href = focus.href.clone();
                        let title = focus.title.clone();
                        let summary = focus.summary.clone();
                        let section_label = focus.section.label();
                        view! {
                            <div class="workspace-focus-card">
                                <div class="item-card-topline">
                                    <span class="status-pill">{status_label}</span>
                                    <span class="muted">{kind_label}</span>
                                </div>
                                <a class="item-title" href=href>{title}</a>
                                <p class="item-summary">{summary}</p>
                                <p class="item-meta">{format!("Focus section: {}", section_label)}</p>
                            </div>
                        }
                        .into_any()
                    }
                    None => view! {
                        <div class="empty-state workspace-empty-state">
                            <h3>"No current focus"</h3>
                            <p>"Select an inbox item or action request to pin it here while related views refresh."</p>
                        </div>
                    }
                    .into_any(),
                }
            }}
        </section>
    }
}

#[component]
fn SettingsPanel() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");

    view! {
        <section class="settings-panel">
            <label class="field">
                <span>"Server URL"</span>
                <input
                    type="url"
                    placeholder="http://127.0.0.1:3000"
                    prop:value=move || settings.with(|settings| settings.server_url.clone())
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        settings.update(|settings| settings.server_url = value);
                    }
                />
            </label>
            <label class="field">
                <span>"Origin node"</span>
                <input
                    type="text"
                    placeholder="daemon-1"
                    prop:value=move || settings.with(|settings| settings.origin_node_id.clone())
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        settings.update(|settings| settings.origin_node_id = value);
                    }
                />
            </label>
            <label class="field">
                <span>"Operator token"</span>
                <input
                    type="password"
                    placeholder="Bearer token"
                    prop:value=move || {
                        settings.with(|settings| settings.operator_api_token.clone().unwrap_or_default())
                    }
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        settings.update(|settings| {
                            settings.operator_api_token = if value.trim().is_empty() {
                                None
                            } else {
                                Some(value)
                            };
                        });
                    }
                />
            </label>
            <label class="field">
                <span>"Push public key"</span>
                <input
                    type="text"
                    placeholder="VAPID public key"
                    prop:value=move || {
                        settings.with(|settings| settings.push_public_key.clone().unwrap_or_default())
                    }
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        settings.update(|settings| {
                            settings.push_public_key = if value.trim().is_empty() {
                                None
                            } else {
                                Some(value)
                            };
                        });
                    }
                />
            </label>
            <p class="settings-status">
                {move || {
                    let current = settings.get();
                    if storage::settings_ready(&current) {
                        format!("Connected as origin `{}`", current.origin_node_id)
                    } else {
                        "Configure server URL and origin node id to load data.".to_string()
                    }
            }}
            </p>
            <p class="settings-note">"Settings persist to localStorage."</p>
            <PushRegistrationPanel />
        </section>
    }
}

#[component]
fn PushRegistrationPanel() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let refresh_epoch = RwSignal::new(0u64);
    let working = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let status = LocalResource::new(move || {
        let settings = settings.get_untracked();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_browser_push_status(settings).await }
    });

    view! {
        <article class="push-panel">
            <h3>"Browser notifications"</h3>
            <p class="settings-status">
                "Register this browser as a push target without talking to the daemon directly."
            </p>
            <div class="toolbar">
                <button
                    class="primary-button"
                    disabled=move || working.get()
                    on:click=move |_| {
                        let _settings = settings.get_untracked();
                        working.set(true);
                        error.set(None);
                        let _refresh_epoch = refresh_epoch.clone();
                        let _error = error.clone();
                        let _working = working.clone();
                        #[cfg(target_arch = "wasm32")]
                        spawn_local(async move {
                            let result = api::register_browser_push_subscription(_settings).await;
                            _working.set(false);
                            match result {
                                Ok(_) => _refresh_epoch.update(|value| *value += 1),
                                Err(failure) => _error.set(Some(failure)),
                            }
                        });
                    }
                >
                    "Enable browser notifications"
                </button>
                <button
                    class="refresh-button"
                    disabled=move || working.get()
                    on:click=move |_| {
                        let _settings = settings.get_untracked();
                        working.set(true);
                        error.set(None);
                        let _refresh_epoch = refresh_epoch.clone();
                        let _error = error.clone();
                        let _working = working.clone();
                        #[cfg(target_arch = "wasm32")]
                        spawn_local(async move {
                            let result = api::disable_browser_push_subscription(_settings).await;
                            _working.set(false);
                            match result {
                                Ok(_) => _refresh_epoch.update(|value| *value += 1),
                                Err(failure) => _error.set(Some(failure)),
                            }
                        });
                    }
                >
                    "Disable"
                </button>
                <button
                    class="refresh-button"
                    disabled=move || working.get()
                    on:click=move |_| refresh_epoch.update(|value| *value += 1)
                >
                    "Refresh status"
                </button>
            </div>
            {move || match error.get() {
                Some(error) => view! { <ErrorPanel error=error /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match status.get() {
                None => view! { <p class="muted">"Loading browser push status…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(state)) => {
                    let permission = pwa::browser_notification_permission_label(
                        state.notification_permission,
                    );
                    view! {
                        <dl class="detail-grid">
                            <div>
                                <dt>"Service worker"</dt>
                                <dd>{if state.service_worker_registered { "registered" } else { "not registered" }}</dd>
                            </div>
                            <div>
                                <dt>"Permission"</dt>
                                <dd>{permission}</dd>
                            </div>
                            <div>
                                <dt>"Browser subscription"</dt>
                                <dd>
                                    {state
                                        .browser_subscription
                                        .as_ref()
                                        .map(|subscription| subscription.endpoint.as_str())
                                        .unwrap_or("none")}
                                </dd>
                            </div>
                            <div>
                                <dt>"Recipient id"</dt>
                                <dd>{state.recipient_id.clone()}</dd>
                            </div>
                            <div>
                                <dt>"Subscription id"</dt>
                                <dd>{state.subscription_id.clone()}</dd>
                            </div>
                            <div>
                                <dt>"Server subscription"</dt>
                                <dd>
                                    {state
                                        .server_subscription_enabled
                                        .map(|enabled| if enabled { "enabled" } else { "disabled" })
                                        .unwrap_or("not registered")}
                                </dd>
                            </div>
                        </dl>
                    }
                    .into_any()
                }
            }}
        </article>
    }
}

#[component]
fn WorkstreamsRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let refresh_epoch = RwSignal::new(0u64);
    let action_message = RwSignal::new(None::<String>);
    let action_error = RwSignal::new(None::<String>);
    let create_title = RwSignal::new(String::new());
    let create_objective = RwSignal::new(String::new());
    let create_priority = RwSignal::new("P2".to_string());
    let create_status = RwSignal::new("active".to_string());
    let create_working = RwSignal::new(false);
    let dashboard = LocalResource::new(move || {
        let settings = settings.get();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_workstreams_dashboard(settings).await }
    });

    Effect::new({
        let workspace = workspace.clone();
        move |_| workspace.update(|state| state.active_section = WorkspaceSection::Workstreams)
    });

    view! {
        <PageFrame
            title="Workstreams"
            subtitle="Authority hierarchy grouped by workstream, work unit, and Codex thread"
        >
            <div class="toolbar">
                <button class="refresh-button" on:click=move |_| dashboard.refetch()>"Refresh"</button>
                <span class="muted">
                    "Live status joins authority hierarchy with daemon assignment and supervisor state."
                </span>
            </div>
            {move || match action_message.get() {
                Some(message) => view! {
                    <div class="info-panel">
                        <strong>"Saved"</strong>
                        <p>{message}</p>
                    </div>
                }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match action_error.get() {
                Some(error) => view! { <ErrorPanel error=error /> }.into_any(),
                None => view! {}.into_any(),
            }}
            <article class="card">
                <h3>"Create workstream"</h3>
                <div class="action-form">
                    <label class="field">
                        <span>"Title"</span>
                        <input
                            type="text"
                            placeholder="Close operator-web branch"
                            prop:value=move || create_title.get()
                            on:input=move |ev| create_title.set(event_target_value(&ev))
                        />
                    </label>
                    <label class="field">
                        <span>"Objective"</span>
                        <textarea
                            rows="3"
                            placeholder="Ship the dashboard slice for workstream and Codex thread management."
                            prop:value=move || create_objective.get()
                            on:input=move |ev| create_objective.set(event_target_value(&ev))
                        ></textarea>
                    </label>
                    <div class="section-grid">
                        <label class="field">
                            <span>"Priority"</span>
                            <input
                                type="text"
                                placeholder="P2"
                                prop:value=move || create_priority.get()
                                on:input=move |ev| create_priority.set(event_target_value(&ev))
                            />
                        </label>
                        <label class="field">
                            <span>"Status"</span>
                            <select
                                prop:value=move || create_status.get()
                                on:change=move |ev| create_status.set(event_target_value(&ev))
                            >
                                {workstream_status_options()
                                    .into_iter()
                                    .map(|(value, label)| view! { <option value=value>{label}</option> })
                                    .collect_view()}
                            </select>
                        </label>
                    </div>
                    <div class="action-buttons">
                        <button
                            class="primary-button"
                            disabled=move || create_working.get()
                            on:click=move |_| {
                                let settings = settings.get_untracked();
                                let title = create_title.get_untracked();
                                let objective = create_objective.get_untracked();
                                let priority = create_priority.get_untracked();
                                let status = create_status.get_untracked();
                                create_working.set(true);
                                action_error.set(None);
                                #[cfg(target_arch = "wasm32")]
                                spawn_local(async move {
                                    let result = parse_workstream_status(&status)
                                        .and_then(|parsed_status| {
                                            Ok((parsed_status, title, objective, priority))
                                        });
                                    match result {
                                        Ok((parsed_status, title, objective, priority)) => {
                                            match api::create_workstream(
                                                settings,
                                                title,
                                                objective,
                                                parsed_status,
                                                priority,
                                            )
                                            .await
                                            {
                                                Ok(()) => {
                                                    create_title.set(String::new());
                                                    create_objective.set(String::new());
                                                    create_priority.set("P2".to_string());
                                                    create_status.set("active".to_string());
                                                    action_message.set(Some(
                                                        "Created workstream.".to_string(),
                                                    ));
                                                    refresh_epoch.update(|value| *value += 1);
                                                }
                                                Err(error) => action_error.set(Some(error)),
                                            }
                                        }
                                        Err(error) => action_error.set(Some(error)),
                                    }
                                    create_working.set(false);
                                });
                            }
                        >
                            "Create workstream"
                        </button>
                    </div>
                </div>
            </article>
            {move || match dashboard.get() {
                None => view! { <p class="muted">"Loading workstreams…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(dashboard)) => {
                    if dashboard.hierarchy.workstreams.is_empty() {
                        view! {
                            <EmptyState
                                title="No workstreams yet"
                                body="Create a workstream to start grouping work units and Codex threads."
                            />
                        }
                        .into_any()
                    } else {
                        let workstream_nodes = dashboard.hierarchy.workstreams.clone();
                        view! {
                            <div class="stack">
                                {workstream_nodes
                                    .into_iter()
                                    .map(|node| {
                                        view! {
                                            <WorkstreamCard
                                                node
                                                dashboard=dashboard.clone()
                                                settings
                                                refresh_epoch
                                                action_message
                                                action_error
                                            />
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                        .into_any()
                    }
                }
            }}
        </PageFrame>
    }
}

#[component]
fn ThreadsRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let refresh_epoch = RwSignal::new(0u64);
    let action_message = RwSignal::new(None::<String>);
    let action_error = RwSignal::new(None::<String>);
    let dashboard = LocalResource::new(move || {
        let settings = settings.get();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_workstreams_dashboard(settings).await }
    });

    Effect::new({
        let workspace = workspace.clone();
        move |_| workspace.update(|state| state.active_section = WorkspaceSection::Threads)
    });

    view! {
        <PageFrame
            title="Codex Threads"
            subtitle="Live Codex app-server threads joined with Orcas runtime assignment state and authority bindings"
        >
            <div class="toolbar">
                <button class="refresh-button" on:click=move |_| dashboard.refetch()>"Refresh"</button>
                <span class="muted">
                    "Inspect live thread state, attach existing threads to work units, and pause or resume Orcas-managed assignments."
                </span>
            </div>
            {move || match action_message.get() {
                Some(message) => view! {
                    <div class="info-panel">
                        <strong>"Updated"</strong>
                        <p>{message}</p>
                    </div>
                }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match action_error.get() {
                Some(error) => view! { <ErrorPanel error=error /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match dashboard.get() {
                None => view! { <p class="muted">"Loading live threads…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(dashboard)) => {
                    if dashboard.snapshot.threads.is_empty() {
                        view! {
                            <EmptyState
                                title="No live Codex threads"
                                body="The daemon is connected, but no thread summaries are currently available from the app-server."
                            />
                        }.into_any()
                    } else {
                        let threads = dashboard.snapshot.threads.clone();
                        view! {
                            <div class="stack">
                                {threads.into_iter().map(|thread| {
                                    view! {
                                        <LiveThreadCard
                                            thread
                                            dashboard=dashboard.clone()
                                            settings
                                            refresh_epoch
                                            action_message
                                            action_error
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }
            }}
        </PageFrame>
    }
}

#[component]
fn LiveThreadCard(
    thread: orcas_core::ipc::ThreadSummary,
    dashboard: WorkstreamsDashboardData,
    settings: RwSignal<OperatorServerSettings>,
    refresh_epoch: RwSignal<u64>,
    action_message: RwSignal<Option<String>>,
    action_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let linkage = live_thread_linkage(&thread, &dashboard);
    let inspecting = RwSignal::new(false);
    let attaching = RwSignal::new(false);
    let working = RwSignal::new(false);
    let detail = RwSignal::new(None::<orcas_core::ipc::ThreadView>);
    let attach_work_unit = RwSignal::new(String::new());
    let attach_title = RwSignal::new(
        thread
            .name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("Codex {}", &thread.id[..thread.id.len().min(8)])),
    );
    let attach_notes = RwSignal::new(String::new());
    let attach_thread_id = thread.id.clone();
    let attach_thread_cwd = thread.cwd.clone();
    let work_unit_options = dashboard
        .hierarchy
        .workstreams
        .iter()
        .flat_map(|workstream| {
            workstream.work_units.iter().map(move |work_unit| {
                (
                    work_unit.work_unit.id.to_string(),
                    format!("{} / {}", workstream.workstream.title, work_unit.work_unit.title),
                )
            })
        })
        .collect::<Vec<_>>();

    view! {
        <article class="card">
            <div class="page-header">
                <div>
                    <p class="eyebrow">{thread.status.clone()}</p>
                    <h3>{thread.name.clone().unwrap_or_else(|| thread.id.clone())}</h3>
                    <p class="item-meta">
                        {format!(
                            "{} · scope {} · loaded {} · monitor {}",
                            thread.model_provider,
                            thread.scope,
                            humanize_snake_case(
                                serde_json::to_string(&thread.loaded_status)
                                    .unwrap_or_default()
                                    .trim_matches('"')
                            ),
                            humanize_snake_case(
                                serde_json::to_string(&thread.monitor_state)
                                    .unwrap_or_default()
                                    .trim_matches('"')
                            )
                        )}
                    </p>
                </div>
                <div class="action-buttons">
                    <button class="refresh-button" on:click=move |_| inspecting.update(|value| *value = !*value)>
                        {move || if inspecting.get() { "Hide details" } else { "Inspect" }}
                    </button>
                    <button class="refresh-button" on:click=move |_| attaching.update(|value| *value = !*value)>
                        {move || if attaching.get() { "Close attach" } else { "Attach to work unit" }}
                    </button>
                    {move || match linkage.codex_assignment.clone() {
                        Some(codex_assignment) if matches!(codex_assignment.status, orcas_core::CodexThreadAssignmentStatus::Active) => view! {
                            <button
                                class="refresh-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let assignment_id = codex_assignment.assignment_id.clone();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match api::pause_codex_assignment(settings, assignment_id).await {
                                            Ok(()) => {
                                                action_message.set(Some("Paused Codex assignment.".to_string()));
                                                refresh_epoch.update(|value| *value += 1);
                                            }
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Pause"
                            </button>
                        }.into_any(),
                        Some(codex_assignment) if matches!(codex_assignment.status, orcas_core::CodexThreadAssignmentStatus::Paused) => view! {
                            <button
                                class="refresh-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let assignment_id = codex_assignment.assignment_id.clone();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match api::resume_codex_assignment(settings, assignment_id).await {
                                            Ok(()) => {
                                                action_message.set(Some("Resumed Codex assignment.".to_string()));
                                                refresh_epoch.update(|value| *value += 1);
                                            }
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Resume"
                            </button>
                        }.into_any(),
                        _ => view! {}.into_any(),
                    }}
                </div>
            </div>
            <p class="item-summary">{thread.preview.clone()}</p>
            <p class="item-meta">{format!("thread {} · cwd {}", thread.id, if thread.cwd.is_empty() { "(none)" } else { thread.cwd.as_str() })}</p>
            <p class="item-meta">
                {format!(
                    "in_flight {}{}{}",
                    thread.turn_in_flight,
                    thread.active_turn_id.as_ref().map(|value| format!(" · active turn {}", value)).unwrap_or_default(),
                    thread.recent_event.as_ref().map(|value| format!(" · {}", value)).unwrap_or_default()
                )}
            </p>
            {move || match linkage.tracked_thread.clone() {
                Some(tracked_thread) => view! {
                    <p class="item-meta">{format!("bound tracked thread {} · work unit {}", tracked_thread.id, tracked_thread.work_unit_id)}</p>
                }.into_any(),
                None => view! { <p class="item-meta">"No authority binding yet"</p> }.into_any(),
            }}
            {move || match linkage.assignment.clone() {
                Some(assignment) => view! {
                    <p class="item-meta">{format!("assignment {} · worker {} · {}", assignment.id, assignment.worker_id, humanize_snake_case(serde_json::to_string(&assignment.status).unwrap_or_default().trim_matches('\"')))}</p>
                }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match linkage.open_decision.clone() {
                Some(decision) => view! {
                    <p class="item-meta">{format!("supervisor decision {} · {}", decision.decision_id, humanize_snake_case(serde_json::to_string(&decision.status).unwrap_or_default().trim_matches('\"')))}</p>
                }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || {
                if attaching.get() {
                    let work_unit_options = work_unit_options.clone();
                    let attach_thread_id = attach_thread_id.clone();
                    let attach_thread_cwd = attach_thread_cwd.clone();
                    view! {
                        <div class="action-form">
                            <label class="field">
                                <span>"Work unit"</span>
                                <select
                                    prop:value=move || attach_work_unit.get()
                                    on:change=move |ev| attach_work_unit.set(event_target_value(&ev))
                                >
                                    <option value="">"Select work unit"</option>
                                    {work_unit_options.iter().map(|(value, label)| view! {
                                        <option value=value.clone()>{label.clone()}</option>
                                    }).collect_view()}
                                </select>
                            </label>
                            <label class="field">
                                <span>"Tracked thread title"</span>
                                <input
                                    type="text"
                                    prop:value=move || attach_title.get()
                                    on:input=move |ev| attach_title.set(event_target_value(&ev))
                                />
                            </label>
                            <label class="field">
                                <span>"Notes"</span>
                                <textarea
                                    rows="2"
                                    prop:value=move || attach_notes.get()
                                    on:input=move |ev| attach_notes.set(event_target_value(&ev))
                                ></textarea>
                            </label>
                            <div class="action-buttons">
                                <button
                                    class="primary-button"
                                    disabled=move || working.get()
                                    on:click=move |_| {
                                        let settings = settings.get_untracked();
                                        let work_unit_id = attach_work_unit.get_untracked();
                                        let title = attach_title.get_untracked();
                                        let notes = attach_notes.get_untracked();
                                        let upstream_thread_id = attach_thread_id.clone();
                                        let cwd = if attach_thread_cwd.is_empty() { None } else { Some(attach_thread_cwd.clone()) };
                                        working.set(true);
                                        action_error.set(None);
                                        #[cfg(target_arch = "wasm32")]
                                        spawn_local(async move {
                                            match authority::WorkUnitId::parse(work_unit_id.as_str()) {
                                                Ok(work_unit_id) => match api::create_tracked_thread(
                                                    settings,
                                                    work_unit_id,
                                                    title,
                                                    Some(upstream_thread_id),
                                                    if notes.trim().is_empty() { None } else { Some(notes) },
                                                    cwd,
                                                ).await {
                                                    Ok(()) => {
                                                        action_message.set(Some("Attached live Codex thread to work unit.".to_string()));
                                                        refresh_epoch.update(|value| *value += 1);
                                                        attaching.set(false);
                                                    }
                                                    Err(error) => action_error.set(Some(error)),
                                                },
                                                Err(error) => action_error.set(Some(error.to_string())),
                                            }
                                            working.set(false);
                                        });
                                    }
                                >
                                    "Attach live thread"
                                </button>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            {move || {
                if inspecting.get() {
                    if detail.get().is_none() {
                        let settings = settings.get_untracked();
                        let thread_id = thread.id.clone();
                        action_error.set(None);
                        #[cfg(target_arch = "wasm32")]
                        spawn_local(async move {
                            match api::load_thread_detail(settings, thread_id).await {
                                Ok(response) => detail.set(Some(response.thread)),
                                Err(error) => action_error.set(Some(error)),
                            }
                        });
                    }
                    match detail.get() {
                        Some(detail) => view! {
                            <div class="stack">
                                <div class="detail-block">
                                    <p class="eyebrow">"Thread summary"</p>
                                    <dl class="detail-grid">
                                        <div>
                                            <dt>"Thread"</dt>
                                            <dd>{detail.summary.id.clone()}</dd>
                                        </div>
                                        <div>
                                            <dt>"Status"</dt>
                                            <dd>{detail.summary.status.clone()}</dd>
                                        </div>
                                        <div>
                                            <dt>"Loaded"</dt>
                                            <dd>{humanize_snake_case(
                                                serde_json::to_string(&detail.summary.loaded_status)
                                                    .unwrap_or_default()
                                                    .trim_matches('"')
                                            )}</dd>
                                        </div>
                                        <div>
                                            <dt>"Monitor"</dt>
                                            <dd>{humanize_snake_case(
                                                serde_json::to_string(&detail.summary.monitor_state)
                                                    .unwrap_or_default()
                                                    .trim_matches('"')
                                            )}</dd>
                                        </div>
                                        <div>
                                            <dt>"Recent event"</dt>
                                            <dd>{detail.summary.recent_event.clone().unwrap_or_else(|| "None".to_string())}</dd>
                                        </div>
                                        <div>
                                            <dt>"Updated"</dt>
                                            <dd>{format_unix_millis(detail.summary.updated_at)}</dd>
                                        </div>
                                    </dl>
                                    <p class="item-summary">{detail.summary.preview.clone()}</p>
                                    {detail.summary.raw_summary.clone().map(|raw_summary| view! {
                                        <div class="json-panel">
                                            <details>
                                                <summary>"Show raw thread summary"</summary>
                                                <JsonValueTree value=raw_summary />
                                            </details>
                                        </div>
                                    })}
                                </div>
                                {detail
                                    .turns
                                    .into_iter()
                                    .rev()
                                    .take(3)
                                    .map(|turn| view! { <ThreadTurnCard turn /> })
                                    .collect_view()}
                            </div>
                        }.into_any(),
                        None => view! { <p class="muted">"Loading thread detail…"</p> }.into_any(),
                    }
                } else {
                    view! {}.into_any()
                }
            }}
        </article>
    }
}

#[component]
fn WorkstreamCard(
    node: authority::WorkstreamNode,
    dashboard: WorkstreamsDashboardData,
    settings: RwSignal<OperatorServerSettings>,
    refresh_epoch: RwSignal<u64>,
    action_message: RwSignal<Option<String>>,
    action_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let workstream_id = node.workstream.id.clone();
    let workstream_revision = node.workstream.revision;
    let workstream_title_display = node.workstream.title.clone();
    let workstream_objective_display = node.workstream.objective.clone();
    let workstream_priority_display = node.workstream.priority.clone();
    let workstream_status_display = node.workstream.status;
    let work_unit_nodes = node.work_units.clone();
    let delete_workstream_id = workstream_id.clone();
    let edit_workstream_root_id = workstream_id.clone();
    let create_work_unit_root_id = workstream_id.clone();
    let editing = RwSignal::new(false);
    let adding_work_unit = RwSignal::new(false);
    let working = RwSignal::new(false);
    let title = RwSignal::new(workstream_title_display.clone());
    let objective = RwSignal::new(workstream_objective_display.clone());
    let priority = RwSignal::new(workstream_priority_display.clone());
    let status = RwSignal::new(workstream_status_value(workstream_status_display).to_string());
    let unit_title = RwSignal::new(String::new());
    let unit_task_statement = RwSignal::new(String::new());
    let unit_status = RwSignal::new("ready".to_string());

    view! {
        <article class="card">
            <div class="page-header">
                <div>
                    <p class="eyebrow">{format!("{} · {}", workstream_status_label(workstream_status_display), workstream_priority_display)}</p>
                    <h3>{workstream_title_display.clone()}</h3>
                    <p class="item-summary">{workstream_objective_display.clone()}</p>
                </div>
                <div class="action-buttons">
                    <button class="refresh-button" on:click=move |_| editing.update(|value| *value = !*value)>
                        {move || if editing.get() { "Close edit" } else { "Edit" }}
                    </button>
                    <button class="refresh-button" on:click=move |_| adding_work_unit.update(|value| *value = !*value)>
                        {move || if adding_work_unit.get() { "Close work unit form" } else { "Add work unit" }}
                    </button>
                    <button
                        class="refresh-button"
                        disabled=move || working.get()
                        on:click=move |_| {
                            let settings = settings.get_untracked();
                            let workstream_id = delete_workstream_id.clone();
                            working.set(true);
                            action_error.set(None);
                            #[cfg(target_arch = "wasm32")]
                            spawn_local(async move {
                                match api::delete_workstream(settings, workstream_id).await {
                                    Ok(()) => {
                                        action_message.set(Some("Deleted workstream.".to_string()));
                                        refresh_epoch.update(|value| *value += 1);
                                    }
                                    Err(error) => action_error.set(Some(error)),
                                }
                                working.set(false);
                            });
                        }
                    >
                        "Delete"
                    </button>
                </div>
            </div>
            {move || {
                let save_workstream_id = edit_workstream_root_id.clone();
                if editing.get() {
                    view! {
                    <div class="action-form">
                        <label class="field">
                            <span>"Title"</span>
                            <input
                                type="text"
                                prop:value=move || title.get()
                                on:input=move |ev| title.set(event_target_value(&ev))
                            />
                        </label>
                        <label class="field">
                            <span>"Objective"</span>
                            <textarea
                                rows="3"
                                prop:value=move || objective.get()
                                on:input=move |ev| objective.set(event_target_value(&ev))
                            ></textarea>
                        </label>
                        <div class="section-grid">
                            <label class="field">
                                <span>"Priority"</span>
                                <input
                                    type="text"
                                    prop:value=move || priority.get()
                                    on:input=move |ev| priority.set(event_target_value(&ev))
                                />
                            </label>
                            <label class="field">
                                <span>"Status"</span>
                                <select
                                    prop:value=move || status.get()
                                    on:change=move |ev| status.set(event_target_value(&ev))
                                >
                                    {workstream_status_options()
                                        .into_iter()
                                        .map(|(value, label)| view! { <option value=value>{label}</option> })
                                        .collect_view()}
                                </select>
                            </label>
                        </div>
                        <div class="action-buttons">
                            <button
                                class="primary-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let workstream_id = save_workstream_id.clone();
                                    let expected_revision = workstream_revision;
                                    let title = title.get_untracked();
                                    let objective = objective.get_untracked();
                                    let priority = priority.get_untracked();
                                    let status = status.get_untracked();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match parse_workstream_status(&status) {
                                            Ok(status) => match api::edit_workstream(
                                                settings,
                                                workstream_id,
                                                expected_revision,
                                                title,
                                                objective,
                                                status,
                                                priority,
                                            )
                                            .await
                                            {
                                                Ok(()) => {
                                                    action_message.set(Some(
                                                        "Updated workstream.".to_string(),
                                                    ));
                                                    refresh_epoch.update(|value| *value += 1);
                                                    editing.set(false);
                                                }
                                                Err(error) => action_error.set(Some(error)),
                                            },
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Save workstream"
                            </button>
                        </div>
                    </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            {move || {
                let create_workstream_id = create_work_unit_root_id.clone();
                if adding_work_unit.get() {
                    view! {
                    <div class="action-form">
                        <label class="field">
                            <span>"Work unit title"</span>
                            <input
                                type="text"
                                prop:value=move || unit_title.get()
                                on:input=move |ev| unit_title.set(event_target_value(&ev))
                            />
                        </label>
                        <label class="field">
                            <span>"Task statement"</span>
                            <textarea
                                rows="3"
                                prop:value=move || unit_task_statement.get()
                                on:input=move |ev| unit_task_statement.set(event_target_value(&ev))
                            ></textarea>
                        </label>
                        <label class="field">
                            <span>"Status"</span>
                            <select
                                prop:value=move || unit_status.get()
                                on:change=move |ev| unit_status.set(event_target_value(&ev))
                            >
                                {workunit_status_options()
                                    .into_iter()
                                    .map(|(value, label)| view! { <option value=value>{label}</option> })
                                    .collect_view()}
                            </select>
                        </label>
                        <div class="action-buttons">
                            <button
                                class="primary-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let workstream_id = create_workstream_id.clone();
                                    let title = unit_title.get_untracked();
                                    let task_statement = unit_task_statement.get_untracked();
                                    let status = unit_status.get_untracked();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match parse_workunit_status(&status) {
                                            Ok(status) => match api::create_work_unit(
                                                settings,
                                                workstream_id,
                                                title,
                                                task_statement,
                                                status,
                                            )
                                            .await
                                            {
                                                Ok(()) => {
                                                    unit_title.set(String::new());
                                                    unit_task_statement.set(String::new());
                                                    unit_status.set("ready".to_string());
                                                    action_message.set(Some(
                                                        "Created work unit.".to_string(),
                                                    ));
                                                    refresh_epoch.update(|value| *value += 1);
                                                    adding_work_unit.set(false);
                                                }
                                                Err(error) => action_error.set(Some(error)),
                                            },
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Create work unit"
                            </button>
                        </div>
                    </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            <div class="stack">
                {work_unit_nodes.into_iter().map(|work_unit_node| {
                    view! {
                        <WorkUnitCard
                            node=work_unit_node
                            dashboard=dashboard.clone()
                            settings
                            refresh_epoch
                            action_message
                            action_error
                        />
                    }
                }).collect_view()}
            </div>
        </article>
    }
}

#[component]
fn WorkUnitCard(
    node: authority::WorkUnitNode,
    dashboard: WorkstreamsDashboardData,
    settings: RwSignal<OperatorServerSettings>,
    refresh_epoch: RwSignal<u64>,
    action_message: RwSignal<Option<String>>,
    action_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let work_unit_id = node.work_unit.id.clone();
    let work_unit_revision = node.work_unit.revision;
    let work_unit_title_display = node.work_unit.title.clone();
    let work_unit_status_display = node.work_unit.status;
    let tracked_threads = node.tracked_threads.clone();
    let has_tracked_threads = !tracked_threads.is_empty();
    let load_work_unit_id = work_unit_id.clone();
    let delete_work_unit_id = work_unit_id.clone();
    let edit_work_unit_root_id = work_unit_id.clone();
    let add_thread_root_work_unit_id = work_unit_id.clone();
    let start_work_unit_id = work_unit_id.clone();
    let editing = RwSignal::new(false);
    let adding_thread = RwSignal::new(false);
    let starting = RwSignal::new(false);
    let working = RwSignal::new(false);
    let edit_title = RwSignal::new(work_unit_title_display.clone());
    let edit_task = RwSignal::new(String::new());
    let edit_status = RwSignal::new(workunit_status_value(work_unit_status_display).to_string());
    let edit_loaded = RwSignal::new(false);
    let thread_title = RwSignal::new(String::new());
    let thread_upstream = RwSignal::new(String::new());
    let thread_notes = RwSignal::new(String::new());
    let start_worker_id = RwSignal::new("codex-worker".to_string());
    let start_cwd = RwSignal::new(String::new());
    let start_model = RwSignal::new(String::new());
    let start_instructions = RwSignal::new(String::new());
    let auto_bind_tracked_thread =
        StoredValue::new((tracked_threads.len() == 1).then(|| tracked_threads[0].clone()));
    let runtime_assignments = dashboard
        .snapshot
        .collaboration
        .assignments
        .iter()
        .filter(|assignment| assignment.work_unit_id == work_unit_id.as_str())
        .cloned()
        .collect::<Vec<_>>();
    let runtime_assignment_cards = runtime_assignments.clone();

    view! {
        <article class="card">
            <div class="page-header">
                <div>
                    <p class="eyebrow">{workunit_status_label(work_unit_status_display)}</p>
                    <h4>{work_unit_title_display.clone()}</h4>
                    <p class="item-meta">
                        {format!("{} tracked threads", tracked_threads.len())}
                    </p>
                </div>
                <div class="action-buttons">
                    <button
                        class="refresh-button"
                        on:click=move |_| {
                            editing.update(|value| *value = !*value);
                            if !edit_loaded.get_untracked() {
                                let settings = settings.get_untracked();
                                let work_unit_id = load_work_unit_id.clone();
                                action_error.set(None);
                                #[cfg(target_arch = "wasm32")]
                                spawn_local(async move {
                                    match api::load_work_unit(settings, work_unit_id).await {
                                        Ok(record) => {
                                            edit_title.set(record.title);
                                            edit_task.set(record.task_statement);
                                            edit_status.set(workunit_status_value(record.status).to_string());
                                            edit_loaded.set(true);
                                        }
                                        Err(error) => action_error.set(Some(error)),
                                    }
                                });
                            }
                        }
                    >
                        {move || if editing.get() { "Close edit" } else { "Edit" }}
                    </button>
                    <button class="refresh-button" on:click=move |_| adding_thread.update(|value| *value = !*value)>
                        {move || if adding_thread.get() { "Close thread form" } else { "Add Codex thread" }}
                    </button>
                    <button class="refresh-button" on:click=move |_| starting.update(|value| *value = !*value)>
                        {move || if starting.get() { "Close start form" } else { "Start Codex run" }}
                    </button>
                    <button
                        class="refresh-button"
                        disabled=move || working.get()
                        on:click=move |_| {
                            let settings = settings.get_untracked();
                            let work_unit_id = delete_work_unit_id.clone();
                            working.set(true);
                            action_error.set(None);
                            #[cfg(target_arch = "wasm32")]
                            spawn_local(async move {
                                match api::delete_work_unit(settings, work_unit_id).await {
                                    Ok(()) => {
                                        action_message.set(Some("Deleted work unit.".to_string()));
                                        refresh_epoch.update(|value| *value += 1);
                                    }
                                    Err(error) => action_error.set(Some(error)),
                                }
                                working.set(false);
                            });
                        }
                    >
                        "Delete"
                    </button>
                </div>
            </div>
            {move || {
                let save_work_unit_id = edit_work_unit_root_id.clone();
                if editing.get() {
                    view! {
                    <div class="action-form">
                        <label class="field">
                            <span>"Title"</span>
                            <input
                                type="text"
                                prop:value=move || edit_title.get()
                                on:input=move |ev| edit_title.set(event_target_value(&ev))
                            />
                        </label>
                        <label class="field">
                            <span>"Task statement"</span>
                            <textarea
                                rows="3"
                                prop:value=move || edit_task.get()
                                on:input=move |ev| edit_task.set(event_target_value(&ev))
                            ></textarea>
                        </label>
                        <label class="field">
                            <span>"Status"</span>
                            <select
                                prop:value=move || edit_status.get()
                                on:change=move |ev| edit_status.set(event_target_value(&ev))
                            >
                                {workunit_status_options()
                                    .into_iter()
                                    .map(|(value, label)| view! { <option value=value>{label}</option> })
                                    .collect_view()}
                            </select>
                        </label>
                        <div class="action-buttons">
                            <button
                                class="primary-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let work_unit_id = save_work_unit_id.clone();
                                    let expected_revision = work_unit_revision;
                                    let title = edit_title.get_untracked();
                                    let task = edit_task.get_untracked();
                                    let status = edit_status.get_untracked();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match parse_workunit_status(&status) {
                                            Ok(status) => match api::edit_work_unit(
                                                settings,
                                                work_unit_id,
                                                expected_revision,
                                                title,
                                                task,
                                                status,
                                            )
                                            .await
                                            {
                                                Ok(()) => {
                                                    action_message.set(Some(
                                                        "Updated work unit.".to_string(),
                                                    ));
                                                    refresh_epoch.update(|value| *value += 1);
                                                    editing.set(false);
                                                }
                                                Err(error) => action_error.set(Some(error)),
                                            },
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Save work unit"
                            </button>
                        </div>
                    </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            {move || {
                let start_work_unit_id = start_work_unit_id.clone();
                if starting.get() {
                    view! {
                    <div class="action-form">
                        <label class="field">
                            <span>"Working directory"</span>
                            <input
                                type="text"
                                placeholder="/home/emmy/openai/orcas"
                                prop:value=move || start_cwd.get()
                                on:input=move |ev| start_cwd.set(event_target_value(&ev))
                            />
                        </label>
                        <div class="section-grid">
                            <label class="field">
                                <span>"Model"</span>
                                <input
                                    type="text"
                                    placeholder="gpt-5.4"
                                    prop:value=move || start_model.get()
                                    on:input=move |ev| start_model.set(event_target_value(&ev))
                                />
                            </label>
                            <label class="field">
                                <span>"Worker id"</span>
                                <input
                                    type="text"
                                    placeholder="codex-worker"
                                    prop:value=move || start_worker_id.get()
                                    on:input=move |ev| start_worker_id.set(event_target_value(&ev))
                                />
                            </label>
                        </div>
                        <label class="field">
                            <span>"Instructions override"</span>
                            <textarea
                                rows="3"
                                placeholder="Optional assignment instructions override"
                                prop:value=move || start_instructions.get()
                                on:input=move |ev| start_instructions.set(event_target_value(&ev))
                            ></textarea>
                        </label>
                        <div class="action-buttons">
                            <button
                                class="primary-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let work_unit_id = start_work_unit_id.clone();
                                    let auto_bind_tracked_thread = auto_bind_tracked_thread.get_value();
                                    let worker_id = start_worker_id.get_untracked();
                                    let cwd = start_cwd.get_untracked();
                                    let model = start_model.get_untracked();
                                    let instructions = start_instructions.get_untracked();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        let start_settings = settings.clone();
                                        let start_cwd_value = cwd.clone();
                                        match api::assignment_start(
                                            settings,
                                            work_unit_id.to_string(),
                                            worker_id,
                                            Some(cwd),
                                            Some(model),
                                            Some(instructions),
                                        )
                                        .await
                                        {
                                            Ok(response) => {
                                                if let (Some(live_thread_id), Some(tracked_thread)) = (
                                                    response.worker_session.thread_id.clone(),
                                                    auto_bind_tracked_thread.clone(),
                                                ) {
                                                    if tracked_thread.upstream_thread_id.as_deref()
                                                        != Some(live_thread_id.as_str())
                                                    {
                                                        if let Err(error) = api::bind_tracked_thread(
                                                            start_settings,
                                                            tracked_thread.id.clone(),
                                                            tracked_thread.revision,
                                                            live_thread_id,
                                                            if start_cwd_value.trim().is_empty() {
                                                                None
                                                            } else {
                                                                Some(start_cwd_value)
                                                            },
                                                        )
                                                        .await
                                                        {
                                                            action_error.set(Some(error));
                                                        }
                                                    }
                                                }
                                                action_message.set(Some(format!(
                                                    "Started assignment {}.",
                                                    response.assignment.id
                                                )));
                                                refresh_epoch.update(|value| *value += 1);
                                                starting.set(false);
                                            }
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Start work unit"
                            </button>
                        </div>
                    </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            {move || {
                let add_thread_work_unit_id = add_thread_root_work_unit_id.clone();
                if adding_thread.get() {
                    view! {
                    <div class="action-form">
                        <label class="field">
                            <span>"Thread title"</span>
                            <input
                                type="text"
                                placeholder="Codex lane A"
                                prop:value=move || thread_title.get()
                                on:input=move |ev| thread_title.set(event_target_value(&ev))
                            />
                        </label>
                        <label class="field">
                            <span>"Codex thread id"</span>
                            <input
                                type="text"
                                placeholder="Optional existing upstream thread id"
                                prop:value=move || thread_upstream.get()
                                on:input=move |ev| thread_upstream.set(event_target_value(&ev))
                            />
                        </label>
                        <label class="field">
                            <span>"Notes"</span>
                            <textarea
                                rows="2"
                                prop:value=move || thread_notes.get()
                                on:input=move |ev| thread_notes.set(event_target_value(&ev))
                            ></textarea>
                        </label>
                        <div class="action-buttons">
                            <button
                                class="primary-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let work_unit_id = add_thread_work_unit_id.clone();
                                    let title = thread_title.get_untracked();
                                    let upstream = thread_upstream.get_untracked();
                                    let notes = thread_notes.get_untracked();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match api::create_tracked_thread(
                                            settings,
                                            work_unit_id,
                                            title,
                                            Some(upstream),
                                            if notes.trim().is_empty() { None } else { Some(notes) },
                                            None,
                                        )
                                        .await
                                        {
                                            Ok(()) => {
                                                thread_title.set(String::new());
                                                thread_upstream.set(String::new());
                                                thread_notes.set(String::new());
                                                action_message.set(Some(
                                                    "Added Codex thread to work unit.".to_string(),
                                                ));
                                                refresh_epoch.update(|value| *value += 1);
                                                adding_thread.set(false);
                                            }
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Add Codex thread"
                            </button>
                        </div>
                    </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            <div class="stack">
                {tracked_threads.into_iter().map(|thread| {
                    view! {
                        <TrackedThreadCard
                            thread
                            dashboard=dashboard.clone()
                            settings
                            refresh_epoch
                            action_message
                            action_error
                        />
                    }
                }).collect_view()}
                {move || {
                    if !has_tracked_threads {
                        runtime_assignment_cards.clone().into_iter().map(|assignment| {
                            view! {
                                <RuntimeAssignmentCard
                                    work_unit_id=work_unit_id.clone()
                                    assignment
                                    dashboard=dashboard.clone()
                                    settings
                                    refresh_epoch
                                    action_message
                                    action_error
                                />
                            }
                        }).collect_view().into_any()
                    } else {
                        view! {}.into_any()
                    }
                }}
            </div>
        </article>
    }
}

#[component]
fn RuntimeAssignmentCard(
    work_unit_id: authority::WorkUnitId,
    assignment: orcas_core::ipc::AssignmentSummary,
    dashboard: WorkstreamsDashboardData,
    settings: RwSignal<OperatorServerSettings>,
    refresh_epoch: RwSignal<u64>,
    action_message: RwSignal<Option<String>>,
    action_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let navigate = use_navigate();
    let working = RwSignal::new(false);
    let showing_detail = RwSignal::new(false);
    let loading_detail = RwSignal::new(false);
    let detail = RwSignal::new(None::<orcas_core::ipc::ThreadView>);
    let inferred_thread = inferred_live_thread_for_assignment(&assignment, &dashboard);
    let latest_report = dashboard
        .snapshot
        .collaboration
        .reports
        .iter()
        .filter(|report| report.assignment_id == assignment.id)
        .max_by_key(|report| report.created_at)
        .cloned();
    let latest_decision = latest_report.as_ref().and_then(|report| {
        dashboard
            .snapshot
            .collaboration
            .decisions
            .iter()
            .filter(|decision| decision.report_id.as_deref() == Some(report.id.as_str()))
            .max_by_key(|decision| decision.created_at)
            .cloned()
    });
    let work_unit_proposal = dashboard
        .snapshot
        .collaboration
        .work_units
        .iter()
        .find(|work_unit| work_unit.id == work_unit_id.as_str())
        .and_then(|work_unit| work_unit.proposal.clone());
    let actionable_inbox_item = dashboard
        .snapshot
        .operator_inbox
        .items
        .iter()
        .filter(|item| item.work_unit_id.as_deref() == Some(work_unit_id.as_str()))
        .max_by_key(|item| item.updated_at)
        .cloned();
    let headline = if assignment.status == orcas_core::AssignmentStatus::AwaitingDecision {
        "Waiting for supervisor".to_string()
    } else {
        humanize_snake_case(
            serde_json::to_string(&assignment.status)
                .unwrap_or_default()
                .trim_matches('"'),
        )
    };
    let mut detail_parts = Vec::new();
    if let Some(thread) = inferred_thread.as_ref() {
        if assignment.status == orcas_core::AssignmentStatus::AwaitingDecision && thread.status == "idle" {
            detail_parts.push("report submitted".to_string());
        } else {
            detail_parts.push(format!("thread {}", thread.status));
        }
        if thread.turn_in_flight {
            detail_parts.push("turn in flight".to_string());
        }
    } else {
        detail_parts.push("runtime lane".to_string());
    }
    detail_parts.push(format!(
        "assignment {}",
        humanize_snake_case(
            serde_json::to_string(&assignment.status)
                .unwrap_or_default()
                .trim_matches('"')
        )
    ));
    let supervisor_summary = lane_activity_summary(
        latest_report.as_ref(),
        work_unit_proposal.as_ref(),
        latest_decision.as_ref(),
    );
    let inferred_thread_id_for_detail = inferred_thread.as_ref().map(|thread| thread.id.clone());
    let has_inferred_thread = inferred_thread.is_some();

    view! {
        <div class="item-card">
            <div class="item-card-topline">
                <span class="status-pill">{headline}</span>
                <span class="muted">"runtime lane"</span>
            </div>
            <p class="item-title">{assignment.worker_id.clone()}</p>
            <p class="item-summary">{supervisor_summary}</p>
            <p class="item-meta">{detail_parts.join(" · ")}</p>
            {match inferred_thread.as_ref() {
                Some(thread) => view! {
                    <p class="item-meta">
                        {format!(
                            "codex lane {}{}",
                            thread.id,
                            if thread.cwd.is_empty() {
                                "".to_string()
                            } else {
                                format!(" · {}", thread.cwd)
                            }
                        )}
                    </p>
                }.into_any(),
                None => view! {
                    <p class="item-meta">"No inferred live Codex thread yet"</p>
                }.into_any(),
            }}
            <div class="action-buttons">
                {match inferred_thread.as_ref() {
                    Some(thread) => view! {
                        <a class="refresh-button" href="/threads">"Open in Threads"</a>
                        <span class="muted">{thread.id.clone()}</span>
                    }.into_any(),
                    None => view! {}.into_any(),
                }}
                {match actionable_inbox_item.clone() {
                    Some(item) => view! {
                        <a class="refresh-button" href={format!("/inbox/{}", item.id)}>
                            "View supervisor item"
                        </a>
                    }.into_any(),
                    None => view! {}.into_any(),
                }}
                {match (
                    latest_report.clone(),
                    work_unit_proposal.clone(),
                    actionable_inbox_item.clone(),
                ) {
                    (Some(report), None, None) if report.needs_supervisor_review => {
                        let work_unit_id = work_unit_id.to_string();
                        let report_id = report.id.clone();
                        view! {
                            <button
                                class="refresh-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let work_unit_id = work_unit_id.clone();
                                    let report_id = report_id.clone();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match api::proposal_create(
                                            settings,
                                            work_unit_id,
                                            Some(report_id),
                                            Some("Created from workstreams dashboard".to_string()),
                                        )
                                        .await
                                        {
                                            Ok(response) => {
                                                action_message.set(Some(format!(
                                                    "Created supervisor proposal {}.",
                                                    response.proposal.id
                                                )));
                                                refresh_epoch.update(|value| *value += 1);
                                            }
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Create proposal"
                            </button>
                        }.into_any()
                    }
                    _ => view! {}.into_any(),
                }}
                <button class="refresh-button" on:click=move |_| showing_detail.update(|value| *value = !*value)>
                    {move || if showing_detail.get() { "Hide inspect" } else { "Inspect lane" }}
                </button>
                {match actionable_inbox_item.clone() {
                    Some(item) => {
                        let available_actions = item.available_actions.clone();
                        let item_id = item.id.clone();
                        let item_updated_at = item.updated_at;
                        let item_source_kind = item.source_kind;
                        let actionable_object_id = item.actionable_object_id.clone();
                        available_actions
                            .into_iter()
                            .filter(|action_kind| {
                                matches!(
                                    action_kind,
                                    OperatorInboxActionKind::Approve | OperatorInboxActionKind::Reject
                                )
                            })
                            .map(|action_kind| {
                                let settings = settings;
                                let navigate = navigate.clone();
                                let item_id_for_action = item_id.clone();
                                let actionable_object_id = actionable_object_id.clone();
                                view! {
                                    <button
                                        class="refresh-button"
                                        disabled=move || working.get()
                                        on:click=move |_| {
                                            let settings = settings.get_untracked();
                                            let item_id = item_id_for_action.clone();
                                            let navigate = navigate.clone();
                                            let actionable_object_id = actionable_object_id.clone();
                                            working.set(true);
                                            action_error.set(None);
                                            #[cfg(target_arch = "wasm32")]
                                            spawn_local(async move {
                                                let result = if item_source_kind
                                                    == orcas_core::OperatorInboxSourceKind::SupervisorProposal
                                                {
                                                    match action_kind {
                                                        OperatorInboxActionKind::Approve => api::proposal_approve(
                                                            settings,
                                                            actionable_object_id.clone(),
                                                            Some("Approved from workstreams dashboard".to_string()),
                                                        )
                                                        .await
                                                        .map(|_| None),
                                                        OperatorInboxActionKind::Reject => api::proposal_reject(
                                                            settings,
                                                            actionable_object_id.clone(),
                                                            Some("Rejected from workstreams dashboard".to_string()),
                                                        )
                                                        .await
                                                        .map(|_| None),
                                                        _ => Ok(None),
                                                    }
                                                } else {
                                                    let idempotency_key = storage::remote_action_idempotency_key(
                                                        &settings.origin_node_id,
                                                        &item_id,
                                                        action_kind,
                                                        item_updated_at,
                                                    );
                                                    api::submit_remote_action(
                                                        settings,
                                                        item_id,
                                                        action_kind,
                                                        Some("web-operator".to_string()),
                                                        None,
                                                        Some(idempotency_key),
                                                    )
                                                    .await
                                                    .map(Some)
                                                };
                                                match result {
                                                    Ok(request) => {
                                                        action_message.set(Some(format!(
                                                            "{} submitted.",
                                                            action_kind_label(action_kind)
                                                        )));
                                                        refresh_epoch.update(|value| *value += 1);
                                                        if let Some(request) = request {
                                                            navigate(&format!("/actions/{}", request.request_id), Default::default());
                                                        }
                                                    }
                                                    Err(error) => action_error.set(Some(error)),
                                                }
                                                working.set(false);
                                            });
                                        }
                                    >
                                        {action_kind_label(action_kind)}
                                    </button>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                    None => view! {}.into_any(),
                }}
            </div>
            {move || {
                    if showing_detail.get() {
                        if detail.get().is_none() && !loading_detail.get() {
                            if let Some(thread_id) = inferred_thread_id_for_detail.clone() {
                                let settings = settings.get_untracked();
                                loading_detail.set(true);
                                action_error.set(None);
                                #[cfg(target_arch = "wasm32")]
                                spawn_local(async move {
                                    match api::load_thread_detail(settings, thread_id).await {
                                        Ok(response) => detail.set(Some(response.thread)),
                                        Err(error) => action_error.set(Some(error)),
                                    }
                                    loading_detail.set(false);
                                });
                            }
                        }
                    
                    view! {
                        <div class="detail-panel">
                            {match latest_report.clone() {
                                Some(report) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Supervisor state"</p>
                                        <dl class="detail-grid">
                                            <div><dt>"Disposition"</dt><dd>{humanize_snake_case(serde_json::to_string(&report.disposition).unwrap_or_default().trim_matches('"'))}</dd></div>
                                            <div><dt>"Parse result"</dt><dd>{humanize_snake_case(serde_json::to_string(&report.parse_result).unwrap_or_default().trim_matches('"'))}</dd></div>
                                            <div><dt>"Supervisor review"</dt><dd>{if report.needs_supervisor_review { "Required" } else { "Not required" }}</dd></div>
                                            <div><dt>"Recorded"</dt><dd>{format_timestamp(report.created_at)}</dd></div>
                                        </dl>
                                        <p class="item-summary">{report.summary.clone()}</p>
                                    </div>
                                }.into_any(),
                                None => view! { <div class="detail-block"><p class="eyebrow">"Latest report"</p><p class="item-meta">"No report recorded yet"</p></div> }.into_any(),
                            }}
                            {match work_unit_proposal.clone() {
                                Some(proposal) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Proposal"</p>
                                        <dl class="detail-grid">
                                            <div><dt>"Status"</dt><dd>{humanize_snake_case(serde_json::to_string(&proposal.latest_status).unwrap_or_default().trim_matches('"'))}</dd></div>
                                            <div><dt>"Proposal id"</dt><dd>{proposal.latest_proposal_id.clone()}</dd></div>
                                            <div><dt>"Created"</dt><dd>{format_timestamp(proposal.latest_created_at)}</dd></div>
                                            {proposal.latest_failure_stage.as_ref().map(|stage| view! {
                                                <div><dt>"Failure stage"</dt><dd>{humanize_snake_case(serde_json::to_string(stage).unwrap_or_default().trim_matches('"'))}</dd></div>
                                            })}
                                        </dl>
                                    </div>
                                }.into_any(),
                                None => view! {}.into_any(),
                            }}
                            {match latest_decision.clone() {
                                Some(decision) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Latest decision"</p>
                                        <dl class="detail-grid">
                                            <div><dt>"Decision id"</dt><dd>{decision.id.clone()}</dd></div>
                                            <div><dt>"Type"</dt><dd>{humanize_snake_case(serde_json::to_string(&decision.decision_type).unwrap_or_default().trim_matches('"'))}</dd></div>
                                            <div><dt>"Recorded"</dt><dd>{format_timestamp(decision.created_at)}</dd></div>
                                        </dl>
                                        <p class="item-summary">{decision.rationale.clone()}</p>
                                    </div>
                                }.into_any(),
                                None => view! {}.into_any(),
                            }}
                            {move || match detail.get() {
                                Some(detail) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Live thread monitor"</p>
                                        <dl class="detail-grid">
                                            <div><dt>"Thread"</dt><dd>{detail.summary.id.clone()}</dd></div>
                                            <div><dt>"Status"</dt><dd>{detail.summary.status.clone()}</dd></div>
                                            <div><dt>"Turns"</dt><dd>{detail.turns.len().to_string()}</dd></div>
                                            <div><dt>"Updated"</dt><dd>{format_unix_millis(detail.summary.updated_at)}</dd></div>
                                        </dl>
                                        <p class="item-summary">{detail.summary.preview.clone()}</p>
                                        <div class="detail-panel">
                                            {detail.turns.into_iter().rev().take(2).map(|turn| view! { <ThreadTurnCard turn /> }).collect_view()}
                                        </div>
                                    </div>
                                }.into_any(),
                                None => {
                                    if loading_detail.get() {
                                        view! { <div class="detail-block"><p class="eyebrow">"Live thread monitor"</p><p class="item-meta">"Loading live thread detail…"</p></div> }.into_any()
                                    } else if has_inferred_thread {
                                        view! { <div class="detail-block"><p class="eyebrow">"Live thread monitor"</p><p class="item-meta">"No thread detail loaded yet."</p></div> }.into_any()
                                    } else {
                                        view! { <div class="detail-block"><p class="eyebrow">"Live thread monitor"</p><p class="item-meta">"No live Codex thread could be inferred for this runtime lane yet."</p></div> }.into_any()
                                    }
                                }
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn TrackedThreadCard(
    thread: authority::TrackedThreadSummary,
    dashboard: WorkstreamsDashboardData,
    settings: RwSignal<OperatorServerSettings>,
    refresh_epoch: RwSignal<u64>,
    action_message: RwSignal<Option<String>>,
    action_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let runtime = tracked_thread_runtime_status(&thread, &dashboard);
    let navigate = use_navigate();
    let working = RwSignal::new(false);
    let binding = RwSignal::new(false);
    let showing_detail = RwSignal::new(false);
    let loading_detail = RwSignal::new(false);
    let detail = RwSignal::new(None::<orcas_core::ipc::ThreadView>);
    let bind_thread_id = RwSignal::new(String::new());
    let bound_upstream_thread_id = thread.upstream_thread_id.clone();
    let has_bound_upstream_thread = bound_upstream_thread_id.is_some();
    let available_threads = dashboard
        .snapshot
        .threads
        .iter()
        .filter(|candidate| {
            dashboard
                .hierarchy
                .workstreams
                .iter()
                .flat_map(|workstream| workstream.work_units.iter())
                .flat_map(|work_unit| work_unit.tracked_threads.iter())
                .all(|tracked_thread| tracked_thread.upstream_thread_id.as_deref() != Some(candidate.id.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let available_threads_for_bind = available_threads.clone();
    let tracked_thread_id_for_bind = thread.id.clone();
    let latest_report = runtime.assignment_id.as_ref().and_then(|assignment_id| {
        dashboard
            .snapshot
            .collaboration
            .reports
            .iter()
            .filter(|report| report.assignment_id == *assignment_id)
            .max_by_key(|report| report.created_at)
            .cloned()
    });
    let latest_decision = latest_report.as_ref().and_then(|report| {
        dashboard
            .snapshot
            .collaboration
            .decisions
            .iter()
            .filter(|decision| decision.report_id.as_deref() == Some(report.id.as_str()))
            .max_by_key(|decision| decision.created_at)
            .cloned()
    });
    let work_unit_proposal = dashboard
        .snapshot
        .collaboration
        .work_units
        .iter()
        .find(|work_unit| work_unit.id == thread.work_unit_id.as_str())
        .and_then(|work_unit| work_unit.proposal.clone());
    let actionable_inbox_item = dashboard
        .snapshot
        .operator_inbox
        .items
        .iter()
        .filter(|item| item.work_unit_id.as_deref() == Some(thread.work_unit_id.as_str()))
        .max_by_key(|item| item.updated_at)
        .cloned();
    let supervisor_summary = lane_activity_summary(
        latest_report.as_ref(),
        work_unit_proposal.as_ref(),
        latest_decision.as_ref(),
    );

    view! {
        <div class="item-card">
            <div class="item-card-topline">
                <span class="status-pill">{runtime.headline.clone()}</span>
                <span class="muted">"tracked lane"</span>
            </div>
            <p class="item-title">{thread.title.clone()}</p>
            <p class="item-summary">{supervisor_summary}</p>
            <p class="item-meta">
                {format!(
                    "{} · binding {}{}",
                    runtime.detail,
                    humanize_snake_case(
                        serde_json::to_string(&thread.binding_state)
                            .unwrap_or_default()
                            .trim_matches('"')
                    ),
                    thread
                        .upstream_thread_id
                        .as_ref()
                        .map(|thread_id| format!(" · codex {}", thread_id))
                        .unwrap_or_default()
                )}
            </p>
            {move || match thread.workspace_status {
                Some(status) => view! {
                    <p class="item-meta">
                        {format!(
                            "workspace {}",
                            humanize_snake_case(
                                serde_json::to_string(&status).unwrap_or_default().trim_matches('"')
                            )
                        )}
                    </p>
                }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match runtime.assignment_id.clone() {
                Some(assignment_id) => view! {
                    <p class="item-meta">{format!("assignment {}", assignment_id)}</p>
                }.into_any(),
                None => view! {}.into_any(),
            }}
            <div class="action-buttons">
                {move || match thread.upstream_thread_id.clone() {
                    Some(codex_thread_id) => view! {
                        <a class="refresh-button" href="/threads">"Open in Threads"</a>
                        <span class="muted">{codex_thread_id}</span>
                    }.into_any(),
                    None => view! {}.into_any(),
                }}
                {match actionable_inbox_item.clone() {
                    Some(item) => view! {
                        <a class="refresh-button" href={format!("/inbox/{}", item.id)}>
                            "View supervisor item"
                        </a>
                    }.into_any(),
                    None => view! {}.into_any(),
                }}
                {match (
                    latest_report.clone(),
                    work_unit_proposal.clone(),
                    actionable_inbox_item.clone(),
                ) {
                    (Some(report), None, None) if report.needs_supervisor_review => {
                        let work_unit_id = thread.work_unit_id.to_string();
                        let report_id = report.id.clone();
                        view! {
                            <button
                                class="refresh-button"
                                disabled=move || working.get()
                                on:click=move |_| {
                                    let settings = settings.get_untracked();
                                    let work_unit_id = work_unit_id.clone();
                                    let report_id = report_id.clone();
                                    working.set(true);
                                    action_error.set(None);
                                    #[cfg(target_arch = "wasm32")]
                                    spawn_local(async move {
                                        match api::proposal_create(
                                            settings,
                                            work_unit_id.clone(),
                                            Some(report_id.clone()),
                                            Some("Created from workstreams dashboard".to_string()),
                                        )
                                        .await
                                        {
                                            Ok(response) => {
                                                action_message.set(Some(format!(
                                                    "Created supervisor proposal {}.",
                                                    response.proposal.id
                                                )));
                                                refresh_epoch.update(|value| *value += 1);
                                            }
                                            Err(error) => action_error.set(Some(error)),
                                        }
                                        working.set(false);
                                    });
                                }
                            >
                                "Create proposal"
                            </button>
                        }.into_any()
                    }
                    _ => view! {}.into_any(),
                }}
                <button class="refresh-button" on:click=move |_| showing_detail.update(|value| *value = !*value)>
                    {move || if showing_detail.get() { "Hide inspect" } else { "Inspect lane" }}
                </button>
                {match actionable_inbox_item.clone() {
                    Some(item) => {
                        let available_actions = item.available_actions.clone();
                        let item_id = item.id.clone();
                        let item_updated_at = item.updated_at;
                        let item_source_kind = item.source_kind;
                        let actionable_object_id = item.actionable_object_id.clone();
                        available_actions
                            .into_iter()
                            .filter(|action_kind| {
                                matches!(
                                    action_kind,
                                    OperatorInboxActionKind::Approve | OperatorInboxActionKind::Reject
                                )
                            })
                            .map(|action_kind| {
                                let settings = settings;
                                let navigate = navigate.clone();
                                let item_id_for_action = item_id.clone();
                                let actionable_object_id = actionable_object_id.clone();
                                view! {
                                    <button
                                        class="refresh-button"
                                        disabled=move || working.get()
                                        on:click=move |_| {
                                            let settings = settings.get_untracked();
                                            let item_id = item_id_for_action.clone();
                                            let navigate = navigate.clone();
                                            let actionable_object_id = actionable_object_id.clone();
                                            working.set(true);
                                            action_error.set(None);
                                            #[cfg(target_arch = "wasm32")]
                                            spawn_local(async move {
                                                let result = if item_source_kind
                                                    == orcas_core::OperatorInboxSourceKind::SupervisorProposal
                                                {
                                                    match action_kind {
                                                        OperatorInboxActionKind::Approve => api::proposal_approve(
                                                            settings,
                                                            actionable_object_id.clone(),
                                                            Some("Approved from workstreams dashboard".to_string()),
                                                        )
                                                        .await
                                                        .map(|_| None),
                                                        OperatorInboxActionKind::Reject => api::proposal_reject(
                                                            settings,
                                                            actionable_object_id.clone(),
                                                            Some("Rejected from workstreams dashboard".to_string()),
                                                        )
                                                        .await
                                                        .map(|_| None),
                                                        _ => Ok(None),
                                                    }
                                                } else {
                                                    let idempotency_key = storage::remote_action_idempotency_key(
                                                        &settings.origin_node_id,
                                                        &item_id,
                                                        action_kind,
                                                        item_updated_at,
                                                    );
                                                    api::submit_remote_action(
                                                        settings,
                                                        item_id,
                                                        action_kind,
                                                        Some("web-operator".to_string()),
                                                        None,
                                                        Some(idempotency_key),
                                                    )
                                                    .await
                                                    .map(Some)
                                                };
                                                match result {
                                                    Ok(request) => {
                                                        action_message.set(Some(format!(
                                                            "{} submitted.",
                                                            action_kind_label(action_kind)
                                                        )));
                                                        refresh_epoch.update(|value| *value += 1);
                                                        if let Some(request) = request {
                                                            navigate(&format!("/actions/{}", request.request_id), Default::default());
                                                        }
                                                    }
                                                    Err(error) => action_error.set(Some(error)),
                                                }
                                                working.set(false);
                                            });
                                        }
                                    >
                                        {action_kind_label(action_kind)}
                                    </button>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                    None => view! {}.into_any(),
                }}
                {move || {
                    if matches!(
                        thread.binding_state,
                        authority::TrackedThreadBindingState::Unbound
                            | authority::TrackedThreadBindingState::Detached
                            | authority::TrackedThreadBindingState::Missing
                    ) {
                        view! {
                            <button class="refresh-button" on:click=move |_| binding.update(|value| *value = !*value)>
                                {move || if binding.get() { "Close bind" } else { "Bind existing" }}
                            </button>
                        }.into_any()
                    } else {
                        view! {}.into_any()
                    }
                }}
                <button
                    class="refresh-button"
                    disabled=move || working.get()
                    on:click=move |_| {
                        let settings = settings.get_untracked();
                        let tracked_thread_id = thread.id.clone();
                        working.set(true);
                        action_error.set(None);
                        #[cfg(target_arch = "wasm32")]
                        spawn_local(async move {
                            match api::delete_tracked_thread(settings, tracked_thread_id).await {
                                Ok(()) => {
                                    action_message.set(Some(
                                        "Removed Codex thread from work unit.".to_string(),
                                    ));
                                    refresh_epoch.update(|value| *value += 1);
                                }
                                Err(error) => action_error.set(Some(error)),
                            }
                            working.set(false);
                        });
                    }
                >
                    "Remove"
                </button>
            </div>
            {move || {
                if showing_detail.get() {
                    if detail.get().is_none() && !loading_detail.get() {
                        if let Some(upstream_thread_id) = bound_upstream_thread_id.clone() {
                            let settings = settings.get_untracked();
                            loading_detail.set(true);
                            action_error.set(None);
                            #[cfg(target_arch = "wasm32")]
                            spawn_local(async move {
                                match api::load_thread_detail(settings, upstream_thread_id).await {
                                    Ok(response) => detail.set(Some(response.thread)),
                                    Err(error) => action_error.set(Some(error)),
                                }
                                loading_detail.set(false);
                            });
                        }
                    }
                    view! {
                        <div class="detail-panel">
                            {match latest_report.clone() {
                                Some(report) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Supervisor state"</p>
                                        <dl class="detail-grid">
                                            <div>
                                                <dt>"Disposition"</dt>
                                                <dd>{humanize_snake_case(
                                                    serde_json::to_string(&report.disposition)
                                                        .unwrap_or_default()
                                                        .trim_matches('"')
                                                )}</dd>
                                            </div>
                                            <div>
                                                <dt>"Parse result"</dt>
                                                <dd>{humanize_snake_case(
                                                    serde_json::to_string(&report.parse_result)
                                                        .unwrap_or_default()
                                                        .trim_matches('"')
                                                )}</dd>
                                            </div>
                                            <div>
                                                <dt>"Supervisor review"</dt>
                                                <dd>{if report.needs_supervisor_review { "Required" } else { "Not required" }}</dd>
                                            </div>
                                            <div>
                                                <dt>"Confidence"</dt>
                                                <dd>{humanize_snake_case(
                                                    serde_json::to_string(&report.confidence)
                                                        .unwrap_or_default()
                                                        .trim_matches('"')
                                                )}</dd>
                                            </div>
                                            <div>
                                                <dt>"Recorded"</dt>
                                                <dd>{format_timestamp(report.created_at)}</dd>
                                            </div>
                                        </dl>
                                        <p class="item-summary">{report.summary.clone()}</p>
                                    </div>
                                }.into_any(),
                                None => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Latest report"</p>
                                        <p class="item-meta">"No report recorded yet"</p>
                                    </div>
                                }.into_any(),
                            }}
                            {match work_unit_proposal.clone() {
                                Some(proposal) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Proposal"</p>
                                        <dl class="detail-grid">
                                            <div>
                                                <dt>"Status"</dt>
                                                <dd>{humanize_snake_case(
                                                    serde_json::to_string(&proposal.latest_status)
                                                        .unwrap_or_default()
                                                        .trim_matches('"')
                                                )}</dd>
                                            </div>
                                            <div>
                                                <dt>"Decision type"</dt>
                                                <dd>{proposal
                                                    .latest_proposed_decision_type
                                                    .as_ref()
                                                    .map(|decision| humanize_snake_case(
                                                        serde_json::to_string(decision)
                                                            .unwrap_or_default()
                                                            .trim_matches('"')
                                                    ))
                                                    .unwrap_or_else(|| "Unknown".to_string())}</dd>
                                            </div>
                                            <div>
                                                <dt>"Proposal id"</dt>
                                                <dd>{proposal.latest_proposal_id.clone()}</dd>
                                            </div>
                                            <div>
                                                <dt>"Open"</dt>
                                                <dd>{if proposal.has_open_proposal { "Yes" } else { "No" }}</dd>
                                            </div>
                                            <div>
                                                <dt>"Created"</dt>
                                                <dd>{format_timestamp(proposal.latest_created_at)}</dd>
                                            </div>
                                            <div>
                                                <dt>"Reviewed"</dt>
                                                <dd>{format_optional_timestamp(proposal.latest_reviewed_at)}</dd>
                                            </div>
                                            {proposal.latest_failure_stage.as_ref().map(|stage| view! {
                                                <div>
                                                    <dt>"Failure stage"</dt>
                                                    <dd>{humanize_snake_case(
                                                        serde_json::to_string(stage)
                                                            .unwrap_or_default()
                                                            .trim_matches('"')
                                                    )}</dd>
                                                </div>
                                            })}
                                        </dl>
                                    </div>
                                }.into_any(),
                                None => match actionable_inbox_item.clone() {
                                    Some(item) => view! {
                                        <div class="detail-block">
                                            <p class="eyebrow">"Supervisor item"</p>
                                            <p class="item-summary">{item.summary.clone()}</p>
                                            <p class="item-meta">
                                                {format!(
                                                    "{} · {}",
                                                    humanize_snake_case(
                                                        serde_json::to_string(&item.source_kind)
                                                            .unwrap_or_default()
                                                            .trim_matches('"')
                                                    ),
                                                    humanize_snake_case(
                                                        serde_json::to_string(&item.status)
                                                            .unwrap_or_default()
                                                            .trim_matches('"')
                                                    )
                                                )}
                                            </p>
                                            <p class="item-meta">
                                                {format!(
                                                    "created {} · updated {}",
                                                    format_timestamp(item.created_at),
                                                    format_timestamp(item.updated_at)
                                                )}
                                            </p>
                                        </div>
                                    }.into_any(),
                                    None => view! {
                                        <div class="detail-block">
                                            <p class="eyebrow">"Supervisor item"</p>
                                            <p class="item-meta">
                                                "No mirrored proposal or decision item is available yet for this work unit."
                                            </p>
                                        </div>
                                    }.into_any(),
                                },
                            }}
                            {match latest_decision.clone() {
                                Some(decision) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Latest decision"</p>
                                        <dl class="detail-grid">
                                            <div>
                                                <dt>"Decision id"</dt>
                                                <dd>{decision.id.clone()}</dd>
                                            </div>
                                            <div>
                                                <dt>"Type"</dt>
                                                <dd>{humanize_snake_case(
                                                    serde_json::to_string(&decision.decision_type)
                                                        .unwrap_or_default()
                                                        .trim_matches('"')
                                                )}</dd>
                                            </div>
                                            <div>
                                                <dt>"Recorded"</dt>
                                                <dd>{format_timestamp(decision.created_at)}</dd>
                                            </div>
                                        </dl>
                                        <p class="item-summary">{decision.rationale.clone()}</p>
                                    </div>
                                }.into_any(),
                                None => view! {}.into_any(),
                            }}
                            {move || match detail.get() {
                                Some(detail) => view! {
                                    <div class="detail-block">
                                        <p class="eyebrow">"Live thread monitor"</p>
                                        <dl class="detail-grid">
                                            <div>
                                                <dt>"Thread"</dt>
                                                <dd>{detail.summary.id.clone()}</dd>
                                            </div>
                                            <div>
                                                <dt>"Status"</dt>
                                                <dd>{detail.summary.status.clone()}</dd>
                                            </div>
                                            <div>
                                                <dt>"Monitor"</dt>
                                                <dd>{humanize_snake_case(
                                                    serde_json::to_string(&detail.summary.monitor_state)
                                                        .unwrap_or_default()
                                                        .trim_matches('"')
                                                )}</dd>
                                            </div>
                                            <div>
                                                <dt>"Turns"</dt>
                                                <dd>{detail.turns.len().to_string()}</dd>
                                            </div>
                                            <div>
                                                <dt>"Updated"</dt>
                                                <dd>{format_unix_millis(detail.summary.updated_at)}</dd>
                                            </div>
                                        </dl>
                                        <p class="item-summary">{detail.summary.preview.clone()}</p>
                                        {detail.summary.raw_summary.clone().map(|raw_summary| view! {
                                            <div class="json-panel">
                                                <p class="eyebrow">"Thread summary"</p>
                                                <details>
                                                    <summary>"Show raw thread summary"</summary>
                                                    <JsonValueTree value=raw_summary />
                                                </details>
                                            </div>
                                        })}
                                        <div class="detail-panel">
                                            {detail
                                                .turns
                                                .into_iter()
                                                .rev()
                                                .take(2)
                                                .map(|turn| view! { <ThreadTurnCard turn /> })
                                                .collect_view()}
                                        </div>
                                    </div>
                                }.into_any(),
                                None => {
                                    if loading_detail.get() {
                                        view! {
                                            <div class="detail-block">
                                                <p class="eyebrow">"Live thread monitor"</p>
                                                <p class="item-meta">"Loading live thread detail…"</p>
                                            </div>
                                        }.into_any()
                                    } else if has_bound_upstream_thread {
                                        view! {
                                            <div class="detail-block">
                                                <p class="eyebrow">"Live thread monitor"</p>
                                                <p class="item-meta">"No thread detail loaded yet."</p>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="detail-block">
                                                <p class="eyebrow">"Live thread monitor"</p>
                                                <p class="item-meta">"No live Codex thread is bound to this tracked thread yet."</p>
                                            </div>
                                        }.into_any()
                                    }
                                }
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
            {move || {
                if binding.get() {
                    let available_threads_for_bind = available_threads_for_bind.clone();
                    let tracked_thread_id_for_bind = tracked_thread_id_for_bind.clone();
                    view! {
                        <div class="action-form">
                            <label class="field">
                                <span>"Live Codex thread"</span>
                                <select
                                    prop:value=move || bind_thread_id.get()
                                    on:change=move |ev| bind_thread_id.set(event_target_value(&ev))
                                >
                                    <option value="">"Select live thread"</option>
                                    {available_threads.iter().map(|candidate| view! {
                                        <option value=candidate.id.clone()>
                                            {format!(
                                                "{} · {} · {}",
                                                candidate.id,
                                                if candidate.cwd.is_empty() { "(no cwd)" } else { candidate.cwd.as_str() },
                                                candidate.status
                                            )}
                                        </option>
                                    }).collect_view()}
                                </select>
                            </label>
                            <div class="action-buttons">
                                <button
                                    class="primary-button"
                                    disabled=move || working.get()
                                    on:click=move |_| {
                                        let settings = settings.get_untracked();
                                        let selected_thread_id = bind_thread_id.get_untracked();
                                        let preferred_cwd = available_threads_for_bind
                                            .iter()
                                            .find(|candidate| candidate.id == selected_thread_id)
                                            .and_then(|candidate| (!candidate.cwd.is_empty()).then(|| candidate.cwd.clone()));
                                        let tracked_thread_id = tracked_thread_id_for_bind.clone();
                                        let expected_revision = thread.revision;
                                        working.set(true);
                                        action_error.set(None);
                                        #[cfg(target_arch = "wasm32")]
                                        spawn_local(async move {
                                            match api::bind_tracked_thread(
                                                settings,
                                                tracked_thread_id,
                                                expected_revision,
                                                selected_thread_id,
                                                preferred_cwd,
                                            ).await {
                                                Ok(()) => {
                                                    action_message.set(Some("Bound tracked thread to live Codex thread.".to_string()));
                                                    refresh_epoch.update(|value| *value += 1);
                                                    binding.set(false);
                                                }
                                                Err(error) => action_error.set(Some(error)),
                                            }
                                            working.set(false);
                                        });
                                    }
                                >
                                    "Bind live thread"
                                </button>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn InboxRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let refresh_epoch = RwSignal::new(0u64);
    let watch_error = RwSignal::new(None::<String>);
    let previous_page = RwSignal::new(None::<InboxPageView>);
    let change_summary = RwSignal::new(None::<ViewChangeSummary>);
    let settings_value = move || settings.get_untracked();
    let inbox = LocalResource::new(move || {
        let settings = settings_value();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_inbox_page(settings).await }
    });

    Effect::new({
        let _settings = settings.clone();
        let _refresh_epoch = refresh_epoch.clone();
        let _watch_error = watch_error.clone();
        let workspace = workspace.clone();
        move |_| {
            workspace.update(|state| state.active_section = WorkspaceSection::Inbox);
            let alive = Arc::new(AtomicBool::new(true));
            on_cleanup({
                let alive = alive.clone();
                move || alive.store(false, Ordering::Release)
            });
            #[cfg(target_arch = "wasm32")]
            {
                let settings = _settings.clone();
                let refresh_epoch = _refresh_epoch.clone();
                let watch_error = _watch_error.clone();
                let alive = alive.clone();
                spawn_local(async move {
                    let current_settings = settings.get_untracked();
                    if !storage::settings_ready(&current_settings) {
                        return;
                    }
                    let mut after_sequence =
                        match api::inbox_checkpoint(current_settings.clone()).await {
                            Ok(response) => response.checkpoint.current_sequence,
                            Err(error) => {
                                watch_error.set(Some(error));
                                return;
                            }
                        };
                    loop {
                        if !alive.load(Ordering::Acquire) {
                            break;
                        }
                        let current_settings = settings.get_untracked();
                        if !storage::settings_ready(&current_settings) {
                            break;
                        }
                        match api::wait_for_inbox_checkpoint(
                            current_settings,
                            Some(after_sequence),
                            Some(30_000),
                        )
                        .await
                        {
                            Ok(response) => {
                                if !alive.load(Ordering::Acquire) {
                                    break;
                                }
                                if let Some(next_sequence) =
                                    api::inbox_checkpoint_advance(after_sequence, &response)
                                {
                                    after_sequence = next_sequence;
                                    watch_error.set(None);
                                    refresh_epoch.update(|value| *value += 1);
                                }
                            }
                            Err(error) => {
                                watch_error.set(Some(error));
                                break;
                            }
                        }
                    }
                });
            }
        }
    });

    Effect::new({
        let inbox = inbox.clone();
        let previous_page = previous_page.clone();
        let change_summary = change_summary.clone();
        move |_| match inbox.get() {
            Some(Ok(page)) => {
                let change =
                    summarize_inbox_page_change(previous_page.get_untracked().as_ref(), &page);
                previous_page.set(Some(page));
                change_summary.set(change);
            }
            Some(Err(_)) | None => change_summary.set(None),
        }
    });

    view! {
        <PageFrame title="Actionable inbox" subtitle="Derived mirrored work that needs operator attention">
            <div class="toolbar">
                <button class="refresh-button" on:click=move |_| inbox.refetch()>"Refresh"</button>
                <span class="muted">"Auto-refreshes on server checkpoint changes while this view is open."</span>
            </div>
            {move || render_change_banner(change_summary.get())}
            {move || match watch_error.get() {
                Some(error) => view! { <ErrorPanel error=format!("Live refresh paused: {error}") /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match inbox.get() {
                None => view! { <p class="muted">"Loading inbox…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => {
                    let workspace = workspace.get();
                    render_inbox_page(page, &workspace)
                }
            }}
        </PageFrame>
    }
}

#[component]
fn InboxDetailRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let params = use_params_map();
    let item_id = move || params.with(|params| params.get("item_id").unwrap_or_default());
    let item_id_value = item_id();
    Effect::new({
        let workspace = workspace.clone();
        let item_id_value = item_id_value.clone();
        move |_| {
            workspace.update(|state| {
                state.active_section = WorkspaceSection::Inbox;
                state.focus = Some(WorkspaceFocus::inbox_item_placeholder(
                    item_id_value.clone(),
                ));
            });
        }
    });
    let detail = LocalResource::new(move || {
        let settings = settings.get();
        let item_id = item_id();
        async move { api::load_inbox_item_detail(settings, item_id).await }
    });
    Effect::new({
        let workspace = workspace.clone();
        let detail = detail.clone();
        let item_id_value = item_id_value.clone();
        move |_| match detail.get() {
            Some(Ok(page)) => {
                workspace.update(|state| {
                    state.focus = page
                        .item
                        .as_ref()
                        .map(WorkspaceFocus::from_inbox_item)
                        .or_else(|| {
                            Some(WorkspaceFocus::inbox_item_placeholder(
                                item_id_value.clone(),
                            ))
                        });
                });
            }
            Some(Err(_)) => {}
            None => {}
        }
    });
    let navigator = use_navigate();
    let navigate = move |path: &str| navigator(path, Default::default());

    view! {
        <PageFrame title="Inbox item" subtitle="Mirrored read-model detail and available actions">
            {move || match detail.get() {
                None => view! { <p class="muted">"Loading item…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => render_inbox_detail_page(page, navigate.clone(), workspace.get()),
            }}
        </PageFrame>
    }
}

#[component]
fn NotificationsRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let refresh_epoch = RwSignal::new(0u64);
    let watch_error = RwSignal::new(None::<String>);
    let watch_started = RwSignal::new(false);
    let previous_page = RwSignal::new(None::<NotificationPageView>);
    let change_summary = RwSignal::new(None::<ViewChangeSummary>);
    let notifications = LocalResource::new(move || {
        let settings = settings.get();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_notifications_page(settings).await }
    });

    Effect::new({
        let settings = settings.clone();
        let refresh_epoch = refresh_epoch.clone();
        let watch_error = watch_error.clone();
        let watch_started = watch_started.clone();
        let workspace = workspace.clone();
        move |_| {
            workspace.update(|state| state.active_section = WorkspaceSection::Notifications);
            if watch_started.get_untracked() {
                return;
            }
            let current_settings = settings.get_untracked();
            if !storage::settings_ready(&current_settings) {
                return;
            }
            #[cfg(target_arch = "wasm32")]
            {
                watch_started.set(true);
                let alive = Arc::new(AtomicBool::new(true));
                on_cleanup({
                    let alive = alive.clone();
                    move || alive.store(false, Ordering::Release)
                });
                let refresh_epoch = refresh_epoch.clone();
                let watch_error = watch_error.clone();
                spawn_local(async move {
                    let initial_checkpoint =
                        match api::load_notification_checkpoint(current_settings.clone()).await {
                            Ok(checkpoint) => checkpoint,
                            Err(error) => {
                                watch_error.set(Some(error));
                                watch_started.set(false);
                                return;
                            }
                        };
                    let result = watch::run_change_watch_loop(
                        alive,
                        initial_checkpoint,
                        move |after_updated_at, timeout_ms| {
                            let current_settings = current_settings.clone();
                            async move {
                                api::wait_for_notification_checkpoint(
                                    current_settings,
                                    after_updated_at,
                                    timeout_ms,
                                )
                                .await
                                .map(|next| next.map(|checkpoint| (Some(checkpoint), ())))
                            }
                        },
                        move |_| {
                            watch_error.set(None);
                            refresh_epoch.update(|value| *value += 1);
                            true
                        },
                    )
                    .await;
                    if let Err(error) = result {
                        watch_error.set(Some(error));
                    }
                    watch_started.set(false);
                });
            }
        }
    });

    Effect::new({
        let notifications = notifications.clone();
        let previous_page = previous_page.clone();
        let change_summary = change_summary.clone();
        move |_| match notifications.get() {
            Some(Ok(page)) => {
                let change = summarize_notification_page_change(
                    previous_page.get_untracked().as_ref(),
                    &page,
                );
                previous_page.set(Some(page));
                change_summary.set(change);
            }
            Some(Err(_)) | None => change_summary.set(None),
        }
    });

    view! {
        <PageFrame title="Notifications" subtitle="Server-side notification readiness">
            <div class="toolbar">
                <button class="refresh-button" on:click=move |_| notifications.refetch()>"Refresh"</button>
                <span class="muted">"Auto-refreshes on server notification checkpoint changes while this view is open."</span>
            </div>
            {move || render_change_banner(change_summary.get())}
            {move || match watch_error.get() {
                Some(error) => view! { <ErrorPanel error=format!("Live refresh paused: {error}") /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match notifications.get() {
                None => view! { <p class="muted">"Loading notifications…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => {
                    let workspace = workspace.get();
                    render_notification_page(page, workspace.clone())
                }
            }}
        </PageFrame>
    }
}

#[component]
fn DeliveriesRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let refresh_epoch = RwSignal::new(0u64);
    let watch_error = RwSignal::new(None::<String>);
    let watch_started = RwSignal::new(false);
    let previous_page = RwSignal::new(None::<DeliveryPageView>);
    let change_summary = RwSignal::new(None::<ViewChangeSummary>);
    let deliveries = LocalResource::new(move || {
        let settings = settings.get();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_deliveries_page(settings).await }
    });

    Effect::new({
        let settings = settings.clone();
        let refresh_epoch = refresh_epoch.clone();
        let watch_error = watch_error.clone();
        let watch_started = watch_started.clone();
        let workspace = workspace.clone();
        move |_| {
            workspace.update(|state| state.active_section = WorkspaceSection::Deliveries);
            if watch_started.get_untracked() {
                return;
            }
            let current_settings = settings.get_untracked();
            if !storage::settings_ready(&current_settings) {
                return;
            }
            #[cfg(target_arch = "wasm32")]
            {
                watch_started.set(true);
                let alive = Arc::new(AtomicBool::new(true));
                on_cleanup({
                    let alive = alive.clone();
                    move || alive.store(false, Ordering::Release)
                });
                let refresh_epoch = refresh_epoch.clone();
                let watch_error = watch_error.clone();
                spawn_local(async move {
                    let initial_checkpoint =
                        match api::load_delivery_checkpoint(current_settings.clone()).await {
                            Ok(checkpoint) => checkpoint,
                            Err(error) => {
                                watch_error.set(Some(error));
                                watch_started.set(false);
                                return;
                            }
                        };
                    let result = watch::run_change_watch_loop(
                        alive,
                        initial_checkpoint,
                        move |after_updated_at, timeout_ms| {
                            let current_settings = current_settings.clone();
                            async move {
                                api::wait_for_delivery_checkpoint(
                                    current_settings,
                                    after_updated_at,
                                    timeout_ms,
                                )
                                .await
                                .map(|next| next.map(|checkpoint| (Some(checkpoint), ())))
                            }
                        },
                        move |_| {
                            watch_error.set(None);
                            refresh_epoch.update(|value| *value += 1);
                            true
                        },
                    )
                    .await;
                    if let Err(error) = result {
                        watch_error.set(Some(error));
                    }
                    watch_started.set(false);
                });
            }
        }
    });

    Effect::new({
        let deliveries = deliveries.clone();
        let previous_page = previous_page.clone();
        let change_summary = change_summary.clone();
        move |_| match deliveries.get() {
            Some(Ok(page)) => {
                let change =
                    summarize_delivery_page_change(previous_page.get_untracked().as_ref(), &page);
                previous_page.set(Some(page));
                change_summary.set(change);
            }
            Some(Err(_)) | None => change_summary.set(None),
        }
    });

    view! {
        <PageFrame title="Deliveries" subtitle="Notification delivery jobs and outcomes">
            <div class="toolbar">
                <button class="refresh-button" on:click=move |_| deliveries.refetch()>"Refresh"</button>
                <span class="muted">"Auto-refreshes on server delivery checkpoint changes while this view is open."</span>
            </div>
            {move || render_change_banner(change_summary.get())}
            {move || match watch_error.get() {
                Some(error) => view! { <ErrorPanel error=format!("Live refresh paused: {error}") /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match deliveries.get() {
                None => view! { <p class="muted">"Loading deliveries…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => {
                    let workspace = workspace.get();
                    render_delivery_page(page, workspace.clone())
                }
            }}
        </PageFrame>
    }
}

#[component]
fn ActionListRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    Effect::new({
        let workspace = workspace.clone();
        move |_| workspace.update(|state| state.active_section = WorkspaceSection::Actions)
    });
    let actions = LocalResource::new(move || {
        let settings = settings.get();
        async move { api::load_action_requests_page(settings).await }
    });

    view! {
        <PageFrame title="Actions" subtitle="Recent remote action requests">
            <button class="refresh-button" on:click=move |_| actions.refetch()>"Refresh"</button>
            {move || match actions.get() {
                None => view! { <p class="muted">"Loading action requests…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => {
                    let workspace = workspace.get();
                    render_action_list_page(page, workspace.clone())
                }
            }}
        </PageFrame>
    }
}

#[component]
fn ActionRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let workspace =
        use_context::<RwSignal<WorkspaceState>>().expect("workspace context should be provided");
    let params = use_params_map();
    let request_id = move || params.with(|params| params.get("request_id").unwrap_or_default());
    let push_context = push::current_push_open_context();
    let request_id_value = request_id();
    Effect::new({
        let workspace = workspace.clone();
        let request_id_value = request_id_value.clone();
        move |_| {
            workspace.update(|state| {
                state.active_section = WorkspaceSection::Actions;
                state.focus = Some(WorkspaceFocus::remote_action_request_placeholder(
                    request_id_value.clone(),
                ));
            });
        }
    });
    let refresh_epoch = RwSignal::new(0u64);
    let watching = RwSignal::new(false);
    let watch_started = RwSignal::new(false);
    let watch_error = RwSignal::new(None::<String>);
    let previous_request = RwSignal::new(None::<RemoteActionRequestView>);
    let change_summary = RwSignal::new(None::<ViewChangeSummary>);
    let action_request = LocalResource::new(move || {
        let settings = settings.get();
        let request_id = request_id();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_action_request(settings, request_id).await }
    });

    Effect::new({
        let action_request = action_request.clone();
        let previous_request = previous_request.clone();
        let change_summary = change_summary.clone();
        move |_| match action_request.get() {
            Some(Ok(Some(request))) => {
                let change = summarize_remote_action_request_change(
                    previous_request.get_untracked().as_ref(),
                    &request,
                );
                previous_request.set(Some(request));
                change_summary.set(change);
            }
            Some(Ok(None)) | Some(Err(_)) | None => change_summary.set(None),
        }
    });
    Effect::new({
        let workspace = workspace.clone();
        let action_request = action_request.clone();
        move |_| match action_request.get() {
            Some(Ok(Some(request))) => {
                workspace.update(|state| {
                    state.focus = Some(WorkspaceFocus::from_remote_action_request(&request));
                });
            }
            Some(Ok(None)) => {}
            Some(Err(_)) | None => {}
        }
    });

    Effect::new(move |_| {
        let should_watch = watching.get();
        let settings_value = settings.get_untracked();
        let request_id_value = request_id();
        let current = action_request.get();
        if !should_watch || watch_started.get_untracked() {
            return;
        }
        let Some(Ok(Some(request))) = current else {
            return;
        };
        if !matches!(
            request.status,
            OperatorRemoteActionRequestStatus::Pending | OperatorRemoteActionRequestStatus::Claimed
        ) {
            watching.set(false);
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            watch_started.set(true);
            let alive = Arc::new(AtomicBool::new(true));
            on_cleanup({
                let alive = alive.clone();
                move || alive.store(false, Ordering::Release)
            });
            let refresh_epoch = refresh_epoch.clone();
            let watch_error = watch_error.clone();
            let watching = watching.clone();
            let watch_started = watch_started.clone();
            spawn_local(async move {
                let result = watch::run_change_watch_loop(
                    alive,
                    request.updated_at,
                    move |after_updated_at, timeout_ms| {
                        let settings_value = settings_value.clone();
                        let request_id_value = request_id_value.clone();
                        async move {
                            api::wait_for_remote_action_update(
                                settings_value,
                                request_id_value,
                                Some(after_updated_at),
                                timeout_ms,
                            )
                            .await
                            .map(|response| response.map(|updated| (updated.updated_at, updated)))
                        }
                    },
                    move |updated| {
                        refresh_epoch.update(|value| *value += 1);
                        watch_error.set(None);
                        let keep_watching = matches!(
                            updated.status,
                            OperatorRemoteActionRequestStatus::Pending
                                | OperatorRemoteActionRequestStatus::Claimed
                        );
                        if !keep_watching {
                            watching.set(false);
                        }
                        keep_watching
                    },
                )
                .await;
                if let Err(error) = result {
                    watch_error.set(Some(error));
                    watching.set(false);
                }
                watch_started.set(false);
            });
        }
    });

    view! {
        <PageFrame title="Action request" subtitle="Remote operator intent routed back through the daemon">
            <button class="refresh-button" on:click=move |_| action_request.refetch()>"Refresh"</button>
            <button class="primary-button" on:click=move |_| watching.set(true)>"Watch status"</button>
            {move || render_change_banner(change_summary.get())}
            {move || match watch_error.get() {
                Some(error) => view! { <ErrorPanel error=error /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match action_request.get() {
                None => view! { <p class="muted">"Loading request…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(None)) => view! {
                    <div class="stack">
                        {render_push_banner(
                            push_context.clone(),
                            Some("remote action request".to_string()),
                            Some(missing_remote_action_notice(push_context.is_some()).to_string()),
                        )}
                        <EmptyState title="Request not found" body={missing_remote_action_notice(push_context.is_some())} />
                    </div>
                }.into_any(),
                Some(Ok(Some(request))) => {
                    let workspace = workspace.get();
                    render_remote_action_page(request, move || watching.get(), workspace.clone())
                }
            }}
        </PageFrame>
    }
}

#[component]
fn PageFrame(title: &'static str, subtitle: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="page">
            <header class="page-header">
                <div>
                    <p class="eyebrow">{subtitle}</p>
                    <h2>{title}</h2>
                </div>
            </header>
            <div class="page-content">
                {children()}
            </div>
        </section>
    }
}

#[component]
fn ErrorPanel(error: String) -> impl IntoView {
    view! {
        <div class="error-panel">
            <strong>"Request failed"</strong>
            <p>{error}</p>
        </div>
    }
}

#[component]
fn EmptyState(title: &'static str, body: &'static str) -> impl IntoView {
    view! {
        <div class="empty-state">
            <h3>{title}</h3>
            <p>{body}</p>
        </div>
    }
}

fn render_push_banner(
    context: Option<push::PushOpenContext>,
    current_subject: Option<String>,
    state_note: Option<String>,
) -> AnyView {
    match context {
        Some(context) => {
            let presentation = push::push_open_context_presentation(&context);
            view! {
            <div class="info-panel">
                <strong>"Opened from browser notification"</strong>
                <p>{push::push_context_summary(&context)}</p>
                <p>{format!("{} · {}", presentation.route_label, presentation.subject_label)}</p>
                <p>{presentation.reason}</p>
                {move || match current_subject.clone() {
                    Some(subject) => view! { <p>{format!("Current mirrored object: {subject}")}</p> }.into_any(),
                    None => view! {}.into_any(),
                }}
                {move || match state_note.clone() {
                    Some(note) => view! { <p>{note}</p> }.into_any(),
                    None => view! {}.into_any(),
                }}
                <p>{presentation.next_step_hint}</p>
            </div>
            }
            .into_any()
        }
        None => view! {}.into_any(),
    }
}

fn render_change_banner(summary: Option<ViewChangeSummary>) -> AnyView {
    match summary {
        Some(summary) => view! {
            <div class="info-panel">
                <strong>"What changed"</strong>
                <p>{summary.headline}</p>
                <p>{summary.detail}</p>
            </div>
        }
        .into_any(),
        None => view! {}.into_any(),
    }
}

fn missing_inbox_item_notice(push_context_present: bool) -> &'static str {
    if push_context_present {
        "The mirrored inbox item for this notification is missing or no longer actionable on the server."
    } else {
        "The mirrored inbox item is missing from the server."
    }
}

fn missing_remote_action_notice(push_context_present: bool) -> &'static str {
    if push_context_present {
        "The remote action request for this notification is missing or no longer visible on the server."
    } else {
        "The remote action request is missing from the server."
    }
}

fn render_inbox_page(page: InboxPageView, workspace: &WorkspaceState) -> AnyView {
    if page.empty_state {
        return view! {
            <EmptyState title="No mirrored inbox items" body="The server has not mirrored any actionable work yet." />
        }
        .into_any();
    }

    view! {
        <div class="stack">
            <p class="muted">
                {format!(
                    "{} actionable / {} total mirrored items from origin `{}`",
                    page.actionable_count, page.total_count, page.origin_node_id
                )}
            </p>
            <div class="section-grid">
                {page
                    .sections
                    .into_iter()
                    .map(|section| render_inbox_section(section, workspace))
                    .collect_view()}
            </div>
        </div>
    }
    .into_any()
}

fn render_inbox_section(
    section: orcas_operator_core::InboxSectionView,
    workspace: &WorkspaceState,
) -> AnyView {
    view! {
        <article class="card">
            <header class="card-header">
                <div>
                    <p class="eyebrow">{source_kind_label(section.source_kind)}</p>
                    <h3>{section.title}</h3>
                </div>
            </header>
            <ul class="item-list">
                {section
                    .items
                    .into_iter()
                    .map(|item| render_inbox_card(item, workspace))
                    .collect_view()}
            </ul>
        </article>
    }
    .into_any()
}

fn render_inbox_card(item: InboxItemCardView, workspace: &WorkspaceState) -> AnyView {
    let href = format!("/inbox/{}", item.id);
    let selected = workspace.focus_matches_inbox_item(&item.id);
    view! {
        <li class=move || if selected { "item-card item-card-selected" } else { "item-card" }>
            <div class="item-card-main">
                <div class="item-card-topline">
                    <span class="status-pill">{item.status_label}</span>
                    <span class="muted">{item.source_kind_label}</span>
                </div>
                <a class="item-title" href=href>{item.title}</a>
                <p class="item-summary">{item.summary}</p>
                <p class="item-meta">{inbox_status_hint(item.status)}</p>
                <p class="item-meta">
                    {format!("actions: {}", item.available_action_labels.join(", "))}
                </p>
            </div>
        </li>
    }
    .into_any()
}

fn render_inbox_detail_page(
    page: InboxDetailPageView,
    navigate: impl Fn(&str) + Clone + 'static,
    workspace: WorkspaceState,
) -> AnyView {
    let navigate_action = navigate.clone();
    let item = page.item.clone();
    let action_buttons = item
        .as_ref()
        .map(|item| item.available_actions.clone())
        .unwrap_or_default();
    let summary = item
        .as_ref()
        .map(|item| item.summary.clone())
        .unwrap_or_else(|| "No item data".to_string());
    let item_updated_at = item.as_ref().map(|item| item.updated_at);
    let title = item
        .as_ref()
        .map(|item| item.title.clone())
        .unwrap_or_else(|| "Missing inbox item".to_string());
    let item_id = item.as_ref().map(|item| item.id.clone());
    let item_id_text = item_id
        .clone()
        .unwrap_or_else(|| "unknown item".to_string());
    let item_title = item.as_ref().map(|item| item.title.clone());
    let origin_node_id = page
        .notification_candidates
        .first()
        .map(|candidate| candidate.origin_node_id.clone())
        .or_else(|| {
            page.delivery_jobs
                .first()
                .map(|job| job.origin_node_id.clone())
        })
        .or_else(|| {
            page.remote_action_requests
                .first()
                .map(|request| request.origin_node_id.clone())
        })
        .unwrap_or_default();
    let note = RwSignal::new(String::new());
    let submitting = RwSignal::new(false);
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let push_context = push::current_push_open_context();
    let push_context_present = push_context.is_some();
    let item_state_note = item
        .as_ref()
        .map(|item| {
            format!(
                "Current mirrored status: {} · {}",
                item.status_label,
                inbox_status_hint(item.status)
            )
        })
        .or_else(|| Some(missing_inbox_item_notice(push_context_present).to_string()));
    let workspace_focus_note = match workspace.focus.as_ref() {
        Some(focus) if focus.item_id.as_deref() == item_id.as_deref() => {
            "This mirrored inbox item is pinned as the current focus.".to_string()
        }
        Some(focus) => format!(
            "Pinned focus remains on {} · {}",
            focus.kind_label, focus.status_label
        ),
        None => "No item is pinned in the workspace yet.".to_string(),
    };

    view! {
        <div class="stack">
            {render_push_banner(push_context.clone(), item_title, item_state_note)}
            <div class="info-panel">
                <strong>"Workspace context"</strong>
                <p>{format!("Active section: {}", workspace.active_section.label())}</p>
                <p>{workspace_focus_note}</p>
            </div>
            <article class="card">
                <p class="eyebrow">{item_id_text}</p>
                <h3>{title}</h3>
                <p class="item-summary">{summary}</p>
                {move || match item.as_ref() {
                    Some(item) => render_item_details(item).into_any(),
                    None => view! { <p class="muted">{missing_inbox_item_notice(push_context_present)}</p> }.into_any(),
                }}
            </article>

            <article class="card">
                <h3>"Available actions"</h3>
                <div class="action-form">
                    <label class="field">
                        <span>"Optional note"</span>
                        <textarea
                            rows="3"
                            prop:value=move || note.get()
                            on:input=move |ev| note.set(event_target_value(&ev))
                        ></textarea>
                    </label>
                    <div class="action-buttons">
                        {action_buttons.into_iter().map(|action_kind| {
                            let note = note.clone();
                            let settings = settings.clone();
                            let navigate = navigate_action.clone();
                            let item_id_value = item_id.clone();
                            let item_updated_at = item_updated_at.clone();
                            let existing_request = item_id_value.as_deref().and_then(|item_id| {
                                pending_remote_action_request_for_item_action(
                                    &page.remote_action_requests,
                                    item_id,
                                    action_kind,
                                )
                                .cloned()
                            });
                            let action_row = match existing_request {
                                Some(request) => view! {
                                    <a class="primary-button secondary-button" href={format!("/actions/{}", request.request_id)}>
                                        {format!("{} pending", action_kind_label(action_kind))}
                                    </a>
                                }
                                .into_any(),
                                None => view! {
                                    <button
                                        class="primary-button"
                                        disabled=move || submitting.get()
                                        on:click=move |_| {
                                            submitting.set(true);
                                            let _note_value = note.get();
                                            let _settings_value = settings.get();
                                            let _navigate = navigate.clone();
                                            let item_id_value = item_id_value.clone();
                                            let item_updated_at = item_updated_at.clone();
                                            #[cfg(target_arch = "wasm32")]
                                            spawn_local(async move {
                                                let Some(item_id_value) = item_id_value else {
                                                    submitting.set(false);
                                                    watch_error_or_log("missing inbox item id for action submission".to_string());
                                                    return;
                                                };
                                                let Some(item_updated_at) = item_updated_at else {
                                                    submitting.set(false);
                                                    watch_error_or_log("missing inbox item timestamp for action submission".to_string());
                                                    return;
                                                };
                                                let idempotency_key = storage::remote_action_idempotency_key(
                                                    &_settings_value.origin_node_id,
                                                    &item_id_value,
                                                    action_kind,
                                                    item_updated_at,
                                                );
                                                let result = api::submit_remote_action(
                                                    _settings_value,
                                                    item_id_value,
                                                    action_kind,
                                                    Some("web-operator".to_string()),
                                                    if _note_value.trim().is_empty() { None } else { Some(_note_value) },
                                                    Some(idempotency_key),
                                                )
                                                .await;
                                                submitting.set(false);
                                                match result {
                                                    Ok(request) => _navigate(&format!("/actions/{}", request.request_id)),
                                                    Err(error) => watch_error_or_log(error),
                                                }
                                            });
                                        }
                                    >
                                        {action_kind_label(action_kind)}
                                    </button>
                                }
                                .into_any(),
                            };
                            view! {
                                <div class="action-button-row">
                                    {action_row}
                                </div>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </article>

            <article class="card">
                <h3>"Related notification candidates"</h3>
                {render_notification_candidates(page.notification_candidates, workspace.clone())}
            </article>

            <article class="card">
                <h3>"Related delivery jobs"</h3>
                {render_delivery_jobs(page.delivery_jobs, workspace.clone())}
            </article>

            <article class="card">
                <h3>"Recent remote action requests"</h3>
                {render_remote_action_requests(page.remote_action_requests, origin_node_id, workspace.clone())}
            </article>
        </div>
    }
    .into_any()
}

fn render_item_details(item: &InboxItemCardView) -> AnyView {
    view! {
        <dl class="detail-grid">
            <div><dt>"Source kind"</dt><dd>{item.source_kind_label}</dd></div>
            <div><dt>"Status"</dt><dd>{inbox_status_label(item.status)}</dd></div>
            <div><dt>"Actionable object"</dt><dd>{item.actionable_object_id.clone()}</dd></div>
            <div><dt>"Workstream"</dt><dd>{item.workstream_id.clone().unwrap_or_else(|| "none".to_string())}</dd></div>
            <div><dt>"Work unit"</dt><dd>{item.work_unit_id.clone().unwrap_or_else(|| "none".to_string())}</dd></div>
            <div><dt>"Actions"</dt><dd>{item.available_action_labels.join(", ")}</dd></div>
        </dl>
        <p class="item-meta">{inbox_status_hint(item.status)}</p>
    }
    .into_any()
}

fn render_notification_page(page: NotificationPageView, workspace: WorkspaceState) -> AnyView {
    let push_context = push::current_push_open_context();
    if page.candidates.is_empty() {
        return view! {
            <div class="stack">
                {render_push_banner(
                    push_context,
                    Some("notification readiness".to_string()),
                    Some(format!(
                        "No mirrored notification candidates are currently ready for origin `{}`.",
                        page.origin_node_id
                    )),
                )}
                <EmptyState title="No notification candidates" body="No mirrored inbox item is currently ready for operator notification." />
            </div>
        }
        .into_any();
    }
    view! {
        <div class="stack">
            {render_push_banner(
                push_context,
                Some(format!(
                    "{} candidates mirrored for origin `{}`",
                    page.candidates.len(),
                    page.origin_node_id
                )),
                Some(format!(
                    "{} notification candidates are currently mirrored for this origin.",
                    page.candidates.len()
                )),
            )}
            <p class="muted">{format!("{} candidates from origin `{}`", page.candidates.len(), page.origin_node_id)}</p>
            {render_notification_candidates(page.candidates, workspace)}
        </div>
    }
    .into_any()
}

fn render_notification_candidates(
    candidates: Vec<NotificationCandidateView>,
    workspace: WorkspaceState,
) -> AnyView {
    if candidates.is_empty() {
        return view! { <p class="muted">"None."</p> }.into_any();
    }
    view! {
        <ul class="item-list">
            {candidates.into_iter().map(|candidate| {
                let href = format!("/inbox/{}", candidate.item_id);
                let selected = workspace.focus_matches_notification_candidate(
                    candidate.candidate_id.as_str(),
                    candidate.item_id.as_str(),
                );
                view! {
                    <li class=move || if selected { "item-card item-card-selected" } else { "item-card" }>
                        <div class="item-card-main">
                            <div class="item-card-topline">
                                <span class="status-pill">{candidate.status_label}</span>
                                <span class="muted">{candidate.origin_node_id.clone()}</span>
                            </div>
                            <a class="item-title" href=href>{candidate.title.clone()}</a>
                            <p class="item-summary">{candidate.summary.clone()}</p>
                            <p class="item-meta">{notification_status_hint(candidate.status)}</p>
                        </div>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

fn render_delivery_page(page: DeliveryPageView, workspace: WorkspaceState) -> AnyView {
    let push_context = push::current_push_open_context();
    if page.jobs.is_empty() {
        return view! {
            <div class="stack">
                {render_push_banner(
                    push_context,
                    Some("delivery jobs".to_string()),
                    Some("No mirrored delivery jobs are currently queued.".to_string()),
                )}
                <EmptyState title="No deliveries yet" body="Delivery jobs will appear once notification readiness triggers delivery work." />
            </div>
        }
        .into_any();
    }
    view! {
        <div class="stack">
            {render_push_banner(
                push_context,
                Some(format!("{} delivery jobs mirrored", page.jobs.len())),
                Some(format!(
                    "{} delivery jobs are currently mirrored on the server.",
                    page.jobs.len()
                )),
            )}
            {render_delivery_jobs(page.jobs, workspace)}
        </div>
    }
    .into_any()
}

fn render_delivery_jobs(jobs: Vec<DeliveryJobView>, workspace: WorkspaceState) -> AnyView {
    if jobs.is_empty() {
        return view! { <p class="muted">"None."</p> }.into_any();
    }
    view! {
        <ul class="item-list">
            {jobs.into_iter().map(|job| {
                let selected = workspace.focus_matches_delivery_job(
                    job.job_id.as_str(),
                    job.candidate_id.as_str(),
                );
                view! {
                    <li class=move || if selected { "item-card item-card-selected" } else { "item-card" }>
                        <div class="item-card-main">
                            <div class="item-card-topline">
                                <span class="status-pill">{job.status_label}</span>
                                <span class="muted">{job.transport_kind.clone()}</span>
                            </div>
                            <p class="item-title">{job.job_id.clone()}</p>
                            <p class="item-summary">{job.summary.clone()}</p>
                            <p class="item-meta">{delivery_status_hint(job.status)}</p>
                            <p class="item-meta">
                                {format!("candidate {} · subscription {}", job.candidate_id, job.subscription_id)}
                            </p>
                        </div>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

fn render_action_list_page(page: RemoteActionPageView, workspace: WorkspaceState) -> AnyView {
    if page.requests.is_empty() {
        return view! { <EmptyState title="No remote action requests" body="Create a remote action from an inbox item to populate this list." /> }
            .into_any();
    }

    let origin_node_id = page
        .requests
        .first()
        .map(|request| request.origin_node_id.clone())
        .unwrap_or_default();

    view! {
        <div class="stack">
            <p class="muted">{format!("{} requests", page.requests.len())}</p>
            {render_remote_action_requests(page.requests, origin_node_id, workspace)}
        </div>
    }
    .into_any()
}

fn render_remote_action_page(
    request: RemoteActionRequestView,
    watching: impl Fn() -> bool + 'static,
    workspace: WorkspaceState,
) -> AnyView {
    let status_label = remote_action_status_label(request.status);
    let status_hint = remote_action_status_hint(request.status);
    let is_active = watching();
    let push_context = push::current_push_open_context();
    let workspace_focus_note = match workspace.focus.as_ref() {
        Some(focus) if focus.request_id.as_deref() == Some(request.request_id.as_str()) => {
            "This remote action request is pinned as the current focus.".to_string()
        }
        Some(focus) => format!(
            "Pinned focus remains on {} · {}",
            focus.kind_label, focus.status_label
        ),
        None => "No action request is pinned in the workspace yet.".to_string(),
    };
    let terminal_panel: Option<(bool, &'static str, String)> = match request.status {
        OperatorRemoteActionRequestStatus::Completed => Some((
            false,
            "Action completed",
            "The daemon completed the request. Related mirrored inbox, notification, or delivery state may also have changed.".to_string(),
        )),
        OperatorRemoteActionRequestStatus::Failed => Some((
            true,
            "Action failed",
            request.error.clone().unwrap_or_else(|| {
                "The daemon reported a failure but did not return an error summary.".to_string()
            }),
        )),
        OperatorRemoteActionRequestStatus::Canceled => Some((
            false,
            "Action canceled",
            "This request was canceled on the server before completion.".to_string(),
        )),
        OperatorRemoteActionRequestStatus::Stale => Some((
            false,
            "Action became stale",
            "The server marked this request stale. Review mirrored inbox state for a newer request if one exists.".to_string(),
        )),
        _ => None,
    };
    view! {
        <div class="stack">
            {render_push_banner(
                push_context,
                Some(format!("remote action request {}", request.request_id)),
                Some(format!("Current mirrored status: {} · {}", request.status_label, status_hint)),
            )}
            <div class="info-panel">
                <strong>"Workspace context"</strong>
                <p>{format!("Active section: {}", workspace.active_section.label())}</p>
                <p>{workspace_focus_note.clone()}</p>
            </div>
            {move || match terminal_panel.as_ref() {
                Some((is_error, title, body)) if *is_error => view! {
                    <div class="error-panel">
                        <strong>{*title}</strong>
                        <p>{body.clone()}</p>
                    </div>
                }
                .into_any(),
                Some((_is_error, title, body)) => view! {
                    <div class="info-panel">
                        <strong>{*title}</strong>
                        <p>{body.clone()}</p>
                    </div>
                }
                .into_any(),
                None => view! {}.into_any(),
            }}
            <article class="card">
                <div class="item-card-topline">
                    <span class="status-pill">{status_label}</span>
                    <span class="muted">{request.action_label}</span>
                </div>
                <h3>{request.request_id.clone()}</h3>
                <p class="item-summary">{request.summary.clone()}</p>
                <p class="item-meta">{status_hint}</p>
                <p class="item-meta">
                    {format!(
                        "Related mirrored inbox item: {}",
                        request.item_id.clone()
                    )}
                </p>
                <dl class="detail-grid">
                    <div><dt>"Status"</dt><dd>{status_label}</dd></div>
                    <div><dt>"Claimed by"</dt><dd>{request.claimed_by.clone().unwrap_or_else(|| "none".to_string())}</dd></div>
                    <div><dt>"Completed at"</dt><dd>{request.completed_at.map(|time| time.to_rfc3339()).unwrap_or_else(|| "none".to_string())}</dd></div>
                    <div><dt>"Failed at"</dt><dd>{request.failed_at.map(|time| time.to_rfc3339()).unwrap_or_else(|| "none".to_string())}</dd></div>
                </dl>
                <div class="toolbar">
                    <A href={format!("/inbox/{}", request.item_id)}>"Open related inbox item"</A>
                </div>
                {move || match request.result.clone() {
                    Some(result) => view! {
                        <article class="card">
                            <h4>"Result"</h4>
                            <pre class="code-block">{serde_json::to_string_pretty(&result).unwrap_or_default()}</pre>
                        </article>
                    }.into_any(),
                    None => view! {}.into_any(),
                }}
                {move || match request.error.clone() {
                    Some(error) => view! {
                        <article class="card">
                            <h4>"Failure summary"</h4>
                            <ErrorPanel error=error />
                        </article>
                    }.into_any(),
                    None => view! {}.into_any(),
                }}
                <p class="muted">
                    {if is_active {
                        "Watching for status changes through the server wait API."
                    } else {
                        "Status is terminal."
                    }}
                </p>
            </article>
        </div>
    }
    .into_any()
}

fn render_remote_action_requests(
    requests: Vec<RemoteActionRequestView>,
    origin_node_id: String,
    workspace: WorkspaceState,
) -> AnyView {
    if requests.is_empty() {
        return view! {
            <p class="muted">
                {format!("No remote action requests recorded for origin `{origin_node_id}`.")}
            </p>
        }
        .into_any();
    }

    view! {
        <ul class="item-list">
            {requests.into_iter().map(|request| {
                let href = format!("/actions/{}", request.request_id);
                let selected = workspace.focus_matches_remote_action_request(
                    request.request_id.as_str(),
                );
                view! {
                    <li class=move || if selected { "item-card item-card-selected" } else { "item-card" }>
                <div class="item-card-main">
                    <div class="item-card-topline">
                        <span class="status-pill">{request.status_label}</span>
                        <span class="muted">{request.action_label}</span>
                    </div>
                    <a class="item-title" href=href>{request.request_id.clone()}</a>
                    <p class="item-summary">{request.summary.clone()}</p>
                    <p class="item-meta">{remote_action_status_hint(request.status)}</p>
                    <p class="item-meta">
                        {format!(
                            "claimed by {} · completed {} · failed {}",
                            request.claimed_by.clone().unwrap_or_else(|| "none".to_string()),
                                    request.completed_at
                                        .map(|time| time.to_rfc3339())
                                        .unwrap_or_else(|| "none".to_string()),
                                    request.failed_at
                                        .map(|time| time.to_rfc3339())
                                        .unwrap_or_else(|| "none".to_string()),
                                )}
                            </p>
                        </div>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn NotFoundPage() -> impl IntoView {
    view! {
        <PageFrame title="Not found" subtitle="The requested operator route does not exist">
            <EmptyState title="Route not found" body="Use the inbox, notifications, deliveries, or action routes from the nav." />
        </PageFrame>
    }
    .into_any()
}

#[cfg(target_arch = "wasm32")]
fn watch_error_or_log(error: String) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::error_1(&error.into());
}

fn workstream_status_options() -> Vec<(&'static str, &'static str)> {
    vec![
        ("active", "Active"),
        ("blocked", "Blocked"),
        ("completed", "Completed"),
    ]
}

fn workunit_status_options() -> Vec<(&'static str, &'static str)> {
    vec![
        ("ready", "Ready"),
        ("blocked", "Blocked"),
        ("running", "Running"),
        ("awaiting_decision", "Awaiting decision"),
        ("accepted", "Accepted"),
        ("needs_human", "Needs human"),
        ("completed", "Completed"),
    ]
}

fn parse_workstream_status(value: &str) -> Result<WorkstreamStatus, String> {
    match value {
        "active" => Ok(WorkstreamStatus::Active),
        "blocked" => Ok(WorkstreamStatus::Blocked),
        "completed" => Ok(WorkstreamStatus::Completed),
        other => Err(format!("unsupported workstream status `{other}`")),
    }
}

fn parse_workunit_status(value: &str) -> Result<WorkUnitStatus, String> {
    match value {
        "ready" => Ok(WorkUnitStatus::Ready),
        "blocked" => Ok(WorkUnitStatus::Blocked),
        "running" => Ok(WorkUnitStatus::Running),
        "awaiting_decision" => Ok(WorkUnitStatus::AwaitingDecision),
        "accepted" => Ok(WorkUnitStatus::Accepted),
        "needs_human" => Ok(WorkUnitStatus::NeedsHuman),
        "completed" => Ok(WorkUnitStatus::Completed),
        other => Err(format!("unsupported work unit status `{other}`")),
    }
}

fn workstream_status_value(status: WorkstreamStatus) -> &'static str {
    match status {
        WorkstreamStatus::Active => "active",
        WorkstreamStatus::Blocked => "blocked",
        WorkstreamStatus::Completed => "completed",
    }
}

fn workunit_status_value(status: WorkUnitStatus) -> &'static str {
    match status {
        WorkUnitStatus::Ready => "ready",
        WorkUnitStatus::Blocked => "blocked",
        WorkUnitStatus::Running => "running",
        WorkUnitStatus::AwaitingDecision => "awaiting_decision",
        WorkUnitStatus::Accepted => "accepted",
        WorkUnitStatus::NeedsHuman => "needs_human",
        WorkUnitStatus::Completed => "completed",
    }
}

fn workstream_status_label(status: WorkstreamStatus) -> String {
    humanize_snake_case(workstream_status_value(status))
}

fn workunit_status_label(status: WorkUnitStatus) -> String {
    humanize_snake_case(workunit_status_value(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_inbox_item_notice_distinguishes_push_opened_routes() {
        assert!(
            missing_inbox_item_notice(true).contains("missing or no longer actionable"),
            "push-opened routes should explain why the inbox item is missing"
        );
        assert!(
            missing_inbox_item_notice(false).contains("missing from the server"),
            "non push-opened routes should still be honest about mirrored state"
        );
    }

    #[test]
    fn missing_remote_action_notice_distinguishes_push_opened_routes() {
        assert!(
            missing_remote_action_notice(true).contains("missing or no longer visible"),
            "push-opened routes should explain why the request is missing"
        );
        assert!(
            missing_remote_action_notice(false).contains("missing from the server"),
            "non push-opened routes should still be honest about mirrored state"
        );
    }
}
