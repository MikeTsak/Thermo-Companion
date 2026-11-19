const BRIDGE = "http://127.0.0.1:47315";
const statusEl = document.getElementById("status");
const qrBtn = document.getElementById("qr-btn");

async function checkHealth() {
  try {
    const res = await fetch(`${BRIDGE}/health`);
    const data = await res.json();
    if (res.ok && data.ok) {
      statusEl.textContent = `connected: Pip v${data.version || ""}`;
      statusEl.className = "status ok";
      qrBtn.disabled = false;
      return;
    }
  } catch {
    // ignore
  }
  statusEl.textContent = "Pip is offline (start Thermo Companion)";
  statusEl.className = "status err";
  qrBtn.disabled = true;
}

qrBtn.addEventListener("click", async () => {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.url) return;
  qrBtn.disabled = true;
  try {
    const res = await fetch(`${BRIDGE}/print`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ kind: "qr", url: tab.url, title: tab.title || "PAGE" })
    });
    const data = await res.json().catch(() => ({}));
    statusEl.textContent = res.ok && data.ok ? "printed page QR!" : (data.error || "print failed");
    statusEl.className = res.ok && data.ok ? "status ok" : "status err";
  } catch {
    statusEl.textContent = "failed to reach Pip";
    statusEl.className = "status err";
  } finally {
    qrBtn.disabled = false;
  }
});

checkHealth();
