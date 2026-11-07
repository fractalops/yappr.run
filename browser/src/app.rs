//! The page commands and the tasks that read, receive, pair, and answer.

#[cfg(not(target_family = "wasm"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use crate::{
    entries,
    outbox::{self, Delivery},
    readers::{self, PendingPair, Readers},
    storage::{load_or_create_secret, Dir, RelayStore},
};
use iroh::{endpoint::Connection, Endpoint, EndpointId};
use n0_future::task::JoinHandle;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, Semaphore};
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;
use yappr::{
    choices::{QuestionState, Tracker},
    client::{Denied, Event, Follower},
    control::ControlClient,
    deliver,
    entry::Entry,
    health::{self, Health},
    net::{self, RelaySettings},
    pairing::{self, Invitation, MachineCode},
    proto::{Frame, Request},
    EndpointTicket,
};

/// How many deliveries the page takes at the same time.
const MAX_DELIVERIES: usize = 8;

#[derive(Debug, Clone, Serialize)]
struct WriterView {
    id: String,
    name: String,
    ticket: String,
    log: Option<String>,
    last_id: u64,
    questions: Vec<QuestionState>,
    enabled: bool,
    /// The machine has a delivery connection open to this page.
    delivering: bool,
    /// The page takes only deliveries from this machine, see
    /// `Reader::delivers_only`.
    delivers_only: bool,
    health: Health,
}
#[derive(Debug, Clone, Serialize)]
struct View {
    device_id: String,
    writers: Vec<WriterView>,
    selected: Option<String>,
    deliveries: Vec<Delivery>,
    pairing: bool,
    pairing_code: Option<String>,
    pairing_name: Option<String>,
    network: RelaySettings,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UiEvent {
    View {
        view: View,
    },
    /// New saved entries, and the question states of their machine.
    Entries {
        writer_id: String,
        rows: Vec<Row>,
        questions: Vec<QuestionState>,
    },
    Health {
        writer_id: String,
        health: Health,
    },
    Status {
        text: String,
    },
    PairFailed {
        message: String,
    },
    /// The pairing request: "sending", "waiting" (the writer has it), or
    /// "retrying" (the writer is not reachable yet).
    PairState {
        state: String,
    },
}
#[derive(Clone)]
struct Runtime {
    endpoint: Endpoint,
    client: ControlClient,
}
#[derive(Default)]
struct AppState {
    dir: Dir,
    #[cfg(not(target_family = "wasm"))]
    active: AtomicBool,
    runtime: Mutex<Option<Runtime>>,
    /// Takes the deliveries to the runtime's endpoint.
    receiver: Mutex<Option<JoinHandle<()>>>,
    book: Mutex<Option<Readers>>,
    /// The question states of each machine, built from its saved entries.
    trackers: Mutex<Option<BTreeMap<String, Tracker>>>,
    /// One entry is saved at a time, so that an entry from two connections
    /// is saved once.
    saving: Mutex<()>,
    tasks: Mutex<BTreeMap<String, JoinHandle<()>>>,
    pair_task: Mutex<Option<JoinHandle<()>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    health: Mutex<BTreeMap<String, Health>>,
    /// The open delivery connection of each machine.
    delivering: Mutex<BTreeMap<String, Connection>>,
    lifecycle: Mutex<()>,
    wake: Notify,
}
/// A saved entry and the time of the entry in Unix milliseconds, its place
/// in the feed.
#[derive(Debug, Clone, Serialize)]
struct Row {
    ms: i64,
    /// The log of the entry. Entry ids repeat in another log of a machine.
    log: String,
    entry: Entry,
}
/// The most entries that the page saves in one step while a machine sends
/// its history.
const BATCH: usize = 256;
/// The page callback for events.
#[derive(Clone)]
struct AppHandle {
    #[cfg(target_family = "wasm")]
    on_event: js_sys::Function,
    #[cfg(not(target_family = "wasm"))]
    on_event: Arc<dyn Fn(String) + Send + Sync>,
}
impl AppHandle {
    /// Send an event to the page as a JSON string.
    fn emit(&self, event: UiEvent) {
        if let Ok(json) = serde_json::to_string(&event) {
            #[cfg(target_family = "wasm")]
            self.on_event
                .call1(&JsValue::NULL, &JsValue::from_str(&json))
                .ok();
            #[cfg(not(target_family = "wasm"))]
            (self.on_event)(json);
        }
    }
}
fn warn(text: &str) {
    #[cfg(target_family = "wasm")]
    web_sys::console::warn_1(&text.into());
    #[cfg(not(target_family = "wasm"))]
    log::warn!("{}", yappr::proto::printable(text));
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
async fn book(state: &AppState) -> Result<Readers, String> {
    let mut slot = state.book.lock().await;
    if slot.is_none() {
        let mut readers = Readers::load(&state.dir).map_err(err)?;
        // Move the entries that an older page saved in the book.
        let mut moved = false;
        for (id, writer) in readers.writers.iter_mut() {
            if writer.entries.is_empty() {
                continue;
            }
            let log = writer.log.clone().unwrap_or_default();
            entries::save(&state.dir, id, &log, &writer.entries, &[])
                .await
                .map_err(err)?;
            let last = writer.entries.iter().map(|e| e.id).max().unwrap_or(0);
            writer.last_id = writer.last_id.max(last);
            writer.entries.clear();
            moved = true;
        }
        if moved {
            readers.save(&state.dir).map_err(err)?;
        }
        // Entries from before logs in IndexedDB have an empty log. They
        // belong to the current log of their machine.
        for (id, writer) in &readers.writers {
            if let Some(log) = &writer.log {
                entries::rename_log(&state.dir, id, "", log)
                    .await
                    .map_err(err)?;
            }
        }
        *slot = Some(readers);
    }
    Ok(slot.as_ref().expect("book exists").clone())
}
/// Update question states from a current or legacy entry.
fn track(tracker: &mut Tracker, entry: &Entry) {
    // Read the legacy question field without changing the signed entry.
    if !entry.extra.contains_key("choices") {
        if let Some(value) = entry.extra.get("rply") {
            let mut view = entry.clone();
            view.extra.insert("choices".into(), value.clone());
            tracker.push(&view);
            return;
        }
    }
    tracker.push(entry);
}
/// The question states of the machine `id`.
async fn questions(state: &AppState, id: &str) -> Result<Vec<QuestionState>, String> {
    let readers = book(state).await?;
    let mut slot = state.trackers.lock().await;
    if slot.is_none() {
        let mut all = BTreeMap::new();
        for id in readers.writers.keys() {
            let mut tracker = Tracker::default();
            for entry in entries::all(&state.dir, id).await.map_err(err)? {
                track(&mut tracker, &entry);
            }
            all.insert(id.clone(), tracker);
        }
        *slot = Some(all);
    }
    Ok(slot
        .as_ref()
        .expect("trackers exist")
        .get(id)
        .map(Tracker::states)
        .unwrap_or_default())
}
async fn update<T>(
    state: &AppState,
    apply: impl FnOnce(&mut Readers) -> Result<T, String>,
) -> Result<T, String> {
    book(state).await?;
    let mut slot = state.book.lock().await;
    let mut next = slot.as_ref().expect("book exists").clone();
    let result = apply(&mut next)?;
    next.save(&state.dir).map_err(err)?;
    *slot = Some(next);
    Ok(result)
}
#[cfg(not(target_family = "wasm"))]
fn is_active(state: &AppState) -> bool {
    state.active.load(Ordering::Acquire)
}
#[cfg(target_family = "wasm")]
fn is_active(_: &AppState) -> bool {
    true
}

async fn runtime(app: &AppHandle, state: &Arc<AppState>) -> Result<Runtime, String> {
    let mut slot = state.runtime.lock().await;
    if !is_active(state) {
        return Err("Open the app to connect.".into());
    }
    if let Some(runtime) = slot.as_ref() {
        return Ok(runtime.clone());
    }
    let secret = load_or_create_secret(&state.dir).map_err(err)?;
    let relays = <RelaySettings as RelayStore>::load(&state.dir).map_err(err)?;
    #[cfg(target_family = "wasm")]
    let endpoint = net::bind(secret, &relays).await.map_err(err)?;
    #[cfg(not(target_family = "wasm"))]
    let endpoint = {
        let opts = net::NetOptions {
            relays,
            ..net::Network::Default.into()
        };
        net::bind(secret, opts, vec![], None).await.map_err(err)?
    };
    if !is_active(state) {
        endpoint.close().await;
        return Err("Open the app to connect.".into());
    }
    endpoint.set_alpns(vec![deliver::ALPN.to_vec()]);
    let receiving = n0_future::task::spawn(receive(app.clone(), state.clone(), endpoint.clone()));
    if let Some(old) = state.receiver.lock().await.replace(receiving) {
        old.abort();
    }
    let runtime = Runtime {
        client: ControlClient::new(endpoint.clone()),
        endpoint,
    };
    Ok(slot.insert(runtime).clone())
}
async fn view(state: &AppState) -> Result<View, String> {
    let book = book(state).await?;
    let mut states = BTreeMap::new();
    for id in book.writers.keys() {
        states.insert(id.clone(), questions(state, id).await?);
    }
    let health = state.health.lock().await;
    let delivering = state.delivering.lock().await;
    let writers = book
        .writers
        .into_iter()
        .map(|(id, writer)| WriterView {
            health: health.get(&id).cloned().unwrap_or_else(|| {
                Health::state(match (writer.enabled, writer.delivers_only) {
                    (false, _) => "paused",
                    (true, true) => "waiting",
                    (true, false) => "connecting",
                })
            }),
            delivers_only: writer.delivers_only,
            delivering: delivering.contains_key(&id),
            questions: states.remove(&id).unwrap_or_default(),
            id,
            name: writer.name,
            ticket: writer.ticket,
            log: writer.log,
            last_id: writer.last_id,
            enabled: writer.enabled,
        })
        .collect();
    Ok(View {
        device_id: load_or_create_secret(&state.dir)
            .map_err(err)?
            .public()
            .to_string(),
        writers,
        selected: book.selected,
        deliveries: outbox::list(&state.dir).map_err(err)?,
        pairing: book.invitation.is_some(),
        pairing_code: book
            .invitation
            .as_ref()
            .map(|p| p.invitation.encode())
            .transpose()
            .map_err(err)?,
        pairing_name: book.invitation.map(|p| p.name),
        network: <RelaySettings as RelayStore>::load(&state.dir).map_err(err)?,
    })
}
async fn emit_view(app: &AppHandle, state: &AppState) {
    match view(state).await {
        Ok(view) => app.emit(UiEvent::View { view }),
        Err(error) => status(app, &error),
    }
}
fn pair_state(app: &AppHandle, state: &str) {
    app.emit(UiEvent::PairState {
        state: state.into(),
    });
}
fn status(app: &AppHandle, text: &str) {
    app.emit(UiEvent::Status { text: text.into() });
}
async fn set_health(app: &AppHandle, state: &AppState, id: &str, health: Health) {
    let mut all = state.health.lock().await;
    if all.get(id) == Some(&health) {
        return;
    }
    all.insert(id.into(), health.clone());
    app.emit(UiEvent::Health {
        writer_id: id.into(),
        health,
    });
}
/// The health of a machine from its feed connection. An open delivery
/// connection shows the machine as connected, so it takes precedence.
async fn set_follow_health(app: &AppHandle, state: &AppState, id: &str, health: Health) {
    if !state.delivering.lock().await.contains_key(id) {
        set_health(app, state, id, health).await;
    }
}
async fn resume(app: AppHandle, state: Arc<AppState>) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    resume_all(app.clone(), state.clone()).await?;
    view(&state).await
}
async fn resume_all(app: AppHandle, state: Arc<AppState>) -> Result<(), String> {
    let book = book(&state).await?;
    if book.writers.values().any(|w| w.enabled)
        || book.invitation.is_some()
        || outbox::list(&state.dir)
            .map_err(err)?
            .iter()
            .any(|d| d.status == "queued")
    {
        runtime(&app, &state).await?;
        start_worker(app.clone(), state.clone()).await;
    }
    // The page reads the feed of each enabled machine, but not of a machine
    // that only delivers.
    for (id, writer) in book
        .writers
        .into_iter()
        .filter(|(_, w)| w.enabled && !w.delivers_only)
    {
        let active = state
            .tasks
            .lock()
            .await
            .get(&id)
            .is_some_and(|t| !t.is_finished());
        if !active {
            start_follow(
                app.clone(),
                state.clone(),
                writer.ticket.parse().map_err(err)?,
            )
            .await;
        }
    }
    if let Some(pair) = book.invitation {
        let active = state
            .pair_task
            .lock()
            .await
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        if !active {
            start_pair(app, state, pair).await;
        }
    }
    Ok(())
}
async fn follow(
    app: AppHandle,
    state: Arc<AppState>,
    ticket: String,
    name: Option<String>,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let ticket: EndpointTicket = ticket.trim().parse().map_err(err)?;
    update(&state, |book| book.add(&ticket, name).map_err(err)).await?;
    start_follow(app.clone(), state.clone(), ticket).await;
    start_worker(app.clone(), state.clone()).await;
    let result = view(&state).await?;
    app.emit(UiEvent::View {
        view: result.clone(),
    });
    Ok(result)
}
async fn abort(task: Option<JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
        task.await.ok();
    }
}
async fn start_follow(app: AppHandle, state: Arc<AppState>, ticket: EndpointTicket) {
    if !is_active(&state) {
        return;
    }
    let id = ticket.endpoint_addr().id.to_string();
    abort(state.tasks.lock().await.remove(&id)).await;
    set_follow_health(&app, &state, &id, Health::default()).await;
    let shared = state.clone();
    let key = id.clone();
    let task = n0_future::task::spawn(async move {
        if let Err(error) = run_follow(&app, &shared, ticket).await {
            set_follow_health(&app, &shared, &key, Health::failure("stopped", error, None)).await;
        }
    });
    state.tasks.lock().await.insert(id, task);
}
// The observer never reads or cancels a framed feed read.
struct Observer(JoinHandle<()>);
impl Drop for Observer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
/// Save entries from a machine with the name of its log, and show them on the
/// page. Returns when the entries are on disk.
async fn save_entries(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    log: Option<&str>,
    batch: Vec<Entry>,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let _saving = state.saving.lock().await;
    update(state, |book| {
        book.set_log(id, log.map(String::from)).map_err(err)
    })
    .await?;
    let readers = book(state).await?;
    let log = readers
        .writers
        .get(id)
        .and_then(|w| w.log.clone())
        .unwrap_or_default();
    let mut new = Vec::new();
    for entry in batch {
        if readers.is_new(id, &entry).map_err(err)?
            && new.last().is_none_or(|e: &Entry| e.id < entry.id)
        {
            new.push(entry);
        }
    }
    let Some(last) = new.last().map(|e| e.id) else {
        return Ok(());
    };
    // Build the question states from the saved entries before these entries
    // are saved, so that the tracker gets each entry once.
    questions(state, id).await?;
    entries::save(&state.dir, id, &log, &new, &[])
        .await
        .map_err(err)?;
    let drop = update(state, |book| {
        book.saved(id, last).map_err(err)?;
        Ok(book.trim(id))
    })
    .await?;
    if !drop.is_empty() {
        entries::save(&state.dir, id, &log, &[], &drop)
            .await
            .map_err(err)?;
    }
    let states = {
        let mut slot = state.trackers.lock().await;
        let tracker = slot
            .as_mut()
            .expect("trackers exist")
            .entry(id.into())
            .or_default();
        for entry in &new {
            track(tracker, entry);
        }
        tracker.states()
    };
    app.emit(UiEvent::Entries {
        writer_id: id.into(),
        rows: new
            .into_iter()
            .map(|entry| Row {
                ms: entry.time.as_millisecond(),
                log: log.clone(),
                entry,
            })
            .collect(),
        questions: states,
    });
    Ok(())
}
async fn run_follow(
    app: &AppHandle,
    state: &Arc<AppState>,
    ticket: EndpointTicket,
) -> Result<(), String> {
    let endpoint = runtime(app, state).await?.endpoint;
    let id = ticket.endpoint_addr().id.to_string();
    loop {
        let saved = book(state)
            .await?
            .writers
            .get(&id)
            .cloned()
            .ok_or("The machine does not exist")?;
        let mut observer = None;
        // While the machine sends its history, entries are saved in batches.
        let mut batch = Vec::new();
        let mut live = false;
        // A failure after a connection that worked is a reconnect. A failure
        // with no connection since the last one means that the machine is
        // offline.
        let mut reached = false;
        let mut follower = Follower::new(
            endpoint.clone(),
            ticket.endpoint_addr().clone(),
            Request {
                since: saved.last_id,
                log: saved.log,
                ..Default::default()
            },
        );
        loop {
            match follower.next().await {
                Ok(Some(Event::Entry(entry))) => {
                    batch.push(entry);
                    if live || batch.len() >= BATCH {
                        save_entries(app, state, &id, follower.log(), std::mem::take(&mut batch))
                            .await?;
                    }
                }
                Ok(Some(Event::CaughtUp { .. })) => {
                    save_entries(app, state, &id, follower.log(), std::mem::take(&mut batch))
                        .await?;
                    live = true;
                    reached = true;
                    if let Some(conn) = follower.connection() {
                        set_follow_health(app, state, &id, health::snapshot(&conn)).await;
                        let (app, shared, id) = (app.clone(), state.clone(), id.clone());
                        observer = Some(Observer(n0_future::task::spawn(async move {
                            loop {
                                n0_future::time::sleep(Duration::from_secs(2)).await;
                                let health = health::snapshot(&conn);
                                let closed = conn.close_reason().is_some();
                                set_follow_health(&app, &shared, &id, health).await;
                                if closed {
                                    break;
                                }
                            }
                        })));
                    }
                }
                Ok(Some(Event::Retrying { delay, error })) => {
                    save_entries(app, state, &id, follower.log(), std::mem::take(&mut batch))
                        .await?;
                    // A new connection sends the history again first.
                    live = false;
                    observer.take();
                    let health = if std::mem::take(&mut reached) {
                        "reconnecting"
                    } else {
                        "offline"
                    };
                    set_follow_health(
                        app,
                        state,
                        &id,
                        Health::failure(health, error, Some(delay.as_millis() as u64)),
                    )
                    .await;
                }
                Ok(None) => return Ok(()),
                Err(error) if error.downcast_ref::<Denied>().is_some() => {
                    observer.take();
                    set_follow_health(
                        app,
                        state,
                        &id,
                        Health::failure("denied", error.to_string(), Some(3000)),
                    )
                    .await;
                    n0_future::time::sleep(Duration::from_secs(3)).await;
                    break;
                }
                Err(error) => return Err(format!("{error:#}")),
            }
        }
    }
}
/// Take the deliveries that machines send to `endpoint`.
async fn receive(app: AppHandle, state: Arc<AppState>, endpoint: Endpoint) {
    let permits = Arc::new(Semaphore::new(MAX_DELIVERIES));
    let mut tasks = n0_future::task::JoinSet::new();
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                if !is_active(&state) { incoming.refuse(); continue; }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    incoming.refuse();
                    continue;
                };
                let (app, state, me) = (app.clone(), state.clone(), endpoint.id());
                tasks.spawn(async move {
                    let _permit = permit;
                    let Ok(conn) = incoming.await else { return };
                    if let Err(error) = take_delivery(&app, &state, conn, me).await {
                        warn(&format!("delivery: {error}"));
                    }
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}
/// Save the entries of one delivery from an enabled machine, and tell the
/// machine each saved entry.
async fn take_delivery(
    app: &AppHandle,
    state: &Arc<AppState>,
    conn: Connection,
    me: EndpointId,
) -> Result<(), String> {
    if !is_active(state) {
        conn.close(0u32.into(), b"app inactive");
        return Ok(());
    }
    let incoming = deliver::accept(conn, me).await.map_err(err)?;
    let id = incoming.writer().to_string();
    let Some(saved) = book(state)
        .await?
        .writers
        .get(&id)
        .filter(|w| w.enabled)
        .cloned()
    else {
        incoming.deny();
        return Ok(());
    };
    let req = Request {
        since: saved.last_id,
        log: saved.log,
        ..Default::default()
    };
    let mut sub = incoming.subscribe(&req).await.map_err(err)?;
    let conn = sub.connection().clone();
    state
        .delivering
        .lock()
        .await
        .insert(id.clone(), conn.clone());
    set_health(app, state, &id, health::snapshot(&conn)).await;
    emit_view(app, state).await;
    // Queued answers for this machine can go on this connection now.
    state.wake.notify_one();
    let result = async {
        // While the machine sends its history, entries are saved in batches.
        let mut batch = Vec::new();
        let mut live = false;
        loop {
            match sub.next().await.map_err(err)? {
                Some(Frame::Entry { entry }) => {
                    let saved = entry.id;
                    batch.push(entry);
                    if live || batch.len() >= BATCH {
                        save_entries(app, state, &id, sub.log(), std::mem::take(&mut batch))
                            .await?;
                        sub.stored(saved).await.map_err(err)?;
                    }
                }
                Some(Frame::CaughtUp { head }) => {
                    save_entries(app, state, &id, sub.log(), std::mem::take(&mut batch)).await?;
                    sub.stored(head).await.map_err(err)?;
                    live = true;
                }
                Some(_) => {}
                None => return Ok(()),
            }
        }
    }
    .await;
    let mut delivering = state.delivering.lock().await;
    if delivering
        .get(&id)
        .is_some_and(|c| c.stable_id() == conn.stable_id())
    {
        delivering.remove(&id);
    }
    drop(delivering);
    state.health.lock().await.remove(&id);
    emit_view(app, state).await;
    // The connection ends when the machine finishes its delivery.
    if conn.close_reason().is_some() {
        return Ok(());
    }
    result
}
async fn select_writer(state: Arc<AppState>, writer_id: Option<String>) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    update(&state, |book| {
        book.selected = writer_id;
        Ok(())
    })
    .await?;
    view(&state).await
}
async fn set_writer_enabled(
    app: AppHandle,
    state: Arc<AppState>,
    writer_id: String,
    enabled: bool,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let (ticket, delivers_only) = update(&state, |book| {
        let writer = book
            .writers
            .get_mut(&writer_id)
            .ok_or("The machine does not exist")?;
        writer.enabled = enabled;
        Ok((writer.ticket.clone(), writer.delivers_only))
    })
    .await?;
    if enabled {
        if delivers_only {
            set_health(&app, &state, &writer_id, Health::state("waiting")).await;
        } else {
            start_follow(app.clone(), state.clone(), ticket.parse().map_err(err)?).await;
        }
        start_worker(app.clone(), state.clone()).await;
        state.wake.notify_one();
    } else {
        abort(state.tasks.lock().await.remove(&writer_id)).await;
        if let Some(conn) = state.delivering.lock().await.remove(&writer_id) {
            conn.close(yappr::proto::DENIED_CODE.into(), b"paused");
        }
        set_health(&app, &state, &writer_id, Health::state("paused")).await;
    }
    view(&state).await
}
async fn rename_writer(
    state: Arc<AppState>,
    writer_id: String,
    name: String,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    readers::validate_name(&name).map_err(err)?;
    update(&state, |book| {
        book.writers
            .get_mut(&writer_id)
            .ok_or("The machine does not exist")?
            .name = name;
        Ok(())
    })
    .await?;
    view(&state).await
}
async fn choose(
    app: AppHandle,
    state: Arc<AppState>,
    writer_id: String,
    question_id: String,
    value: String,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let book = book(&state).await?;
    let writer = book
        .writers
        .get(&writer_id)
        .ok_or("The machine does not exist")?;
    if !writer.enabled {
        return Err("Resume this machine before answering.".into());
    }
    let question = questions(&state, &writer_id)
        .await?
        .into_iter()
        .find(|q| q.question.id == question_id)
        .ok_or("The question does not exist")?;
    if question
        .question
        .expires_at
        .is_some_and(|expires| expires <= jiff::Timestamp::now())
        && question.status.as_str() == "pending"
    {
        return Err("The question expired.".into());
    }
    if question.status.as_str() != "pending" {
        return Err(format!("The question is {}.", question.status));
    }
    let local = load_or_create_secret(&state.dir).map_err(err)?.public();
    if question.question.responder != local {
        return Err("This question is for another device.".into());
    }
    if !question.question.options.iter().any(|o| o.value == value) {
        return Err("The option does not exist.".into());
    }
    let ticket: EndpointTicket = writer.ticket.parse().map_err(err)?;
    outbox::queue(&state.dir, &ticket, question_id, value).map_err(err)?;
    start_worker(app.clone(), state.clone()).await;
    state.wake.notify_one();
    view(&state).await
}
/// Send the queued answers again and again, one task for each machine.
async fn start_worker(app: AppHandle, state: Arc<AppState>) {
    let mut slot = state.worker.lock().await;
    if !is_active(&state) {
        return;
    }
    if slot.as_ref().is_some_and(|t| !t.is_finished()) {
        return;
    }
    let shared = state.clone();
    *slot = Some(n0_future::task::spawn(async move {
        loop {
            if let (Ok(runtime), Ok(readers), Ok(deliveries)) = (
                runtime(&app, &shared).await,
                book(&shared).await,
                outbox::list(&shared.dir),
            ) {
                let mut by_writer: BTreeMap<String, Vec<Delivery>> = BTreeMap::new();
                for mut delivery in deliveries.into_iter().filter(|d| d.status == "queued") {
                    let Some(writer) = readers.writers.get(&delivery.writer) else {
                        continue;
                    };
                    if !writer.enabled {
                        continue;
                    }
                    delivery.ticket = writer.ticket.clone();
                    by_writer
                        .entry(delivery.writer.clone())
                        .or_default()
                        .push(delivery);
                }
                let mut tasks = n0_future::task::JoinSet::new();
                for (writer, deliveries) in by_writer {
                    let (client, app, shared) =
                        (runtime.client.clone(), app.clone(), shared.clone());
                    tasks.spawn(async move {
                        for delivery in deliveries {
                            // Check the pause state again before each answer.
                            let enabled = book(&shared)
                                .await
                                .ok()
                                .and_then(|b| b.writers.get(&writer).map(|w| w.enabled))
                                .unwrap_or(false);
                            if !enabled {
                                break;
                            }
                            let delivering = shared.delivering.lock().await.get(&writer).cloned();
                            outbox::deliver_with(
                                &shared.dir,
                                &client,
                                &delivery,
                                delivering.as_ref(),
                            )
                            .await
                            .ok();
                            emit_view(&app, &shared).await;
                        }
                    });
                }
                while tasks.join_next().await.is_some() {}
            }
            tokio::select! {
                _ = n0_future::time::sleep(Duration::from_secs(3)) => {},
                _ = shared.wake.notified() => {},
            }
        }
    }));
}

#[derive(Serialize)]
struct PairPreview {
    name: String,
    writer_id: String,
    expires_at: String,
}
fn preview_invitation(invitation: String) -> Result<PairPreview, String> {
    let invitation = Invitation::decode(&invitation).map_err(err)?;
    let ticket: EndpointTicket = invitation.ticket.parse().map_err(err)?;
    Ok(PairPreview {
        name: invitation.name,
        writer_id: ticket.endpoint_addr().id.to_string(),
        expires_at: invitation.expires_at.to_string(),
    })
}
async fn pair_device(
    app: AppHandle,
    state: Arc<AppState>,
    invitation: String,
    name: String,
) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    let invitation = Invitation::decode(&invitation).map_err(err)?;
    readers::validate_name(&name).map_err(err)?;
    let pair = PendingPair { invitation, name };
    abort(state.pair_task.lock().await.take()).await;
    update(&state, |book| {
        book.invitation = Some(pair.clone());
        Ok(())
    })
    .await?;
    start_pair(app, state.clone(), pair).await;
    Ok(())
}
/// The address of this page, after the endpoint has a relay. The writer
/// delivers its feed to this address.
async fn own_ticket(endpoint: &Endpoint) -> Result<EndpointTicket, String> {
    n0_future::time::timeout(Duration::from_secs(15), endpoint.online())
        .await
        .map_err(|_| "The relay did not connect. Check your connection.".to_string())?;
    Ok(EndpointTicket::new(net::relay_addr(endpoint.addr())))
}
async fn start_pair(app: AppHandle, state: Arc<AppState>, pair: PendingPair) {
    let shared = state.clone();
    let task = n0_future::task::spawn(async move {
        let result = async {
            let runtime = runtime(&app, &shared).await?;
            let invitation = pair.invitation;
            let ticket: EndpointTicket = invitation.ticket.parse().map_err(err)?;
            pair_state(&app, "sending");
            let own = own_ticket(&runtime.endpoint).await?;
            // The writer has the request after its first reply.
            let mut delivered = false;
            loop {
                // The writer answers a claimed invitation for a short time after
                // its deadline, so keep asking until that window ends.
                let last_ask = invitation
                    .expires_at
                    .checked_add(pairing::RESULT_WINDOW)
                    .unwrap_or(invitation.expires_at);
                if last_ask <= jiff::Timestamp::now() {
                    return Err("The invitation expired. Create a new code on the computer.".into());
                }
                let reply: Result<pairing::PairReply, _> = runtime
                    .client
                    .exchange(
                        ticket.endpoint_addr().clone(),
                        pairing::ALPN,
                        &pairing::Join {
                            token: invitation.token.clone(),
                            name: pair.name.clone(),
                            ticket: Some(own.to_string()),
                        },
                    )
                    .await;
                match reply {
                    Ok(reply) if reply.status == "accepted" => {
                        let _guard = shared.lifecycle.lock().await;
                        update(&shared, |book| {
                            book.add(&ticket, Some(invitation.name.clone()))
                                .map_err(err)?;
                            book.invitation = None;
                            Ok(())
                        })
                        .await?;
                        emit_view(&app, &shared).await;
                        start_follow(app.clone(), shared.clone(), ticket).await;
                        start_worker(app.clone(), shared.clone()).await;
                        return Ok(());
                    }
                    Ok(reply) if reply.status == "rejected" => return Err(reply.message),
                    Err(error) => {
                        let text = format!("Pairing request failed: {error:#}");
                        warn(&text);
                        if !delivered {
                            pair_state(&app, "retrying");
                        }
                    }
                    Ok(_) => {
                        if !delivered {
                            delivered = true;
                            pair_state(&app, "waiting");
                        }
                    }
                }
                n0_future::time::sleep(Duration::from_secs(1)).await;
            }
        }
        .await;
        if let Err(message) = result {
            update(&shared, |book| {
                book.invitation = None;
                Ok(())
            })
            .await
            .ok();
            emit_view(&app, &shared).await;
            app.emit(UiEvent::PairFailed { message });
        }
    });
    *state.pair_task.lock().await = Some(task);
}
/// Allow the machine of a machine code, such as a CI job, to deliver to this
/// page. The page does not read its feed.
async fn allow_machine(
    app: AppHandle,
    state: Arc<AppState>,
    code: String,
    name: Option<String>,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    let machine = MachineCode::decode(&code).map_err(err)?;
    let name = name.unwrap_or(machine.name);
    update(&state, |book| {
        book.allow_machine(machine.id, name).map_err(err)
    })
    .await?;
    if let Some(all) = state.trackers.lock().await.as_mut() {
        all.entry(machine.id.to_string()).or_default();
    }
    // The page takes deliveries while its endpoint runs.
    runtime(&app, &state).await?;
    start_worker(app.clone(), state.clone()).await;
    let result = view(&state).await?;
    app.emit(UiEvent::View {
        view: result.clone(),
    });
    Ok(result)
}
async fn stop(app: AppHandle, state: Arc<AppState>) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    abort(state.pair_task.lock().await.take()).await;
    update(&state, |book| {
        book.invitation = None;
        Ok(())
    })
    .await?;
    emit_view(&app, &state).await;
    Ok(())
}
async fn configure_network(
    app: AppHandle,
    state: Arc<AppState>,
    settings: RelaySettings,
) -> Result<View, String> {
    let _guard = state.lifecycle.lock().await;
    settings.validate().map_err(err)?;
    if book(&state).await?.invitation.is_some() {
        return Err("Finish or cancel pairing before changing relays.".into());
    }
    RelayStore::save(&settings, &state.dir).map_err(err)?;
    let tasks = std::mem::take(&mut *state.tasks.lock().await);
    for task in tasks.into_values() {
        abort(Some(task)).await;
    }
    abort(state.worker.lock().await.take()).await;
    abort(state.receiver.lock().await.take()).await;
    state.delivering.lock().await.clear();
    if let Some(runtime) = state.runtime.lock().await.take() {
        runtime.client.close().await;
        runtime.endpoint.close().await;
    }
    state.health.lock().await.clear();
    resume_all(app.clone(), state.clone()).await?;
    view(&state).await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FollowArgs {
    ticket: String,
    name: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectArgs {
    writer_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnableArgs {
    writer_id: String,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameArgs {
    writer_id: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChooseArgs {
    writer_id: String,
    question_id: String,
    value: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageArgs {
    /// The place of the last entry that the page has: [ms, writer, log, id].
    before: Option<serde_json::Value>,
    limit: u32,
    writer_id: Option<String>,
}
#[derive(Deserialize)]
struct MachineArgs {
    code: String,
    name: Option<String>,
}
#[derive(Deserialize)]
struct InvitationArgs {
    invitation: String,
}
#[derive(Deserialize)]
struct PairArgs {
    invitation: String,
    name: String,
}
#[derive(Deserialize)]
struct NetworkArgs {
    settings: RelaySettings,
}

fn args<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(err)
}
fn json<T: Serialize>(result: Result<T, String>) -> Result<serde_json::Value, String> {
    result.and_then(|value| serde_json::to_value(value).map_err(err))
}

/// Run one page command.
async fn call(
    app: AppHandle,
    state: Arc<AppState>,
    command: &str,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match command {
        "snapshot" => json(view(&state).await),
        "entries" => {
            let a: PageArgs = args(value)?;
            book(&state).await?;
            let before = a.before.map(|b| b.to_string()).unwrap_or_default();
            entries::page(
                &state.dir,
                &before,
                a.limit.min(500),
                a.writer_id.as_deref().unwrap_or(""),
            )
            .await
            .map_err(err)
        }
        "resume" => json(resume(app, state).await),
        "preview_feed" => {
            let a: FollowArgs = args(value)?;
            let ticket: EndpointTicket = a.ticket.parse().map_err(err)?;
            Ok(serde_json::json!({"writer_id":ticket.endpoint_addr().id.to_string()}))
        }
        "follow" => {
            let a: FollowArgs = args(value)?;
            json(follow(app, state, a.ticket, a.name).await)
        }
        "select_writer" => {
            let a: SelectArgs = args(value)?;
            json(select_writer(state, a.writer_id).await)
        }
        "set_writer_enabled" => {
            let a: EnableArgs = args(value)?;
            json(set_writer_enabled(app, state, a.writer_id, a.enabled).await)
        }
        "rename_writer" => {
            let a: RenameArgs = args(value)?;
            json(rename_writer(state, a.writer_id, a.name).await)
        }
        "choose" => {
            let a: ChooseArgs = args(value)?;
            json(choose(app, state, a.writer_id, a.question_id, a.value).await)
        }
        "preview_machine" => {
            let a: MachineArgs = args(value)?;
            let machine = MachineCode::decode(&a.code).map_err(err)?;
            Ok(serde_json::json!({"name": machine.name, "writer_id": machine.id.to_string()}))
        }
        "allow_machine" => {
            let a: MachineArgs = args(value)?;
            json(allow_machine(app, state, a.code, a.name).await)
        }
        "preview_invitation" => {
            let a: InvitationArgs = args(value)?;
            json(preview_invitation(a.invitation))
        }
        "pair_device" => {
            let a: PairArgs = args(value)?;
            json(pair_device(app, state, a.invitation, a.name).await)
        }
        "stop" => json(stop(app, state).await),
        "configure_network" => {
            let a: NetworkArgs = args(value)?;
            json(configure_network(app, state, a.settings).await)
        }
        _ => Err(format!("unknown command {command:?}")),
    }
}

/// The reader of this page. The page creates one and keeps it.
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
#[derive(Clone)]
pub struct App {
    app: AppHandle,
    state: Arc<AppState>,
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen]
impl App {
    /// `on_event` receives each event as a JSON string.
    #[wasm_bindgen(constructor)]
    pub fn new(on_event: js_sys::Function) -> App {
        console_error_panic_hook::set_once();
        App {
            app: AppHandle { on_event },
            state: Arc::new(AppState::default()),
        }
    }

    /// Run an app command. `args` and the result are JSON strings. A failed
    /// command rejects with its message.
    pub fn invoke(&self, command: String, args: String) -> js_sys::Promise {
        let (app, state) = (self.app.clone(), self.state.clone());
        wasm_bindgen_futures::future_to_promise(async move {
            let value = if args.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&args).map_err(|e| JsValue::from_str(&e.to_string()))?
            };
            let result = call(app, state, &command, value)
                .await
                .map_err(|e| JsValue::from_str(&e))?;
            Ok(JsValue::from_str(&result.to_string()))
        })
    }
}

#[cfg(not(target_family = "wasm"))]
impl App {
    /// Create a reader with private files and a JSON event sink.
    pub fn new(dir: std::path::PathBuf, on_event: impl Fn(String) + Send + Sync + 'static) -> Self {
        Self {
            app: AppHandle {
                on_event: Arc::new(on_event),
            },
            state: Arc::new(AppState {
                dir: Dir::new(dir),
                active: AtomicBool::new(true),
                ..AppState::default()
            }),
        }
    }

    /// Run the same commands and return the same JSON values as the browser reader.
    pub async fn invoke(
        &self,
        command: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        call(self.app.clone(), self.state.clone(), command, args).await
    }

    /// Allow network tasks before the native view resumes.
    pub fn activate(&self) {
        self.state.active.store(true, Ordering::Release);
    }

    /// Prevent new network tasks as soon as the native view becomes inactive.
    pub fn deactivate(&self) {
        self.state.active.store(false, Ordering::Release);
    }

    /// Stop network tasks when the app leaves the foreground.
    pub async fn suspend(&self) {
        let state = &self.state;
        state.active.store(false, Ordering::Release);
        let _guard = state.lifecycle.lock().await;
        state.active.store(false, Ordering::Release);
        abort(state.pair_task.lock().await.take()).await;
        for task in std::mem::take(&mut *state.tasks.lock().await).into_values() {
            abort(Some(task)).await;
        }
        abort(state.worker.lock().await.take()).await;
        abort(state.receiver.lock().await.take()).await;
        for conn in std::mem::take(&mut *state.delivering.lock().await).into_values() {
            conn.close(0u32.into(), b"app suspended");
        }
        if let Some(runtime) = state.runtime.lock().await.take() {
            runtime.client.close().await;
            runtime.endpoint.close().await;
        }
        state.health.lock().await.clear();
        let _saving = state.saving.lock().await;
        *state.trackers.lock().await = None;
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod native_tests {
    use super::*;
    use serde_json::json;
    use yappr::disk;

    #[tokio::test]
    async fn inline_history_moves_once_and_preserves_identity_and_outbox() {
        let root = tempfile::tempdir().unwrap();
        let writer = iroh::SecretKey::generate().public();
        let ticket = EndpointTicket::new(iroh::EndpointAddr::new(writer)).to_string();
        let id = writer.to_string();
        let entries: Vec<Entry> = (1..=250).map(|id| serde_json::from_value(json!({
            "v":1,"id":id,"time":"2026-11-01T10:00:00Z","source":"old","kind":"event","title":format!("Saved {id}"),"body":"saved text"
        })).unwrap()).collect();
        disk::write(&root.path().join("readers.json"), &json!({"writers":{id.clone():{"ticket":ticket,"name":"Saved machine","entries":entries,"enabled":false}},"selected":id,"invitation":null})).unwrap();
        disk::write(&root.path().join("outbox.json"), &json!({"deliveries":[]})).unwrap();
        let dir = Dir::new(root.path().into());
        let identity = load_or_create_secret(&dir).unwrap().public().to_string();
        let app = App::new(root.path().into(), |_| {});
        let snapshot = app.invoke("snapshot", json!({})).await.unwrap();
        assert_eq!(snapshot["device_id"], identity);
        assert_eq!(snapshot["writers"][0]["last_id"], 250);
        assert!(snapshot["writers"][0].get("entries").is_none());
        assert_eq!(snapshot["writers"][0]["enabled"], false);
        let page = app
            .invoke(
                "entries",
                json!({"before":null,"limit":100,"writerId":null}),
            )
            .await
            .unwrap();
        assert_eq!(page.as_array().unwrap().len(), 100);
        assert_eq!(page[0]["entry"]["title"], "Saved 250");
        let saved: serde_json::Value = disk::read(&root.path().join("readers.json")).unwrap();
        assert!(saved["writers"][&id].get("entries").is_none());
        assert!(root.path().join("readers.inline.json").exists());
        let restarted = App::new(root.path().into(), |_| {});
        assert_eq!(
            restarted.invoke("snapshot", json!({})).await.unwrap()["device_id"],
            identity
        );
        assert_eq!(entries::all(&dir, &id).await.unwrap().len(), 250);
        assert_eq!(outbox::list(&dir).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn invalid_legacy_entries_do_not_block_migration_or_restart() {
        let root = tempfile::tempdir().unwrap();
        let writer = iroh::SecretKey::generate().public();
        let ticket = EndpointTicket::new(iroh::EndpointAddr::new(writer)).to_string();
        let id = writer.to_string();
        let entry: Entry = serde_json::from_value(json!({
            "v":1,"id":1,"time":"2026-11-01T10:00:00Z","source":"", "kind":"event","body":"old text"
        }))
        .unwrap();
        assert!(entry.validate().is_err());
        disk::write(&root.path().join("readers.json"), &json!({
            "writers":{id.clone():{"ticket":ticket,"name":"Old machine","entries":[entry],"enabled":false}},
            "selected":id,"invitation":null
        })).unwrap();
        for _ in 0..2 {
            let app = App::new(root.path().into(), |_| {});
            let view = app.invoke("snapshot", json!({})).await.unwrap();
            assert_eq!(view["writers"][0]["last_id"], 1);
            let page = app.invoke("entries", json!({"limit":100})).await.unwrap();
            assert_eq!(page[0]["entry"]["body"], "old text");
        }
    }

    #[tokio::test]
    async fn the_single_writer_cache_remains_readable() {
        let root = tempfile::tempdir().unwrap();
        let ticket = EndpointTicket::new(iroh::EndpointAddr::new(
            iroh::SecretKey::generate().public(),
        ))
        .to_string();
        disk::write(
            &root.path().join("reader.json"),
            &json!({"ticket":ticket,"entries":[],"writer_name":"Old machine","invitation":null}),
        )
        .unwrap();
        let app = App::new(root.path().into(), |_| {});
        assert_eq!(
            app.invoke("snapshot", json!({})).await.unwrap()["writers"][0]["name"],
            "Old machine"
        );
        assert!(root.path().join("reader.json").exists());
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn legacy_questions_keep_their_decisions_without_rewriting_entries() {
        let responder = iroh::SecretKey::generate().public();
        let id = yappr::choices::random_id();
        let make = |id, record| {
            serde_json::from_value::<Entry>(serde_json::json!({
            "v":1,"id":id,"time":"2026-11-01T10:00:00Z","source":"test","kind":"event","body":"saved", "rply":record,
        })).unwrap()
        };
        let question = make(
            1,
            serde_json::json!({"type":"question","question":{
                "id":id,"source":"test","title":"Continue?","body":"Saved question",
                "options":[{"value":"yes","label":"Yes"},{"value":"no","label":"No"}],
                "responder":responder,"expires_at":null,"approval":false,
            }}),
        );
        let decision = make(
            2,
            serde_json::json!({"type":"decision","decision":{
                "question_id":id,"submission_id":yappr::choices::random_id(),"value":"yes","responder":responder,"time":"2026-11-01T10:00:01Z",
            }}),
        );
        let original = serde_json::to_value(&question).unwrap();
        let mut tracker = Tracker::default();
        track(&mut tracker, &question);
        track(&mut tracker, &decision);
        let states = serde_json::to_value(tracker.states()).unwrap();
        assert_eq!(states[0]["status"], "answered");
        assert_eq!(states[0]["decision"]["value"], "yes");
        assert_eq!(serde_json::to_value(&question).unwrap(), original);
        assert!(!question.extra.contains_key("choices"));
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod lifecycle_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn a_choice_after_suspension_stays_queued_without_starting_network_tasks() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::new(root.path().into());
        let responder = load_or_create_secret(&dir).unwrap().public();
        let writer = iroh::SecretKey::generate().public();
        let ticket = EndpointTicket::new(iroh::EndpointAddr::new(writer));
        let question_id = yappr::choices::random_id();
        let entry: Entry = serde_json::from_value(json!({"v":1,"id":1,"time":"2026-11-01T10:00:00Z","source":"test","kind":"question","body":"saved","choices":{
            "type":"question","question":{"id":question_id,"source":"test","title":"Continue?","body":"saved",
            "options":[{"value":"yes","label":"Yes"},{"value":"no","label":"No"}],"responder":responder,"expires_at":null,"approval":false}
        }})).unwrap();
        let mut book = Readers::default();
        let id = book.add(&ticket, Some("Test machine".into())).unwrap();
        book.save(&dir).unwrap();
        let app = App::new(root.path().into(), |_| {});
        app.invoke("snapshot", json!({})).await.unwrap();
        // A save can stop after the cursor advances and before tracking the entry.
        entries::save(&app.state.dir, &id, "", &[entry], &[])
            .await
            .unwrap();
        update(&app.state, |book| book.saved(&id, 1).map_err(err))
            .await
            .unwrap();
        app.suspend().await;
        // A foreground countdown can finish as the app becomes hidden.
        let result = app
            .invoke(
                "choose",
                json!({"writerId":id,"questionId":question_id,"value":"yes"}),
            )
            .await
            .unwrap();
        assert_eq!(result["deliveries"][0]["status"], "queued");
        assert!(app.state.worker.lock().await.is_none());
        assert!(app.state.runtime.lock().await.is_none());
    }
}
