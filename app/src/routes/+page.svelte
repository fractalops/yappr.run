<script lang="ts">
  import CodeInput from "$lib/CodeInput.svelte";
  import { invoke, listen } from "$lib/reader";
  import { onMount, tick } from "svelte";
  import { base } from "$app/paths";
  import { linkSegments, renderMarkdown } from "$lib/body";

  type Entry = { id: number; time: string; source: string; kind: string; level?: string; subject?: string; title?: string; body: string;
    format?: string; data?: Record<string, unknown>; sig?: string; choices?: { type?: string; question?: { id: string } } };
  type Question = { question: { id: string; title: string; body: string; options: { value: string; label: string }[];
    responder: string; expires_at: string | null; approval: boolean }; status: string; decision: { value: string; responder: string } | null };
  type Delivery = { decision?:Question["decision"]; ticket: string; writer: string; submission: { question_id: string; value: string }; status: string; error: string | null };
  type Health = { state: string; route: string | null; address: string | null; rtt_ms: number | null; error: string | null; retry_in_ms: number | null };
  type Writer = { id: string; name: string; ticket: string; log: string | null; last_id: number; questions: Question[]; enabled: boolean; delivering: boolean; delivers_only: boolean; health: Health };
  // One saved entry and its place in the feed: time, machine, id.
  type Row = { writer: string; log: string; ms: number; entry: Entry };
  type Network = { disabled: boolean; urls: string[] };
  type View = { device_id: string; writers: Writer[]; selected: string | null; deliveries: Delivery[];
    pairing: boolean; pairing_code: string | null; pairing_name: string | null; network: Network };
  type Preview = { name: string; writer_id: string; expires_at: string|null; ticket?:string; machine?:string };
  type FeedEvent = { type: "view"; view: View } | { type: "status"; text: string } | { type: "pair_failed"; message: string }
    | { type: "pair_state"; state: "sending" | "waiting" | "retrying" }
    | { type: "health"; writer_id: string; health: Health }
    | { type: "entries"; writer_id: string; rows: { ms: number; log: string; entry: Entry }[]; questions: Question[] };
  // The page loads the feed in parts of this many entries, newest first.
  const PAGE = 100;

  let selectedMachine = $state("");
  let screen=$state('feed');
  let followSuccess=$state('');
  let view = $state<View>({ device_id: "", writers: [], selected: null, deliveries: [], pairing: false,
    pairing_code: null, pairing_name: null, network: { disabled: false, urls: [] } });
  let relayMode = $state("default");
  let relayUrls = $state("");
  let savingNetwork = $state(false);
  let machineName = $state("");
  let status = $state("Reading saved entries…");
  let error = $state("");
  let code = $state("");
  let name = $state("My browser");
  let preview = $state<Preview | null>(null);
  let pairing = $state(false);
  // The steps of a pairing request: sending, then waiting for the writer's
  // approval, then approved. "retrying" means the writer is not reachable yet.
  let pairStep = $state<"idle" | "sending" | "waiting" | "retrying" | "approved">("idle");
  let pairWriter = $state("");
  const APPROVED_MS = 1100;
  // The reader loads after the page. In a browser build it is a large
  // WebAssembly file, so it can arrive several seconds later.
  let readerReady = $state(false);
  let copied = $state(false);
  let tab = $state("all");
  // The key of the one entry that shows its full text and details.
  let expanded = $state<string | null>(null);
  let busy = $state("");
  // Choices wait UNDO_MS before they leave the device, so a wrong tap can be undone.
  const UNDO_MS = 5000;
  let sending = $state<Record<string, { value: string; label: string; at: number }>>({});
  const sendTimers = new Map<string, ReturnType<typeof setTimeout>>();
  let now = $state(Date.now());
  // The last time that each machine delivered to this page, since the page loaded.
  let deliveredAt = $state<Record<string, string>>({});
  $effect(() => {
    for (const w of view.writers) if (w.delivering) deliveredAt[w.id] = new Date().toISOString();
  });
  const hasConnections=$derived(view.writers.length>0);
  const hasHistory=$derived(view.writers.some(w=>w.last_id>0));
  // The loaded entries, newest first, and whether older entries can exist.
  let rows = $state<Row[]>([]);
  let older = $state(false);
  let loading = $state(false);
  // The place of a row: newest first, then by machine, log, and id, as in
  // storage.
  function before(a: Row, b: Row): boolean {
    if (a.ms !== b.ms) return a.ms > b.ms;
    if (a.writer !== b.writer) return a.writer > b.writer;
    if (a.log !== b.log) return a.log > b.log;
    return a.entry.id > b.entry.id;
  }
  // An entry id repeats in another log of a machine, so the key has the log.
  const rowKey = (r: Row) => `${r.writer}:${r.log}:${r.entry.id}`;
  // Add new rows in their places. While older rows are not loaded, rows that
  // belong after the last loaded row wait for "Show older entries".
  function merged(old: Row[], add: Row[]): Row[] {
    const keys = new Set(old.map(rowKey));
    const last = old.at(-1);
    const fresh = add.filter((r) => !keys.has(rowKey(r)) && (!older || !last || before(r, last)));
    const all = [...old, ...fresh].sort((a, b) => (before(a, b) ? -1 : 1));
    // A long catch-up shows its newest page; the rest is one click away.
    if (all.length > Math.max(old.length, PAGE)) { older = true; return all.slice(0, Math.max(old.length, PAGE)); }
    return all;
  }
  async function loadRows(reset: boolean) {
    if (loading) return;
    loading = true;
    try {
      const last = reset ? null : rows.at(-1);
      const page = await invoke<Row[]>("entries", { before: last ? [last.ms, last.writer, last.log, last.entry.id] : null,
        limit: PAGE, writerId: selectedMachine || null });
      rows = reset ? page : [...rows, ...page];
      older = page.length === PAGE;
    } catch (e) { error = String(e); }
    finally { loading = false; }
  }

  type FeedItem = { key: string; writer: Writer; log: string; entry: Entry; question?: Question };
  const machines = $derived(view.writers.map(w=>({id:w.id,name:w.name})));
  const selectedWriters = $derived(view.writers.filter((w) => !selectedMachine || w.id === selectedMachine));
  const feed = $derived.by(() => {
    const writers = new Map(view.writers.map((w) => [w.id, w]));
    const questions = new Map(view.writers.flatMap((w) => w.questions.map((q) => [`${w.id}:${q.question.id}`, q] as const)));
    // A state entry replaces the older state entries with the same subject.
    // The rows are newest first, so the first one of a subject stays.
    const seen = new Set<string>();
    return rows.flatMap((row): FeedItem[] => {
      const writer = writers.get(row.writer);
      const { entry } = row;
      if (!writer || (selectedMachine && writer.id !== selectedMachine)) return [];
      if (entry.choices?.type === "decision" || entry.choices?.type === "cancelled") return [];
      if (entry.kind === "state" && entry.subject) {
        const subject = `${writer.id}:${entry.subject}`;
        if (seen.has(subject)) return [];
        seen.add(subject);
      }
      return [{ key: rowKey(row), writer, log: row.log, entry,
        question: entry.choices?.type === "question" ? questions.get(`${writer.id}:${entry.choices.question?.id ?? ""}`) : undefined }];
    });
  });
  // Count the open questions of all saved entries, also the ones that the
  // page has not loaded.
  const pending = $derived(selectedWriters.reduce((n, w) => n + w.questions.filter((q) => stateOf(q) === "pending").length, 0));
  const visibleFeed = $derived(tab === "open" ? feed.filter((item) => item.question && stateOf(item.question) === "pending") : feed);
  const connected = $derived(selectedWriters.filter((w) => w.delivering || w.health.state === "connected").length);
  const allConnected = $derived(connected>0 && connected===selectedWriters.length);
  const connectionLabel = $derived(selectedWriters.length===1 ? writerLabel(selectedWriters[0]) : selectedWriters.length ? `${connected}/${selectedWriters.length} connected` : 'Paused');

  function healthLabel(health: Health): string {
    if (health.state === "connected") return `${health.route === "relay" ? "Relay" : health.route === "direct" ? "Direct" : "Connected"}${health.rtt_ms !== null ? ` · ${Math.round(health.rtt_ms)} ms` : ""}`;
    const labels: Record<string, string> = { connecting: "Connecting", reconnecting: "Reconnecting", offline: "Offline", waiting: "Waiting for deliveries", denied: "Access denied", paused: "Paused", disconnected: "Disconnected", stopped: "Stopped" };
    return labels[health.state] ?? health.state;
  }
  // A machine that delivers to this page is connected, also when the page
  // cannot reach it. After a delivery, the label shows when it was.
  function writerLabel(writer: Writer): string {
    if (!writer.enabled) return "Paused";
    if (writer.health.state === "connected") return healthLabel(writer.health);
    if (writer.delivering) return "Delivering";
    const delivered = deliveredAt[writer.id];
    if (!delivered || !["offline", "reconnecting", "connecting", "waiting"].includes(writer.health.state)) return healthLabel(writer.health);
    if (writer.health.state === "waiting") return `Last delivery ${clock(delivered)}`;
    return `${healthLabel(writer.health)} · last delivery ${clock(delivered)}`;
  }
  function openDevice() { screen='devices'; }
  function switchTab(event:KeyboardEvent) {
    if(!['ArrowLeft','ArrowRight','Home','End'].includes(event.key))return;
    event.preventDefault();
    const values=['feed','devices'];
    const next=event.key==='Home'?values[0]:event.key==='End'?values[1]:values[screen===values[0]?1:0];
    screen=next;
    document.getElementById(`${next}-tab`)?.focus();
  }
  function selectWriter(event:Event) { selectedMachine=(event.target as HTMLSelectElement).value; void loadRows(true); }
  let copiedEntry=$state('');
  function shortId(id:string) { return id.length>16 ? `${id.slice(0,8)}…${id.slice(-4)}` : id; }
  async function copy(text:string,key:string) {
    try{await navigator.clipboard.writeText(text);copiedEntry=key;setTimeout(()=>{if(copiedEntry===key)copiedEntry='';},1600);}
    catch{error='The clipboard is unavailable.';}
  }
  // The title and the body as people read them. A Markdown body keeps its syntax.
  function copyText(item:FeedItem) {
    void copy([item.entry.title,item.entry.body].filter(Boolean).join("\n\n"),`${item.key}:text`);
  }
  // The entry with its machine and log, which a program needs to check the signature.
  function copySigned(item:FeedItem) {
    void copy(JSON.stringify({writer:item.writer.id,log:item.log,entry:item.entry}),`${item.key}:signed`);
  }
  async function toggleWriter(writer: Writer) {
    try { view = await invoke<View>("set_writer_enabled", { writerId: writer.id, enabled: !writer.enabled }); }
    catch (e) { error = String(e); }
  }
  async function renameWriter(writer: Writer, event: Event) {
    try { view = await invoke<View>("rename_writer", { writerId: writer.id, name: (event.target as HTMLInputElement).value.trim() }); }
    catch (e) { error = String(e); }
  }
  async function saveNetwork() {
    savingNetwork = true; error = "";
    try {
      view = await invoke<View>("configure_network", { settings: { disabled: relayMode === "direct", urls: relayMode === "custom" ? relayUrls.split(/\s+/).filter(Boolean) : [] } });
    } catch (e) { error = String(e); }
    finally { savingNetwork = false; }
  }
  function stateOf(q: Question): string {
    if (q.status === "pending" && q.question.expires_at && new Date(q.question.expires_at).getTime() <= now) return "expired";
    return q.status;
  }
  function deliveryFor(q: Question, writer: Writer) {
    return view.deliveries.findLast((d) => d.submission.question_id === q.question.id
      && d.writer === writer.id);
  }
  function selectedLabel(q: Question, value: string): string {
    return q.question.options.find((o) => o.value === value)?.label ?? value;
  }
  // A decided approval reads as a past outcome, not as the button label.
  function outcome(q: Question, value: string): string {
    if (q.question.approval && value === "approve") return "Approved";
    if (q.question.approval && value === "reject") return "Rejected";
    return selectedLabel(q, value);
  }
  // An approval's answer is good or bad. A choice between options is neither.
  function tone(q: Question, value: string): "good" | "bad" | "neutral" {
    if (!q.question.approval) return "neutral";
    return value === "approve" ? "good" : "bad";
  }
  function when(time: string): string {
    return new Date(time).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }
  function clock(time: string): string {
    return new Date(time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  function dayLabel(date: Date): string {
    const day = new Date(now);
    if (date.toDateString() === day.toDateString()) return "Today";
    day.setDate(day.getDate() - 1);
    if (date.toDateString() === day.toDateString()) return "Yesterday";
    return date.toLocaleDateString([], { month: "short", day: "numeric" });
  }
  function feedTime(time: string): string {
    const day = dayLabel(new Date(time));
    return day === "Today" ? clock(time) : day;
  }
  function detailTime(time: string): string {
    return `${dayLabel(new Date(time))}, ${clock(time)}`;
  }
  function expiryTime(time: string): string {
    const date = new Date(time);
    return date.toDateString() === new Date(now).toDateString() ? clock(time)
      : `${date.toLocaleDateString([], { month: "short", day: "numeric" })} ${clock(time)}`;
  }
  function responderFor(q: Question): string {
    return q.decision?.responder ?? q.question.responder;
  }
  // Before levels, the kind was the level, and other entries had the kind "info".
  function levelOf(entry: Entry): string {
    const level = entry.level ?? entry.kind;
    return level === "warn" || level === "alert" ? level : "info";
  }
  function kindOf(entry: Entry): string {
    return ["info", "warn", "alert"].includes(entry.kind) ? "event" : entry.kind;
  }
  // Opening an entry closes the open one. When the closed entry is above,
  // the page scrolls so that the opened entry stays where it was tapped.
  async function toggleEntry(key: string, row: Element) {
    const top = row.getBoundingClientRect().top;
    expanded = expanded === key ? null : key;
    await tick();
    window.scrollBy(0, row.getBoundingClientRect().top - top);
  }
  // A tap on a closed entry opens it. An open entry closes from its header
  // only, so its text stays easy to select. Controls in the row do not toggle.
  function rowClick(event: MouseEvent, key: string) {
    const target = event.target as Element;
    if (target.closest("button, a, input, select, textarea, label")) return;
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed && (event.currentTarget as Element).contains(selection.anchorNode)) return;
    if (expanded === key && !target.closest(".row-head")) return;
    toggleEntry(key, event.currentTarget as Element);
  }
  function headKey(event: KeyboardEvent, key: string) {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    toggleEntry(key, (event.currentTarget as Element).closest(".feed-row") ?? (event.currentTarget as Element));
  }
  async function readCode(value:string) {
    error='';
    code=value.trim();preview=null;followSuccess='';
    try {
      if(/^yappr-machine:/i.test(code)) {
        const details=await invoke<{name:string;writer_id:string}>('preview_machine',{code});
        machineName=details.name;preview={name:details.name,writer_id:details.writer_id,expires_at:null,machine:code};
      }
      else if(/^yappr:/i.test(code))preview=await invoke<Preview>('preview_invitation',{invitation:code});
      else {
        const details=await invoke<{writer_id:string}>('preview_feed',{ticket:code});
        machineName='My computer';preview={name:machineName,writer_id:details.writer_id,expires_at:null,ticket:code};
      }
    }catch(e){error=String(e);}
  }
  async function requestPair() {
    if(!preview)return;
    error='';
    if(preview.machine) {
      try{view=await invoke<View>('allow_machine',{code:preview.machine,name:machineName.trim()||null});followSuccess=machineName.trim();preview=null;}catch(e){error=String(e);}
      return;
    }
    if(preview.ticket) {
      try{view=await invoke<View>('follow',{ticket:preview.ticket,name:machineName.trim()||null});followSuccess=machineName.trim()||'Computer';preview=null;}catch(e){error=String(e);}
      return;
    }
    pairWriter=preview.name;pairStep='sending';
    try{await invoke('pair_device',{invitation:code,name});pairing=true;preview=null;}catch(e){error=String(e);pairStep='idle';}
  }
  function startChoice(writerId: string, id: string, value: string, label: string) {
    const key = `${writerId}:${id}`;
    sending = { ...sending, [key]: { value, label, at: Date.now() + UNDO_MS } };
    sendTimers.set(key, setTimeout(() => sendChoice(writerId, id, value), UNDO_MS));
  }
  function undoChoice(key: string) {
    clearTimeout(sendTimers.get(key));
    sendTimers.delete(key);
    const { [key]: _, ...rest } = sending; sending = rest;
  }
  async function sendChoice(writerId: string, id: string, value: string) {
    const key = `${writerId}:${id}`;
    sendTimers.delete(key);
    busy = key; error = "";
    try { view = await invoke<View>("choose", { writerId, questionId: id, value }); }
    catch (e) { error = String(e); }
    finally {
      const { [key]: _, ...rest } = sending; sending = rest;
      busy = "";
    }
  }
  async function copyId() {
    try { await navigator.clipboard.writeText(view.device_id); copied = true; setTimeout(() => copied = false, 1500); }
    catch { error = "Select the device ID and copy it."; }
  }

  onMount(() => {
    let active = true;
    const unlisten = listen<FeedEvent>((payload) => {
      if (payload.type === "view") {
        const wasPairing = pairing || view.pairing;
        view = payload.view;
        if (wasPairing && view.writers.length && !view.pairing) {
          // Show the approval for a moment before the feed replaces it.
          pairStep = "approved"; followSuccess=pairWriter;
          setTimeout(() => { pairing = false; pairStep = "idle"; }, APPROVED_MS);
        }
      } else if (payload.type === "pair_failed") { pairing = false; pairStep = "idle"; screen="devices"; error = payload.message; }
      else if (payload.type === "pair_state") { if (pairStep !== "approved") pairStep = payload.state; }
      else if (payload.type === "health") view = { ...view, writers: view.writers.map((writer) => writer.id === payload.writer_id ? { ...writer, health: payload.health } : writer) };
      else if (payload.type === "entries") {
        const last = payload.rows.at(-1)?.entry.id ?? 0;
        view = { ...view, writers: view.writers.map((writer) => writer.id === payload.writer_id
          ? { ...writer, questions: payload.questions, last_id: Math.max(writer.last_id, last) } : writer) };
        rows = merged(rows, payload.rows.map((r) => ({ writer: payload.writer_id, ...r })));
      }
      else status = payload.text;
    });
    (async () => {
      try {
        await unlisten;
        const saved = await invoke<View>("snapshot");
        if (!active) return;
        view = saved;
        readerReady = true;
        pairing = saved.pairing;
        if (pairing) pairStep = "sending";
        relayMode=saved.network.disabled?'direct':saved.network.urls.length?'custom':'default';
        relayUrls=saved.network.urls.join('\n');
        if (!saved.writers.length) screen = "devices";
        await loadRows(true);
        view = await invoke<View>("resume");
        // Ask the browser to keep the saved entries when it needs space.
        navigator.storage?.persist?.().catch(() => {});
      } catch (e) { error = String(e); status = "Saved entries remain on this device."; }
    })();
    const timer = setInterval(() => { now = Date.now(); }, 1000);
    return () => { active = false; clearInterval(timer); unlisten.then((f) => f()); };
  });
</script>

{#snippet glyph(size: number)}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" aria-hidden="true">
    <path d="M6.5 4h11A3.5 3.5 0 0 1 21 7.5v7a3.5 3.5 0 0 1-3.5 3.5h-.7l1.4 3.3L14 18H6.5A3.5 3.5 0 0 1 3 14.5v-7A3.5 3.5 0 0 1 6.5 4z" />
    <circle cx="9" cy="11" r="1.15" fill="currentColor" stroke="none" />
    <circle cx="15" cy="11" r="1.15" fill="currentColor" stroke="none" />
  </svg>
{/snippet}

<svelte:head><title>Yappr</title><meta name="description" content="Updates and questions from your machines in one feed." /></svelte:head>

<main>
  <header class="topbar"><a class="brand" href="{base}/" aria-label="Yappr home">{@render glyph(28)} Yappr</a></header>
  <div class="app-tabs" role="tablist" aria-label="App views">
    <button role="tab" id="feed-tab" aria-controls="feed-panel" aria-selected={screen==='feed'} tabindex={screen==='feed'?0:-1} onclick={()=>screen='feed'} onkeydown={switchTab}>Feed</button>
    <button role="tab" id="devices-tab" aria-controls="devices-panel" aria-selected={screen==='devices'} tabindex={screen==='devices'?0:-1} onclick={()=>screen='devices'} onkeydown={switchTab}>Setup</button>
  </div>
  {#if error}<p class="error global-error" role="alert">{error}</p>{/if}
  <div id="devices-panel" role="tabpanel" tabindex="0" aria-labelledby="devices-tab" hidden={screen!=='devices'}>
    <div class="devices-intro"><h1>{hasConnections?'Machines':'Connect your computer'}</h1><p class="intro">{hasConnections?'Pause a machine, or connect another one.':'Pair this device with a computer that runs Yappr.'}</p></div>
    {#if view.writers.length}
      <section class="connected-devices" aria-label="Connected machines">
        {#each view.writers as writer(writer.id)}
          <div class="connected-device"><div><strong>{writer.name}</strong><p>{writerLabel(writer)}</p>
            {#if writer.enabled && !writer.delivering && writer.health.state === "offline"}<p class="offline-hint">This device cannot reach it. Run <code>yappr start</code> on that machine.</p>{/if}</div>
            <div class="device-actions"><button onclick={()=>toggleWriter(writer)} aria-label={`${writer.enabled?'Pause':'Resume'} ${writer.name}`}>{writer.enabled?'Pause':'Resume'}</button></div></div>
        {/each}
      </section>
    {/if}
    <section class="connection-panel">
      <div class="connection-content">
        <h2>Connect a computer</h2><p class="hint">Get its updates and answer its questions on this device.</p>
        {#if pairing || pairStep!=='idle'}
          <div class="steps" aria-live="polite">
            <p class="step done"><span class="mark">✓</span>Code read</p>
            <p class="step">{pairStep==='approved'?'Connected':pairStep==='retrying'?'Cannot reach the computer yet. Retrying…':pairStep==='waiting'?'Approve this device in the computer’s terminal.':'Sending your request…'}</p>
          </div>
          {#if pairStep!=='approved'}<code class="identity">{shortId(view.device_id)}</code><p class="hint">Compare this ID with the computer's terminal.</p><button class="quiet" onclick={async()=>{await invoke('stop');pairing=false;pairStep='idle';}}>Use another code</button>{/if}
        {:else if preview}
          <p class="step-label">3. Confirm the {preview.machine?'machine':'computer'}</p><p class="hint">{preview.name} · <code title={preview.writer_id}>{shortId(preview.writer_id)}</code></p>
          {#if preview.machine}<label for="machine-name">Machine name</label><input id="machine-name" bind:value={machineName} maxlength="80"/><p class="hint">This machine can send entries and questions to this device. It cannot read anything here.</p>
          {:else if preview.ticket}<label for="machine-name">Computer name</label><input id="machine-name" bind:value={machineName} maxlength="80"/>
          {:else}<label for="device-name">Your device name</label><input id="device-name" bind:value={name} maxlength="64"/><p class="hint">The computer must approve your request.{#if preview.expires_at} This code expires {when(preview.expires_at)}.{/if}</p>{/if}
          <div class="confirm-actions"><button class="option" onclick={requestPair} disabled={preview.machine||preview.ticket?!machineName.trim():!name.trim()}>{preview.machine?'Allow machine':preview.ticket?'Follow this feed':'Request pairing'}</button><button class="quiet" onclick={()=>preview=null}>Use another code</button></div>
        {:else}
          {#if followSuccess}<p class="connection-success" role="status">Connected to {followSuccess}.</p>{/if}
          <p class="step-label">1. On your computer</p><pre class="setup-command">yappr start --pair</pre>
          <p class="hint">It shows a pairing code. Keep it running.</p><p class="step-label">2. Read its code</p>
          <CodeInput oncode={readCode} disabled={!readerReady} label="Pairing code, machine code, or feed ticket" imageLabel="Import a pairing QR image"/>
        {/if}
      </div>
    </section>
    {#if hasConnections || hasHistory}<button class="view-feed" onclick={()=>screen='feed'}>View feed <span aria-hidden="true">→</span></button>{/if}
    <details class="panel device-panel"><summary>Advanced settings</summary>
      <details><summary>This device</summary>
      <code class="identity">{view.device_id || "Reading identity…"}</code>
      <button class="quiet" onclick={copyId}>{copied ? "Copied" : "Copy device ID"}</button></details>
      <details>
        <summary>Machines <span class="count">{view.writers.length}</span></summary>
        {#each view.writers as writer (writer.id)}
          <div class="machine-settings">
            <label for={`name-${writer.id}`}>Machine name</label>
            <div class="machine-control"><input id={`name-${writer.id}`} value={writer.name} maxlength="80" onchange={(event) => renameWriter(writer, event)} />
            </div>
            <p class="hint">{healthLabel(writer.health)}</p>
            {#if writer.health.address}<code class="identity">{writer.health.address}</code>{/if}
            {#if writer.health.error}<p class="hint">{writer.health.error}</p>{/if}
            {#if writer.health.retry_in_ms}<p class="hint">Retry delay: {writer.health.retry_in_ms / 1000}s</p>{/if}
            <code class="identity">{writer.id}</code>
          </div>
        {/each}
      </details>
      <details>
        <summary>Relays</summary>
        <label for="relay-mode">Connection settings</label>
        <select id="relay-mode" bind:value={relayMode}>
          <option value="default">Default relays</option><option value="custom">Custom relays</option><option value="direct">Direct only</option>
        </select>
        {#if relayMode === "custom"}<label for="relay-urls">Relay URLs, one per line</label><textarea id="relay-urls" bind:value={relayUrls} rows="3" placeholder="https://relay.example.com" spellcheck="false" autocapitalize="off" autocorrect="off"></textarea>{/if}
        <p class="hint">Changes reconnect your machines. Direct mode requires a reachable direct path.</p>
        <button class="quiet" disabled={savingNetwork || view.pairing || (relayMode === "custom" && !relayUrls.trim())} onclick={saveNetwork}>{savingNetwork ? "Reconnecting…" : "Save settings"}</button>
      </details>
    </details>
  </div>
  {#if screen==='feed'}
    <div role="tabpanel" tabindex="0" id="feed-panel" aria-labelledby="feed-tab">
    {#if !readerReady}
      <section class="empty setup-empty"><h2>Starting Yappr…</h2></section>
    {:else if !hasHistory}
      <section class="empty setup-empty"><h1>{hasConnections?'Ready for updates':'Connect your first computer'}</h1><p>{hasConnections?'Updates appear here when your computer sends them.':'Bring updates and questions from your machines into one feed.'}</p><button class="option" onclick={()=>screen='devices'}>{hasConnections?'Open setup':'Connect a device'}</button></section>
    {:else}
    <section class="feed-heading">
      <select class="machine-select" aria-label="Select machine" value={selectedMachine} onchange={selectWriter}>
        <option value="">All machines</option>{#each machines as writer (writer.id)}<option value={writer.id}>{writer.name}</option>{/each}
      </select>
      <button class="connection-status" onclick={openDevice} aria-label={`Connection: ${connectionLabel}. Open setup`}><span class:online={allConnected} class="status-dot"></span>{connectionLabel}</button>
    </section>
    <nav aria-label="Feed views">
      <button class:active={tab === "all"} aria-current={tab === "all" ? "page" : undefined} onclick={() => tab = "all"}>All</button>
      <button class:active={tab === "open"} aria-current={tab === "open" ? "page" : undefined} onclick={() => tab = "open"}
        aria-label={`Open, ${pending} ${pending === 1 ? "question" : "questions"}`}>Open {#if pending}<span class="pill">{pending}</span>{/if}</button>
    </nav>

    <section class="feed-list" aria-label={tab === "open" ? "Open questions" : "All entries"}>
      {#if !visibleFeed.length}
        <section class="empty"><h2>{tab === "open" ? "All caught up" : "No entries yet"}</h2><p>{tab === "open" ? "No questions need an answer." : "Updates and questions from your machines appear here."}</p></section>
      {/if}
      {#each visibleFeed as item (item.key)}
        {@const entry = item.entry}
        {@const q = item.question}
        {@const state = q ? stateOf(q) : ""}
        {@const delivery = q ? deliveryFor(q, item.writer) : undefined}
        {@const isExpanded = expanded === item.key}
        {@const sendKey = q ? `${item.writer.id}:${q.question.id}` : ""}
        {@const label = entry.title || entry.source}
        {@const level = levelOf(entry)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
        <article class="feed-row level-{level}" class:expanded={isExpanded} class:resolved={q && state !== "pending"} onclick={(event) => rowClick(event, item.key)}>
          <div class="row-head" role="button" tabindex="0" aria-expanded={isExpanded} aria-controls={`entry-${item.key}`}
            aria-label={`${label}. ${isExpanded ? "Hide details" : "Show full text and details"}`} onkeydown={(event) => headKey(event, item.key)}>
            <div class="row-meta">
              <span class="row-labels">
                {#if !selectedMachine && machines.length > 1}<span class="writer-label">{item.writer.name}</span>{/if}
                {#if level !== "info"}<span class="level"><span class="level-dot" aria-hidden="true"></span>{level}</span>{/if}
                <span class="source">{entry.source}</span>
                <span>{q ? (q.question.approval ? "approval" : "question") : kindOf(entry)} #{entry.id}</span>
                {#if q && state !== "pending" && state !== "answered" && state !== "expired"}<span class="entry-state">{state}</span>{/if}
              </span>
              <time datetime={entry.time}>{feedTime(entry.time)}</time>
            </div>
            {#if entry.title}<p class="title">{entry.title}</p>{/if}
          </div>
          <div class="row-content" id={`entry-${item.key}`}>
            {#if entry.body && (isExpanded || !q || state !== "answered")}
              {#if entry.format === "markdown"}
                <div class="body markdown" class:clamped={!isExpanded}>{@html renderMarkdown(entry.body)}</div>
              {:else}
                <pre class="body" class:clamped={!isExpanded} class:titled={!!entry.title}>{#each linkSegments(entry.body) as part}{#if part.href}<a href={part.href} target="_blank" rel="noopener noreferrer">{part.text}</a>{:else}{part.text}{/if}{/each}</pre>
              {/if}
            {/if}
            {#if q}
              <div class="row-foot">
                {#if state === "answered" && q.decision}
                  <p class="decision {tone(q, q.decision.value)}" title="Confirmed by the writer"><span class="dot"></span>{outcome(q, q.decision.value)} · confirmed</p>
                {:else if state === "pending" && delivery && delivery.status !== "rejected"}
                  <p class="decision {tone(q, delivery.submission.value)}"><span class="dot"></span>{outcome(q, delivery.submission.value)} <span>· {delivery.status === "queued" ? "queued, waiting for the writer" : "confirmed by the writer"}</span></p>
                {:else if state === "pending" && sending[sendKey]}
                  <p class="decision {tone(q, sending[sendKey].value)}" role="status"><span class="dot"></span>{sending[sendKey].label} <span>· sending in {Math.max(1, Math.ceil((sending[sendKey].at - now) / 1000))} s</span>
                    <button class="undo" onclick={() => undoChoice(sendKey)}>Undo</button></p>
                {:else if state === "pending"}
                  {#if !item.writer.enabled}<span class="hint">Resume this machine to answer.</span>{/if}
                  {#if q.question.responder === view.device_id}
                    <div class="options">
                      {#each q.question.options as option}
                        <button class="option" class:approve={q.question.approval && option.value === "approve"} class:reject={q.question.approval && option.value === "reject"} disabled={busy === sendKey || !item.writer.enabled}
                          onclick={() => startChoice(item.writer.id, q.question.id, option.value, option.label)}>{option.label}</button>
                      {/each}
                    </div>
                  {:else}<span class="hint">For another device</span>{/if}
                {/if}
                {#if q.question.expires_at && (state === "pending" || state === "expired")}
                  <time class="expiry" datetime={q.question.expires_at} aria-label={`${state === "expired" ? "Expired" : "Expires"} ${when(q.question.expires_at)}`}>
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></svg>
                    {state === "expired" ? "Expired" : expiryTime(q.question.expires_at)}
                  </time>
                {/if}
              </div>
              {#if state === "pending" && delivery?.status === "rejected"}<p class="error">The writer rejected this submission. {delivery.error}</p>{/if}
            {/if}
            {#if isExpanded}
              <footer class="entry-footer" aria-label="Entry metadata">
                <div class="entry-context">
                  <span class="entry-machine" title={`Machine: ${item.writer.name} · ${item.writer.id}`}>{item.writer.name} <code>{shortId(item.writer.id)}</code></span>
                  {#if entry.sig}
                    <span class="verified" title="The machine's signature was checked">
                      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6z"/><path d="m8.5 12 2.5 2.5 4.5-5"/></svg>Signed
                    </span>
                  {/if}
                  <span class="entry-copies">
                    <button class="entry-copy" onclick={()=>copyText(item)} aria-label="Copy text" title="Copy the title and the text">
                      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/></svg>
                      {copiedEntry===`${item.key}:text`?'Copied':'Copy text'}
                    </button>
                    {#if entry.sig}
                      <button class="entry-copy" onclick={()=>copySigned(item)} aria-label="Copy signed entry" title="Copy the signed entry as JSON, for programs that check the signature">
                        {copiedEntry===`${item.key}:signed`?'Copied':'Copy signed'}
                      </button>
                    {/if}
                  </span>
                </div>
                <dl class="entry-details">
                  {#if q}
                    <div><dt>{state === "answered" ? "Answered by" : "Assigned to"}</dt><dd>{responderFor(q) === view.device_id ? "You" : "Device"} <code title={responderFor(q)}>{shortId(responderFor(q))}</code></dd></div>
                  {/if}
                  {#if entry.subject}<div><dt>Subject</dt><dd>{entry.subject}</dd></div>{/if}
                  <div><dt>Created</dt><dd><time datetime={entry.time}>{detailTime(entry.time)}</time></dd></div>
                  {#if q?.question.expires_at}<div><dt>{state === "expired" ? "Expired" : "Expires"}</dt><dd><time datetime={q.question.expires_at}>{detailTime(q.question.expires_at)}</time></dd></div>{/if}
                  {#if q}<div><dt>Question</dt><dd><code title={q.question.id}>{shortId(q.question.id)}</code></dd></div>{/if}
                </dl>
                {#if entry.data && Object.keys(entry.data).length}<pre class="entry-data" aria-label="Entry data">{JSON.stringify(entry.data,null,1)}</pre>{/if}
              </footer>
            {/if}
          </div>
        </article>
      {/each}
      {#if older}<button class="older" onclick={() => loadRows(false)} disabled={loading}>{loading ? "Loading…" : "Show older entries"}</button>{/if}
    </section>
    {/if}
    </div>
  {/if}
</main>


<style>
  .connected-devices {margin:0 var(--gutter) 16px;}
  .connected-device {display:flex;align-items:center;justify-content:space-between;gap:12px;padding:12px 0;border-bottom:1px solid var(--rule);}
  .connected-device strong{font-size:13px;font-weight:500;overflow-wrap:anywhere;}.connected-device p{font-size:11px;color:var(--muted);margin:3px 0 0;}.device-actions{display:flex;gap:12px;flex:none;}.device-actions button{background:none;border:0;color:var(--muted);font-size:12px;min-height:36px;padding:4px 0;}
  .app-tabs {display:flex;gap:20px;padding:0 var(--gutter);border-bottom:1px solid var(--border);}
  .app-tabs button {min-height:44px;padding:8px 0;border:0;border-bottom:2px solid transparent;background:none;color:var(--muted);font-size:14px;}
  .app-tabs button[aria-selected="true"] {color:#edf0f5;border-bottom-color:#c4cbd6;}
  .devices-intro {padding:22px var(--gutter) 0;}.devices-intro h1{font-size:19px;}.devices-intro .intro{font-size:13px;margin-bottom:14px;}
  .connection-panel {margin:0 var(--gutter);border:1px solid var(--border);border-radius:9px;overflow:hidden;background:var(--surface);}
  .connection-content {padding:18px;}.connection-content h2{margin:0;font-size:15px;}.step-label{font-size:12px;color:#c2c8d1;margin:18px 0 8px;}
  .setup-command {margin:0;padding:10px;border:1px solid var(--border);border-radius:6px;background:#15171b;font:12px/1.6 ui-monospace,monospace;white-space:pre-wrap;overflow-wrap:anywhere;}
  .confirm-actions {display:flex;align-items:center;gap:16px;margin-top:10px;}.connection-success{font-size:12px;color:var(--good);margin:12px 0;}
  .older {display:block;width:calc(100% - 2 * var(--gutter));margin:14px var(--gutter);min-height:40px;border:1px solid var(--border);border-radius:7px;background:none;color:var(--muted);font-size:13px;}
  .view-feed {display:flex;justify-content:space-between;align-items:center;width:calc(100% - 2 * var(--gutter));margin:14px var(--gutter);padding:10px 12px;border:1px solid #3a3f49;border-radius:7px;background:#1d2026;color:#d0d4dd;font-size:13px;}
  .setup-empty {padding:32px var(--gutter);}.setup-empty p{margin:8px 0 18px;}.device-panel>summary{padding:0;min-height:24px;}.device-panel {margin-bottom:14px;}
  :global(:root) {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    font-size: 14px;
    line-height: 1.5;
    font-weight: 400;
    color-scheme: dark;
    color: #e0e3e8;
    background: #101114;
    font-synthesis: none;
    --surface: #17191d;
    --border: #2b2e35;
    --rule: #24272d;
    --muted: #a0a5af;
    --subtle: #9299a5;
    --good: #7fcf95;
    --gutter: 16px;
  }
  :global(body) { margin: 0; background: #101114; }
  :global(*) { box-sizing: border-box; }
  :global(button), :global(input), :global(textarea), :global(select) { font: inherit; }
  :global(button) { cursor: pointer; touch-action: manipulation; -webkit-tap-highlight-color: transparent; }
  :global(button:disabled) { cursor: default; opacity: .5; }
  :global(button:focus-visible), :global(summary:focus-visible), :global(a:focus-visible),
  :global(input:focus-visible), :global(textarea:focus-visible), :global(select:focus-visible) { outline: 2px solid #a4aad4; outline-offset: 3px; }
  main { max-width: 560px; padding: env(safe-area-inset-top) 0 calc(env(safe-area-inset-bottom) + 20px); margin: auto; min-height: 100dvh; }
  .topbar { display: flex; align-items: center; justify-content: space-between; min-height: 60px; padding: 0 var(--gutter); border-bottom: 1px solid var(--border); }
  .brand { display: inline-flex; align-items: center; gap: 12px; color: #e0e3e8; text-decoration: none; font-size: 18px; font-weight: 500; letter-spacing: -.2px; min-height: 44px; }
  .brand svg { color: #c9ced8; }
  h1 { font-size: 20px; line-height: 1.35; letter-spacing: -.35px; font-weight: 500; margin: 0 0 8px; }
  h2 { font-size: 15px; line-height: 1.45; letter-spacing: -.1px; margin: 8px 0; font-weight: 500; overflow-wrap: anywhere; }
  .eyebrow { font-size: 12px; font-weight: 400; letter-spacing: 0; margin: 0; color: var(--muted); }
  .panel { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 14px; margin: 12px var(--gutter) 0; }
  .connect { padding: 18px; }
  .connect-title { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; }
  .connect-title .quiet { margin-top: -10px; }
  .intro { color: var(--muted); font-size: 14px; line-height: 1.55; margin: 0 0 18px; }
  .primary, .secondary { display: flex; align-items: center; justify-content: space-between; gap: 8px; width: 100%; min-height: 44px; border-radius: 7px; padding: 10px 12px; text-align: left; font-size: 14px; font-weight: 400; line-height: 1.4; }
  .primary { background: #313440; border: 1px solid #474b5d; color: #e4e6f7; margin: 8px 0; }
  .primary span, .secondary span { color: var(--muted); }
  .secondary { background: #1d2026; color: #d0d4dd; border: 1px solid #343943; margin: 8px 0; }
  .quiet { border: 0; background: transparent; color: var(--muted); min-height: 44px; padding: 10px 0; font-size: 13px; }
  .file { display: none; }
  details { margin-top: 12px; font-size: 12px; }
  summary { cursor: pointer; color: var(--muted); min-height: 44px; padding: 12px 0; }
  label { display: block; color: var(--muted); font-size: 12px; margin: 14px 0 6px; }
  input, textarea, select { width: 100%; background: #121418; border: 1px solid #3a3f49; border-radius: 7px; padding: 10px; color: inherit; font-size: 16px; }
  textarea { resize: vertical; overflow-wrap: anywhere; }
  .connect-options { display: grid; gap: 12px; margin-top: 18px; }
  .choice { display: flex; align-items: center; gap: 16px; width: 100%; min-height: 56px; padding: 0 16px; border: 1px solid var(--border); border-radius: 8px; background: #15171b; color: #e0e3e8; font-size: 15px; text-align: left; }
  .choice svg { flex: none; color: #d6dae2; }
  .choice span:first-of-type { flex: 1; }
  .choice .chevron { flex: none; font-size: 22px; line-height: 1; color: var(--muted); transition: transform .15s ease; }
  .choice .chevron.open { transform: rotate(90deg); }
  .choice.plain { border-color: transparent; background: transparent; min-height: 44px; color: #c9ced7; font-size: 14px; }
  .terminal-note { margin-top: 20px; padding-top: 18px; border-top: 1px solid var(--border); }
  .terminal-note > span { display: block; font-size: 13px; color: var(--muted); margin-bottom: 10px; }
  .command { margin: 0; padding: 14px 16px; border: 1px solid var(--border); border-radius: 8px; background: #15171b; font-family: ui-monospace, SFMono-Regular, monospace; font-size: 14px; color: #e0e3e8; user-select: text; }
  .terminal-note .hint { margin-top: 12px; }
  code { font-family: ui-monospace, SFMono-Regular, monospace; font-size: 11px; line-height: 1.7; }
  .identity { display: block; overflow-wrap: anywhere; user-select: text; margin: 10px 0 0; color: var(--muted); }
  .hint { font-size: 12px; line-height: 1.55; color: var(--muted); overflow-wrap: anywhere; margin: 8px 0 0; }
  .count { display: inline-block; margin-left: 4px; color: var(--subtle); font-size: 11px; font-variant-numeric: tabular-nums; }
  .connect.awaiting { border-color: #5e5034; background: linear-gradient(180deg, #1e1c18, var(--surface) 70%); animation: await-glow 2.6s ease-in-out infinite; }
  .connect.approved { border-color: #3ccf7a; box-shadow: 0 0 0 1px #3ccf7a55, 0 0 28px #3ccf7a33; transition: box-shadow .3s ease, border-color .3s ease; }
  .steps { display: grid; gap: 12px; margin: 4px 0 18px; }
  .step { display: flex; align-items: center; gap: 10px; margin: 0; font-size: 14px; color: var(--muted); }
  .step .mark { width: 20px; height: 20px; flex: none; border-radius: 50%; border: 1.5px solid #3a3f4a; display: inline-flex; align-items: center; justify-content: center; font-size: 12px; font-weight: 700; line-height: 1; }
  .step.done, .step.active, .step.waiting { color: #e0e3e8; }
  .step.done .mark { background: #3ccf7a; border-color: #3ccf7a; color: #06210f; }
  .step.active .mark { border-color: #c7ae80; border-top-color: transparent; animation: spin .9s linear infinite; }
  .step.waiting .mark { border-color: #c7ae80; animation: ring-pulse 1.8s ease-in-out infinite; }
  .compare { font-size: 12px; color: var(--muted); margin: 0; }
  @keyframes await-glow { 0%, 100% { box-shadow: 0 0 0 1px #c7ae8018; } 50% { box-shadow: 0 0 0 1px #c7ae8050, 0 0 26px #c7ae801f; } }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes ring-pulse { 0%, 100% { box-shadow: 0 0 0 0 #c7ae8066; } 50% { box-shadow: 0 0 0 6px #c7ae8000; } }
  @media (prefers-reduced-motion: reduce) {
    .connect.awaiting, .step .mark { animation: none; }
  }
  .machine-control { display: flex; align-items: center; gap: 10px; }
  .machine-control .option { flex: none; white-space: nowrap; }
  .machine-control input { min-width: 0; font-size: 14px; }
  .machine-settings { padding: 8px 0 12px; border-bottom: 1px solid var(--border); }

  .feed-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px var(--gutter) 0; }
  .machine-select { width: auto; max-width: 65%; border: 0; background: transparent url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12' fill='none' stroke='%23d6dae2' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M2.5 4.5 6 8l3.5-3.5'/%3E%3C/svg%3E") no-repeat right 2px center;
    -webkit-appearance: none; appearance: none; padding: 6px 24px 6px 0; font-size: 15px; font-weight: 500; min-height: 40px; color: #e0e3e8; text-overflow: ellipsis; }
  .connection-status { background: transparent; border: 0; padding: 0; min-height: 40px; display: flex; gap: 8px; align-items: center; color: #d6dae2; font-size: 13px; }
  .status-dot { width: 8px; height: 8px; flex: none; display: inline-block; border-radius: 100%; background: #c7ae80; }
  .status-dot.online { background: var(--good); }
  nav { display: flex; align-items: flex-end; gap: 8px; padding: 0 var(--gutter); border-bottom: 1px solid var(--border); }
  nav button { display: inline-flex; align-items: center; gap: 8px; min-height: 44px; min-width: 64px; justify-content: center; padding: 8px 14px; margin-bottom: -1px;
    border: 0; border-bottom: 2px solid transparent; background: transparent; color: var(--muted); font-size: 15px; }
  nav .active { color: #e8eaee; border-bottom-color: #d6dae2; }
  .pill { display: inline-flex; align-items: center; justify-content: center; min-width: 20px; height: 20px; padding: 0 6px; border-radius: 10px; background: #2c2f37; color: #d6dae2; font-size: 11px; font-variant-numeric: tabular-nums; }

  .feed-row { padding: 12px var(--gutter) 14px; border-bottom: 1px solid var(--rule); cursor: pointer; }
  .feed-row.expanded { cursor: auto; background: #15171b; box-shadow: inset 2px 0 #8d93a1; }
  .row-head { cursor: pointer; border-radius: 4px; }
  .row-head:focus-visible { outline: 2px solid #a4aad4; outline-offset: 4px; }
  .row-meta { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; color: var(--muted); font-size: 12px; }
  .row-labels { display: flex; flex-wrap: wrap; align-items: baseline; min-width: 0; }
  .row-labels > span { overflow-wrap: anywhere; }
  .row-labels > span + span::before { content: "·"; margin: 0 7px; color: #6f7682; }
  .writer-label { color: #c3c8d1; }
  .entry-state { color: #bdad8b; }
  .row-meta time { flex: none; white-space: nowrap; font-variant-numeric: tabular-nums; }
  .title { margin: 3px 0 2px; font-size: 16px; line-height: 1.4; font-weight: 500; letter-spacing: -.1px; color: #eceef2; overflow-wrap: anywhere; }
  .resolved .title { color: #d3d7de; }
  .body { white-space: pre-wrap; overflow-wrap: anywhere; font: inherit; font-size: 14px; line-height: 1.55; color: #aeb4be; margin: 2px 0 0; user-select: text; }
  .body.clamped { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 4; line-clamp: 4; overflow: hidden; }
  .body.clamped.titled { -webkit-line-clamp: 2; line-clamp: 2; }
  .body a, .markdown :global(a) { color: #a9c1ea; text-decoration: underline; text-underline-offset: 2px; overflow-wrap: anywhere; }
  .markdown { white-space: normal; }
  .markdown.clamped { display: block; max-height: 4.65em; -webkit-mask-image: linear-gradient(180deg, #000 55%, transparent); mask-image: linear-gradient(180deg, #000 55%, transparent); }
  .markdown :global(:is(p, ul, ol, pre, blockquote, table)) { margin: 0 0 8px; }
  .markdown :global(:is(h1, h2, h3, h4)) { font-size: 14px; font-weight: 600; color: #dfe2e8; margin: 10px 0 4px; }
  .markdown :global(:is(ul, ol)) { padding-left: 20px; }
  .markdown :global(code) { font-size: 12px; background: #1d2026; border-radius: 4px; padding: 1px 4px; }
  .markdown :global(pre) { background: #15171b; border: 1px solid var(--border); border-radius: 6px; padding: 8px 10px; overflow-x: auto; }
  .markdown :global(pre code) { background: none; padding: 0; }
  .markdown :global(blockquote) { border-left: 2px solid #3a3e47; padding-left: 10px; color: var(--muted); }
  .markdown :global(table) { border-collapse: collapse; display: block; overflow-x: auto; }
  .markdown :global(:is(th, td)) { border: 1px solid var(--border); padding: 3px 8px; text-align: left; }
  .level { display: inline-flex; align-items: center; gap: 5px; font-weight: 500; }
  .level-dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
  .level-warn .level { color: #e2b45c; }
  .level-alert .level { color: #f0846a; }
  .feed-row.level-warn { box-shadow: inset 2px 0 #b98d3b; }
  .feed-row.level-alert { box-shadow: inset 2px 0 #d9654b; }
  .row-foot { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; margin-top: 10px; }
  .row-foot:empty { display: none; }
  .options { display: flex; flex-wrap: wrap; gap: 10px; }
  .option { width: auto; max-width: 100%; min-height: 36px; border-radius: 6px; padding: 6px 18px; font-size: 13px; line-height: 1.4; overflow-wrap: anywhere; background: transparent; border: 1px solid #4a4f5a; color: #e0e3e8; }
  .option.reject { border-color: #6a4249; color: #efc9c9; }
  .expiry { display: inline-flex; align-items: center; gap: 6px; margin-left: auto; color: #c3c8d1; font-size: 13px; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .decision { display: flex; flex-wrap: wrap; align-items: center; gap: 0 4px; font-size: 14px; color: var(--good); margin: 0; }
  .decision .dot { width: 9px; height: 9px; border-radius: 50%; background: currentColor; margin-right: 6px; }
  .decision.bad { color: #e49c9c; }
  .decision.neutral { color: #d6dae2; }
  .decision > span:not(.dot) { color: var(--muted); font-size: 13px; }
  .undo { margin-left: 8px; border: 0; background: transparent; color: #e0e3e8; font-size: 13px; font-weight: 600; text-decoration: underline; min-height: 36px; padding: 0 4px; }
  .entry-footer { margin-top: 12px; padding-top: 8px; border-top: 1px solid var(--rule); color: var(--subtle); font-size: 11px; line-height: 1.6; user-select: text; }
  .entry-context { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; min-height: 28px; }
  .entry-machine { display: inline-flex; flex-wrap: wrap; align-items: baseline; gap: 6px; min-width: 0; overflow-wrap: anywhere; color: #aeb4be; }
  .entry-machine code { color: var(--subtle); }
  .verified { display: inline-flex; align-items: center; gap: 4px; white-space: nowrap; }
  .verified svg { color: #91ad99; }
  .entry-copies { display: inline-flex; gap: 12px; margin-left: auto; }
  .offline-hint { margin-top: 4px !important; }
  .offline-hint code { font-size: 11px; }
  .entry-copy { display: inline-flex; align-items: center; gap: 5px; min-height: 28px; padding: 3px 0 3px 6px; border: 0; border-radius: 4px; background: transparent; color: var(--subtle); font-size: 11px; }
  .entry-copy:hover { color: #e0e3e8; }
  .entry-details { display: flex; flex-wrap: wrap; gap: 3px 14px; margin: 2px 0 0; font-size: 11px; }
  .entry-details > div { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0 5px; min-width: 0; max-width: 100%; }
  .entry-details dt { color: var(--subtle); }
  .entry-details dd { display: inline-flex; flex-wrap: wrap; gap: 0 4px; min-width: 0; margin: 0; color: #aeb4be; overflow-wrap: anywhere; }
  .entry-details code, .entry-context code { font-size: 10px; line-height: inherit; }
  .entry-details time { color: inherit; font-size: inherit; white-space: nowrap; }
  .entry-data { margin: 8px 0 0; padding: 8px 10px; max-height: 12em; overflow: auto; background: #111318; border: 1px solid var(--rule); border-radius: 5px; font: 11px/1.6 ui-monospace, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
  .empty { padding: 32px var(--gutter); }
  .empty h2 { font-size: 15px; margin: 0 0 5px; color: #c1c6d0; }
  .empty p { font-size: 13px; color: var(--muted); margin: 0; }
  time { color: var(--muted); font-size: 12px; }
  .error { color: #e0aaaa; font-size: 12px; line-height: 1.55; overflow-wrap: anywhere; }
  .global-error { margin: 12px var(--gutter); padding: 12px; background: #291e22; border: 1px solid #51363e; border-radius: 8px; }
  @media (hover: hover) {
    .option:hover, .secondary:hover { background: #262a31; }
    .primary:hover { background: #3b3f4d; }
    .choice:hover { background: #1b1e23; }
    .quiet:hover, nav button:hover { color: #e0e3e8; }
    .feed-row:not(.expanded):hover { background: #14161a; }
  }
  @media (min-width: 640px) {
    :global(:root) { --gutter: 24px; background: #0c0d10; }
    :global(body) { background: #0c0d10; }
    main { margin: 32px auto; min-height: 0; padding: 0 0 8px; background: #101114; border: 1px solid var(--border); border-radius: 14px; overflow: hidden; }
  }
  @media (prefers-contrast: more) {
    :global(:root) { --border: #626976; --rule: #4a505b; --muted: #c4cad4; --subtle: #b5bdc9; }
    .body { color: #d9dde4; }
  }
</style>
