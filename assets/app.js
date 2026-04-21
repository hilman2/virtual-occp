(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];

  let state = null;

  const api = (path, opts = {}) =>
    fetch(path, { headers: { "content-type": "application/json" }, ...opts });

  const post = (path, body) =>
    api(path, { method: "POST", body: body ? JSON.stringify(body) : undefined });

  const del = (path) => api(path, { method: "DELETE" });

  // ---------- Rendering ----------

  function renderStation(s) {
    $("#pill-version").textContent = "OCPP " + s.version;
    const pill = $("#pill-status");
    pill.textContent = s.connected
      ? (s.boot_accepted ? "connected · boot ok" : "connected · boot pending")
      : "disconnected";
    pill.className = "pill " + (s.connected ? "connected" : "disconnected");
    $("#pill-id").textContent = s.id;
    $("#stn-id").textContent = s.id;
    $("#stn-vendor").textContent = `${s.vendor} / ${s.model}`;
    $("#stn-fw").textContent = s.firmware_version;
    $("#stn-serial").textContent = s.serial_number;
    $("#stn-csms").textContent = s.csms_url;
    $("#stn-hb").textContent = s.last_heartbeat ? new Date(s.last_heartbeat).toLocaleTimeString() : "—";
    const hbInp = $("#hb-int");
    if (document.activeElement !== hbInp) hbInp.value = s.heartbeat_interval_s;
  }

  function renderConnectors(s) {
    const root = $("#connectors");
    root.innerHTML = "";
    for (const c of s.connectors) {
      const stateCls = c.state.toLowerCase();
      const el = document.createElement("div");
      el.className = "connector";
      el.innerHTML = `
        <header>
          <h3>Connector ${c.id}</h3>
          <span class="state ${stateCls}">${c.state}</span>
        </header>
        <div class="meter">
          Meter: <b>${(c.meter_wh / 1000).toFixed(3)} kWh</b>
          · max ${c.max_power_w} W
          ${c.transaction_id ? `· Tx: <code>${c.transaction_id}</code>` : ""}
          ${c.current_tag ? `· Chip: <code>${c.current_tag}</code>` : ""}
        </div>
        <div class="row">
          <button data-act="plug" data-cid="${c.id}" ${c.state !== "Available" ? "disabled" : ""}>Plug in</button>
          <button data-act="unplug" data-cid="${c.id}" ${c.state === "Available" ? "disabled" : ""}>Unplug</button>
          <button data-act="stop" data-cid="${c.id}" ${!c.transaction_id ? "disabled" : ""}>Stop charging</button>
          <button data-act="meter" data-cid="${c.id}">MeterValues</button>
          <button data-act="fault" data-cid="${c.id}" data-faulted="${c.state !== "Faulted"}">
            ${c.state === "Faulted" ? "Clear fault" : "Set fault"}
          </button>
        </div>
        <div class="row tag-swipe-row">
          <select data-role="swipe-tag" data-cid="${c.id}">
            ${s.tags.map(t => `<option value="${t.id_tag}">${t.label} (${t.id_tag})</option>`).join("")}
          </select>
          <button class="accent" data-act="swipe" data-cid="${c.id}">Swipe RFID</button>
        </div>
      `;
      root.appendChild(el);
    }
  }

  function renderTags(s) {
    const root = $("#tags");
    root.innerHTML = "";
    for (const t of s.tags) {
      const el = document.createElement("span");
      el.className = "tag " + t.status.toLowerCase();
      el.innerHTML = `
        <code>${t.id_tag}</code>
        <span>${t.label}</span>
        <span class="status">${t.status}</span>
        <button data-act="rmtag" data-tag="${t.id_tag}">✕</button>
      `;
      root.appendChild(el);
    }
  }

  function renderHistory(s) {
    const tbody = $("#history tbody");
    tbody.innerHTML = "";
    const rows = [...s.history].reverse().slice(0, 20);
    for (const h of rows) {
      const tr = document.createElement("tr");
      const dur = Math.floor((new Date(h.ended) - new Date(h.started)) / 1000);
      tr.innerHTML = `
        <td>${new Date(h.ended).toLocaleString()}</td>
        <td>${h.connector_id}</td>
        <td><code>${h.id_tag || "—"}</code></td>
        <td>${h.wh_consumed}</td>
        <td>${Math.floor(dur / 60)}m ${dur % 60}s</td>
        <td><code>${h.transaction_id}</code></td>
      `;
      tbody.appendChild(tr);
    }
  }

  function appendLog(entry) {
    const log = $("#log");
    const line = document.createElement("div");
    const cls = entry.direction === "->" ? "out"
             : entry.direction === "<-" ? "in"
             : entry.direction;
    line.className = "log-line " + cls;
    const ts = new Date(entry.ts).toLocaleTimeString();
    const dir = entry.direction === "->" ? "→" : entry.direction === "<-" ? "←" : entry.direction.toUpperCase();
    const action = entry.action ? `[${entry.action}]` : "";
    line.innerHTML = `<span class="ts">${ts}</span><span class="dir">${dir}</span> <span class="action">${action}</span>${escape(entry.message)}`;
    log.appendChild(line);
    // Rundpuffer im DOM
    while (log.childElementCount > 800) log.removeChild(log.firstChild);
    if ($("#log-auto").checked) log.scrollTop = log.scrollHeight;
  }
  function escape(s) { return String(s).replace(/[<>&]/g, c => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;" }[c])); }

  function applySnapshot(s) {
    state = s;
    renderStation(s);
    renderConnectors(s);
    renderTags(s);
    renderHistory(s);
    // Populate the log only on first render.
    const log = $("#log");
    if (log.childElementCount === 0 && s.log) s.log.forEach(appendLog);
  }

  // ---------- Events ----------

  document.addEventListener("click", async (ev) => {
    const btn = ev.target.closest("button[data-act]");
    if (!btn) return;
    const act = btn.dataset.act;
    const cid = btn.dataset.cid ? parseInt(btn.dataset.cid, 10) : undefined;
    try {
      if (act === "plug") await post("/api/plug", { connector_id: cid });
      else if (act === "unplug") await post("/api/unplug", { connector_id: cid });
      else if (act === "stop") await post("/api/stop", { connector_id: cid });
      else if (act === "meter") await post("/api/meter", { connector_id: cid });
      else if (act === "fault") {
        const faulted = btn.dataset.faulted === "true";
        await post("/api/fault", { connector_id: cid, faulted });
      }
      else if (act === "swipe") {
        const sel = $(`select[data-role="swipe-tag"][data-cid="${cid}"]`);
        if (!sel || !sel.value) return;
        await post("/api/swipe", { connector_id: cid, id_tag: sel.value });
      }
      else if (act === "boot") await post("/api/boot");
      else if (act === "reconnect") await post("/api/reconnect");
      else if (act === "hb") {
        const s = parseInt($("#hb-int").value, 10);
        await post("/api/heartbeat_interval", { seconds: s });
      }
      else if (act === "addtag") {
        const id_tag = $("#new-tag-id").value.trim();
        const label = $("#new-tag-label").value.trim() || id_tag;
        const status = $("#new-tag-status").value;
        if (!id_tag) return;
        await post("/api/tags", { id_tag, label, status });
        $("#new-tag-id").value = "";
        $("#new-tag-label").value = "";
      }
      else if (act === "rmtag") {
        await del("/api/tags/" + encodeURIComponent(btn.dataset.tag));
      }
      else if (act === "clearlog") {
        $("#log").innerHTML = "";
      }
    } catch (e) {
      console.error(e);
    }
  });

  // ---------- SSE ----------

  function connectSSE() {
    const src = new EventSource("/api/events");
    src.addEventListener("snapshot", (e) => {
      const ev = JSON.parse(e.data);
      if (ev.state) applySnapshot(ev.state);
    });
    src.addEventListener("log", (e) => {
      const ev = JSON.parse(e.data);
      if (ev.entry) appendLog(ev.entry);
    });
    src.onerror = () => {
      // Browser reconnectet automatisch.
    };
  }

  connectSSE();
})();
