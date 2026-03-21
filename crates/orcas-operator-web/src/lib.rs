#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

mod api;
mod pwa;
mod storage;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::hooks::{use_navigate, use_params_map};
use leptos_router::path;

use orcas_core::ipc::OperatorRemoteActionRequestStatus;
use orcas_operator_core::{
    DeliveryJobView, DeliveryPageView, InboxDetailPageView, InboxItemCardView, InboxPageView,
    NotificationCandidateView, NotificationPageView, OperatorServerSettings, RemoteActionPageView,
    RemoteActionRequestView, action_kind_label, inbox_status_label, remote_action_status_label,
    source_kind_label,
};

pub fn mount_app() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        pwa::register_service_worker();
        leptos::mount_to_body(App);
    }
}

#[component]
pub fn App() -> impl IntoView {
    let settings = RwSignal::new(storage::load_settings());
    provide_context(settings);

    Effect::new(move |_| {
        storage::save_settings(&settings.get());
    });

    view! {
        <Router>
            <div class="app-shell">
                <header class="shell-header">
                    <div>
                        <p class="eyebrow">"Orcas operator web"</p>
                        <h1>"Mirrored operator control plane"</h1>
                    </div>
                    <SettingsPanel />
                </header>
                <nav class="shell-nav">
                    <A href="/inbox">"Inbox"</A>
                    <A href="/notifications">"Notifications"</A>
                    <A href="/deliveries">"Deliveries"</A>
                    <A href="/actions">"Actions"</A>
                </nav>
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
        </Router>
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
                        leptos::spawn_local(async move {
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
                        leptos::spawn_local(async move {
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
    let refresh_epoch = RwSignal::new(0u64);
    let watch_error = RwSignal::new(None::<String>);
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
        move |_| {
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
                leptos::spawn_local(async move {
                    let current_settings = settings.get_untracked();
                    if !storage::settings_ready(&current_settings) {
                        return;
                    }
                    let mut after_sequence =
                        match api::inbox_checkpoint(current_settings.clone()).await {
                            Ok(response) => response.checkpoint.sequence,
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

    view! {
        <PageFrame title="Actionable inbox" subtitle="Derived mirrored work that needs operator attention">
            <div class="toolbar">
                <button class="refresh-button" on:click=move |_| inbox.refetch()>"Refresh"</button>
                <span class="muted">"Auto-refreshes on server checkpoint changes while this view is open."</span>
            </div>
            {move || match watch_error.get() {
                Some(error) => view! { <ErrorPanel error=format!("Live refresh paused: {error}") /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match inbox.get() {
                None => view! { <p class="muted">"Loading inbox…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => render_inbox_page(page),
            }}
        </PageFrame>
    }
}

#[component]
fn InboxDetailRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let params = use_params_map();
    let item_id = move || params.with(|params| params.get("item_id").unwrap_or_default());
    let detail = LocalResource::new(move || {
        let settings = settings.get();
        let item_id = item_id();
        async move { api::load_inbox_item_detail(settings, item_id).await }
    });
    let navigator = use_navigate();
    let navigate = move |path: &str| navigator(path, Default::default());

    view! {
        <PageFrame title="Inbox item" subtitle="Mirrored read-model detail and available actions">
            {move || match detail.get() {
                None => view! { <p class="muted">"Loading item…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => render_inbox_detail_page(page, navigate.clone()),
            }}
        </PageFrame>
    }
}

#[component]
fn NotificationsRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let notifications = LocalResource::new(move || {
        let settings = settings.get();
        async move { api::load_notifications_page(settings).await }
    });

    view! {
        <PageFrame title="Notifications" subtitle="Server-side notification readiness">
            <button class="refresh-button" on:click=move |_| notifications.refetch()>"Refresh"</button>
            {move || match notifications.get() {
                None => view! { <p class="muted">"Loading notifications…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => render_notification_page(page),
            }}
        </PageFrame>
    }
}

#[component]
fn DeliveriesRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let deliveries = LocalResource::new(move || {
        let settings = settings.get();
        async move { api::load_deliveries_page(settings).await }
    });

    view! {
        <PageFrame title="Deliveries" subtitle="Notification delivery jobs and outcomes">
            <button class="refresh-button" on:click=move |_| deliveries.refetch()>"Refresh"</button>
            {move || match deliveries.get() {
                None => view! { <p class="muted">"Loading deliveries…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(page)) => render_delivery_page(page),
            }}
        </PageFrame>
    }
}

#[component]
fn ActionListRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
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
                Some(Ok(page)) => render_action_list_page(page),
            }}
        </PageFrame>
    }
}

#[component]
fn ActionRoute() -> impl IntoView {
    let settings = use_context::<RwSignal<OperatorServerSettings>>()
        .expect("settings context should be provided");
    let params = use_params_map();
    let request_id = move || params.with(|params| params.get("request_id").unwrap_or_default());
    let refresh_epoch = RwSignal::new(0u64);
    let watching = RwSignal::new(false);
    let watch_error = RwSignal::new(None::<String>);
    let action_request = LocalResource::new(move || {
        let settings = settings.get();
        let request_id = request_id();
        let _refresh_epoch = refresh_epoch.get();
        async move { api::load_action_request(settings, request_id).await }
    });

    Effect::new(move |_| {
        let should_watch = watching.get();
        let _settings_value = settings.get();
        let _request_id_value = request_id();
        let current = action_request.get();
        if !should_watch {
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
            if watching.get_untracked() {
                let refresh_epoch = refresh_epoch.clone();
                let watch_error = watch_error.clone();
                let watching = watching.clone();
                leptos::spawn_local(async move {
                    let mut after_updated_at = Some(request.updated_at);
                    loop {
                        match api::wait_for_remote_action_update(
                            settings_value.clone(),
                            request_id_value.clone(),
                            after_updated_at,
                            Some(30_000),
                        )
                        .await
                        {
                            Ok(Some(updated)) => {
                                after_updated_at = Some(updated.updated_at);
                                refresh_epoch.update(|value| *value += 1);
                                if !matches!(
                                    updated.status,
                                    OperatorRemoteActionRequestStatus::Pending
                                        | OperatorRemoteActionRequestStatus::Claimed
                                ) {
                                    watching.set(false);
                                    break;
                                }
                            }
                            Ok(None) => {
                                watching.set(false);
                                break;
                            }
                            Err(error) => {
                                watch_error.set(Some(error));
                                watching.set(false);
                                break;
                            }
                        }
                    }
                });
            }
        }
    });

    view! {
        <PageFrame title="Action request" subtitle="Remote operator intent routed back through the daemon">
            <button class="refresh-button" on:click=move |_| action_request.refetch()>"Refresh"</button>
            <button class="primary-button" on:click=move |_| watching.set(true)>"Watch status"</button>
            {move || match watch_error.get() {
                Some(error) => view! { <ErrorPanel error=error /> }.into_any(),
                None => view! {}.into_any(),
            }}
            {move || match action_request.get() {
                None => view! { <p class="muted">"Loading request…"</p> }.into_any(),
                Some(Err(error)) => view! { <ErrorPanel error=error /> }.into_any(),
                Some(Ok(None)) => view! { <EmptyState title="Request not found" body="This remote action request does not exist or is no longer visible." /> }.into_any(),
                Some(Ok(Some(request))) => render_remote_action_page(request, move || watching.get()),
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

fn render_inbox_page(page: InboxPageView) -> AnyView {
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
                {page.sections.into_iter().map(render_inbox_section).collect_view()}
            </div>
        </div>
    }
    .into_any()
}

fn render_inbox_section(section: orcas_operator_core::InboxSectionView) -> AnyView {
    view! {
        <article class="card">
            <header class="card-header">
                <div>
                    <p class="eyebrow">{source_kind_label(section.source_kind)}</p>
                    <h3>{section.title}</h3>
                </div>
            </header>
            <ul class="item-list">
                {section.items.into_iter().map(render_inbox_card).collect_view()}
            </ul>
        </article>
    }
    .into_any()
}

fn render_inbox_card(item: InboxItemCardView) -> AnyView {
    let href = format!("/inbox/{}", item.id);
    view! {
        <li class="item-card">
            <div class="item-card-main">
                <div class="item-card-topline">
                    <span class="status-pill">{item.status_label}</span>
                    <span class="muted">{item.source_kind_label}</span>
                </div>
                <a class="item-title" href=href>{item.title}</a>
                <p class="item-summary">{item.summary}</p>
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
    let title = item
        .as_ref()
        .map(|item| item.title.clone())
        .unwrap_or_else(|| "Missing inbox item".to_string());
    let item_id = item.as_ref().map(|item| item.id.clone());
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

    view! {
        <div class="stack">
            <article class="card">
                <p class="eyebrow">{item_id.unwrap_or_else(|| "unknown item".to_string())}</p>
                <h3>{title}</h3>
                <p class="item-summary">{summary}</p>
                {move || match item.as_ref() {
                    Some(item) => render_item_details(item).into_any(),
                    None => view! { <p class="muted">"No mirrored item payload available."</p> }.into_any(),
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
                            view! {
                                <button
                                    class="primary-button"
                                    disabled=move || submitting.get()
                                    on:click=move |_| {
                                        submitting.set(true);
                                        let _note_value = note.get();
                                        let _settings_value = settings.get();
                                        let _navigate = navigate.clone();
                                        #[cfg(target_arch = "wasm32")]
                                        leptos::spawn_local(async move {
                                            let result = api::submit_remote_action(
                                                _settings_value,
                                                item_id.clone().unwrap_or_default(),
                                                action_kind,
                                                Some("web-operator".to_string()),
                                                if _note_value.trim().is_empty() { None } else { Some(_note_value) },
                                                Some(api::generated_idempotency_key()),
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
                        }).collect_view()}
                    </div>
                </div>
            </article>

            <article class="card">
                <h3>"Related notification candidates"</h3>
                {render_notification_candidates(page.notification_candidates)}
            </article>

            <article class="card">
                <h3>"Related delivery jobs"</h3>
                {render_delivery_jobs(page.delivery_jobs)}
            </article>

            <article class="card">
                <h3>"Recent remote action requests"</h3>
                {render_remote_action_requests(page.remote_action_requests, origin_node_id)}
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
    }
    .into_any()
}

fn render_notification_page(page: NotificationPageView) -> AnyView {
    if page.candidates.is_empty() {
        return view! { <EmptyState title="No notification candidates" body="No mirrored inbox item is currently ready for operator notification." /> }
            .into_any();
    }
    view! {
        <div class="stack">
            <p class="muted">{format!("{} candidates from origin `{}`", page.candidates.len(), page.origin_node_id)}</p>
            {render_notification_candidates(page.candidates)}
        </div>
    }
    .into_any()
}

fn render_notification_candidates(candidates: Vec<NotificationCandidateView>) -> AnyView {
    if candidates.is_empty() {
        return view! { <p class="muted">"None."</p> }.into_any();
    }
    view! {
        <ul class="item-list">
            {candidates.into_iter().map(|candidate| {
                let href = format!("/inbox/{}", candidate.item_id);
                view! {
                    <li class="item-card">
                        <div class="item-card-main">
                            <div class="item-card-topline">
                                <span class="status-pill">{candidate.status_label}</span>
                                <span class="muted">{candidate.origin_node_id.clone()}</span>
                            </div>
                            <a class="item-title" href=href>{candidate.title.clone()}</a>
                            <p class="item-summary">{candidate.summary.clone()}</p>
                        </div>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

fn render_delivery_page(page: DeliveryPageView) -> AnyView {
    if page.jobs.is_empty() {
        return view! { <EmptyState title="No deliveries yet" body="Delivery jobs will appear once notification readiness triggers delivery work." /> }
            .into_any();
    }
    view! { <div class="stack">{render_delivery_jobs(page.jobs)}</div> }.into_any()
}

fn render_delivery_jobs(jobs: Vec<DeliveryJobView>) -> AnyView {
    if jobs.is_empty() {
        return view! { <p class="muted">"None."</p> }.into_any();
    }
    view! {
        <ul class="item-list">
            {jobs.into_iter().map(|job| {
                view! {
                    <li class="item-card">
                        <div class="item-card-main">
                            <div class="item-card-topline">
                                <span class="status-pill">{job.status_label}</span>
                                <span class="muted">{job.transport_kind.clone()}</span>
                            </div>
                            <p class="item-title">{job.job_id.clone()}</p>
                            <p class="item-summary">{job.summary.clone()}</p>
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

fn render_action_list_page(page: RemoteActionPageView) -> AnyView {
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
            {render_remote_action_requests(page.requests, origin_node_id)}
        </div>
    }
    .into_any()
}

fn render_remote_action_page(
    request: RemoteActionRequestView,
    watching: impl Fn() -> bool + 'static,
) -> AnyView {
    let status_label = remote_action_status_label(request.status);
    let is_active = watching();
    view! {
        <div class="stack">
            <article class="card">
                <div class="item-card-topline">
                    <span class="status-pill">{status_label}</span>
                    <span class="muted">{request.action_label}</span>
                </div>
                <h3>{request.request_id.clone()}</h3>
                <p class="item-summary">{request.summary.clone()}</p>
                <dl class="detail-grid">
                    <div><dt>"Status"</dt><dd>{status_label}</dd></div>
                    <div><dt>"Claimed by"</dt><dd>{request.claimed_by.clone().unwrap_or_else(|| "none".to_string())}</dd></div>
                    <div><dt>"Completed at"</dt><dd>{request.completed_at.map(|time| time.to_rfc3339()).unwrap_or_else(|| "none".to_string())}</dd></div>
                    <div><dt>"Failed at"</dt><dd>{request.failed_at.map(|time| time.to_rfc3339()).unwrap_or_else(|| "none".to_string())}</dd></div>
                </dl>
                {move || match request.result.clone() {
                    Some(result) => view! { <pre class="code-block">{serde_json::to_string_pretty(&result).unwrap_or_default()}</pre> }.into_any(),
                    None => view! {}.into_any(),
                }}
                {move || match request.error.clone() {
                    Some(error) => view! { <ErrorPanel error=error /> }.into_any(),
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
                view! {
                    <li class="item-card">
                        <div class="item-card-main">
                            <div class="item-card-topline">
                                <span class="status-pill">{request.status_label}</span>
                                <span class="muted">{request.action_label}</span>
                            </div>
                            <a class="item-title" href=href>{request.request_id.clone()}</a>
                            <p class="item-summary">{request.summary.clone()}</p>
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
