// Model lists and credentials stay in page memory only.
(() => {
  const pickers = {};
  for (const prefix of ["asr", "text"]) {
    const base = $(prefix + "_base"), key = $(prefix + "_key");
    const value = $(prefix + "_model"), select = $(prefix + "-select");
    const search = $(prefix + "-search"), manual = $(prefix + "-manual");
    const button = $(prefix + "-fetch"), status = $(prefix + "-status");
    const testButton = $(prefix + "-test"), testStatus = $(prefix + "-test-status");
    let testRevision = 0, testing = false, tested = false;
    function resetTest() {
      testRevision++;
      testStatus.className = "connection-status";
      if (tested || testing) testStatus.textContent = "设置已更改，请重新测试。";
      tested = false;
    }
    testButton.onclick = async () => {
      if (testing) return;
      testStatus.className = "connection-status";
      if (!base.value.trim() || !key.value.trim() || !value.value.trim()) {
        testStatus.textContent = "请先填写接口地址、API Key 并选择模型。";
        testStatus.classList.add("failed");
        return;
      }
      const snapshot = ++testRevision;
      testing = true;
      testButton.disabled = true;
      testButton.textContent = "测试中…";
      testStatus.textContent = prefix === "asr" ? "正在用内置短音频测试所选语音模型…" : "正在向所选文本模型发送测试消息…";
      try {
        const response = await api("/api/test-connection", {
          method: "POST", headers: {"Content-Type":"application/json"},
          body: JSON.stringify({kind:prefix,base:base.value.trim(),key:key.value.trim(),model:value.value.trim(),
            language:prefix==="asr"?$("language").value:"",verbose:prefix==="asr"&&$("verbose").checked,
            protocol:prefix==="asr"?$("asr_protocol").value:""})
        });
        const result = await response.json();
        if (snapshot !== testRevision) return;
        tested = true;
        testStatus.textContent = result.message + " 耗时 " + (result.elapsed_ms/1000).toFixed(1) + " 秒。";
        testStatus.classList.add("success");
      } catch(error) {
        if (snapshot !== testRevision) return;
        tested = true;
        testStatus.textContent = error.message;
        testStatus.classList.add("failed");
      } finally {
        testing = false;
        testButton.disabled = prefix === "asr" && !!window.asrProfileError;
        testButton.textContent = "测试连接";
      }
    };
    value.addEventListener("input", resetTest);
    select.addEventListener("change", resetTest);
    if (prefix === "asr") for (const id of ["language","verbose"]) $(id).addEventListener("change", resetTest);
    let profileRevision = 0, profileTimer = null;
    function updateProfile() {
      if (prefix !== "asr") return;
      const version = ++profileRevision;
      clearTimeout(profileTimer);
      window.asrProfileError = "正在确认语音接口类型，请稍候。";
      testButton.disabled = true;
      $("asr-protocol-hint").textContent = "正在匹配语音接口…";
      profileTimer = setTimeout(async () => {
        try {
          const response = await api("/api/asr-profile", {method:"POST",headers:{"Content-Type":"application/json"},
            body:JSON.stringify({base:base.value.trim(),model:value.value.trim(),protocol:$("asr_protocol").value})});
          const profile = await response.json();
          if (version !== profileRevision) return;
          $("asr-protocol-hint").textContent = profile.label + " · " + profile.hint;
          $("asr-protocol-hint").classList.remove("failed");
          $("verbose").disabled = !profile.timestamps;
          if (!profile.timestamps && $("verbose").checked) { $("verbose").checked = false; resetTest(); }
          window.asrProfileError = null;
        } catch(error) {
          if (version !== profileRevision) return;
          window.asrProfileError = error.message;
          $("asr-protocol-hint").textContent = error.message;
          $("asr-protocol-hint").classList.add("failed");
        } finally {
          if (version === profileRevision) testButton.disabled = testing || !!window.asrProfileError;
        }
      }, 200);
    }
    if (prefix === "asr") {
      $("asr_protocol").addEventListener("change", () => { resetTest(); updateProfile(); });
      value.addEventListener("input", updateProfile);
      select.addEventListener("change", updateProfile);
    }
    let models = [], controller = null, revision = 0, manualMode = false;
    const setStatus = (message, failed = false) => {
      status.textContent = message;
      status.classList.toggle("failed", failed);
    };
    function render() {
      const query = search.value.trim().toLowerCase();
      const visible = models.filter(id => id.toLowerCase().includes(query));
      select.replaceChildren(new Option(models.length ? "选择模型" : "请先获取模型，或手动填写", ""));
      // Keep an explicit prior/manual choice without claiming it is in the fetched catalog.
      if (value.value && !visible.includes(value.value)) {
        select.add(new Option(value.value + (models.includes(value.value) ? "（当前选择）" : "（手动 / 上次选择）"), "selected:" + value.value));
      }
      for (const id of visible) select.add(new Option(id, "model:" + id));
      select.add(new Option("手动填写模型 ID…", "manual"));
      select.value = manualMode ? "manual" : (value.value ? (visible.includes(value.value) ? "model:" : "selected:") + value.value : "");
      manual.classList.toggle("hidden", !manualMode);
      search.classList.toggle("hidden", models.length < 9);
    }
    function invalidate() {
      resetTest();
      updateProfile();
      revision++;
      controller?.abort();
      controller = null;
      models = [];
      search.value = "";
      button.disabled = false;
      button.textContent = "获取模型";
      setStatus("接口或 Key 已更改，请重新获取模型。");
      render();
    }
    async function fetchModels() {
      if (controller) return;
      if (!base.value.trim() || !key.value.trim()) {
        setStatus("请先填写接口地址和 API Key。", true);
        return;
      }
      if (!base.checkValidity()) { setStatus("请填写有效的接口地址。", true); return; }
      const requestRevision = ++revision;
      controller = new AbortController();
      button.disabled = true;
      button.textContent = "获取中…";
      setStatus("正在读取服务商的模型列表…");
      try {
        const response = await api("/api/models", {
          method: "POST", signal: controller.signal,
          headers: {"Content-Type": "application/json"},
          body: JSON.stringify({base: base.value.trim(), key: key.value.trim()})
        });
        const result = await response.json();
        if (revision !== requestRevision) return;
        models = result.models;
        search.value = "";
        render();
        setStatus(models.length ? "已获取 " + models.length + " 个模型，请选择。" : "服务商返回了空列表，可手动填写模型 ID。", !models.length);
      } catch (error) {
        if (revision !== requestRevision || error.name === "AbortError") return;
        models = [];
        render();
        setStatus(error.message, true);
      } finally {
        if (revision === requestRevision) {
          controller = null;
          button.disabled = false;
          button.textContent = "重新获取";
        }
      }
    }
    button.onclick = fetchModels;
    for (const input of [base, key]) {
      input.addEventListener("input", invalidate);
      input.addEventListener("change", () => { if (base.value.trim() && key.value.trim()) fetchModels(); });
    }
    select.onchange = () => {
      manualMode = select.value === "manual";
      if (!manualMode) value.value = select.value ? select.value.slice(select.value.indexOf(":") + 1) : "";
      manual.classList.toggle("hidden", !manualMode);
      if (manualMode) value.focus();
    };
    search.oninput = render;
    value.addEventListener("input", () => { manualMode = true; });
    render();
    updateProfile();
    pickers[prefix] = {invalidate, fetchModels};
  }
  $("copy-config").onclick = () => {
    $("text_base").value = $("asr_base").value;
    $("text_key").value = $("asr_key").value;
    pickers.text.invalidate();
    if ($("text_key").value.trim()) pickers.text.fetchModels();
  };
  document.querySelectorAll("[data-settings-tab]").forEach(button => {
    button.onclick = () => {
      for (const prefix of ["asr", "text"]) {
        const active = prefix === button.dataset.settingsTab;
        $(prefix + "-panel").classList.toggle("hidden", !active);
        $(prefix + "-tab").classList.toggle("selected", active);
        $(prefix + "-tab").setAttribute("aria-selected", String(active));
      }
    };
  });
})();
