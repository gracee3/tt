#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

mod api;
mod push;
mod pwa;
mod storage;
mod watch;
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

use orcas_core::ipc::OperatorRemoteActionRequestStatus;
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
use workspace::{WorkspaceFocus, WorkspaceSection, WorkspaceState};

pub fn mount_app() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        pwa::register_service_worker();
        mount_to_body(App);
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
                            <Route path=path!("") view=InboxRoute />
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
