(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const api = (path, opts = {}) =>
    fetch(path, { headers: { "content-type": "application/json" }, ...opts });
  const post = (path, body) =>
    api(path, { method: "POST", body: body ? JSON.stringify(body) : undefined });
  const del = (path) => api(path, { method: "DELETE" });

  async function refresh() {
    try {
      const res = await api("/api/manager/stations");
      const stations = await res.json();
      render(stations);
    } catch (e) {
      console.error(e);
    }
  }

  function render(stations) {
    $("#count").textContent = stations.length === 0
      ? "no stations"
      : `${stations.length} station${stations.length === 1 ? "" : "s"}`;
    const tbody = $("#stations tbody");
    tbody.innerHTML = "";
    $("#empty-hint").textContent = stations.length === 0
      ? "No stations configured. Use the form above to create one."
      : "";
    stations.sort((a, b) => a.id.localeCompare(b.id));
    for (const s of stations) {
      const guiUrl = `http://${location.hostname}:${s.http_port}/`;
      const tr = document.createElement("tr");
      const authCell = s.username
        ? `<code>${escape(s.username)}</code>${s.has_password ? " · 🔒" : ""}`
        : s.csms_url.startsWith("wss://") ? "<span class=\"muted\">TLS only</span>" : "<span class=\"muted\">—</span>";
      tr.innerHTML = `
        <td><span class="dot ${s.running ? "run" : ""}"></span>${s.running ? "running" : "stopped"}</td>
        <td><code>${escape(s.id)}</code></td>
        <td>${s.version}</td>
        <td>${s.running ? `<a href="${guiUrl}" target="_blank">${s.http_port} ↗</a>` : s.http_port}</td>
        <td><code class="muted">${escape(s.csms_url)}</code></td>
        <td>${authCell}</td>
        <td>${s.autostart ? "✓" : "—"}</td>
        <td class="actions">
          ${s.running
            ? `<button class="small" data-act="stop" data-id="${escape(s.id)}">Stop</button>`
            : `<button class="small accent" data-act="start" data-id="${escape(s.id)}">Start</button>`}
          <button class="small danger" data-act="delete" data-id="${escape(s.id)}">Delete</button>
        </td>
      `;
      tbody.appendChild(tr);
    }
  }
  function escape(s) { return String(s).replace(/[<>&"']/g, c => ({ "<":"&lt;", ">":"&gt;", "&":"&amp;", '"':"&quot;", "'":"&#39;" }[c])); }

  document.addEventListener("click", async (ev) => {
    const btn = ev.target.closest("button[data-act]");
    if (!btn) return;
    const id = btn.dataset.id;
    const act = btn.dataset.act;
    try {
      if (act === "start") await post(`/api/manager/stations/${encodeURIComponent(id)}/start`);
      else if (act === "stop") await post(`/api/manager/stations/${encodeURIComponent(id)}/stop`);
      else if (act === "delete") {
        if (!confirm(`Really delete station '${id}'?`)) return;
        await del(`/api/manager/stations/${encodeURIComponent(id)}`);
      }
      await refresh();
    } catch (e) { console.error(e); }
  });

  $("#new-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    $("#form-err").textContent = "";
    const f = ev.target;
    const body = {
      id: f.id.value.trim(),
      http_port: parseInt(f.http_port.value, 10),
      version: f.version.value,
      csms_url: f.csms_url.value.trim(),
      username: f.username.value.trim() || null,
      password: f.password.value || null,
      autostart: f.autostart.checked,
      start_now: f.start_now.checked,
    };
    const res = await post("/api/manager/stations", body);
    if (res.ok) {
      f.reset();
      // Re-apply defaults after reset.
      f.autostart.checked = true;
      f.start_now.checked = true;
      await refresh();
    } else {
      const t = await res.text();
      $("#form-err").textContent = "Error: " + t;
    }
  });

  refresh();
  setInterval(refresh, 2000);
})();
