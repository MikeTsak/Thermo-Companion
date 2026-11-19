const BRIDGE = "http://127.0.0.1:47315";

function createMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "thermo-print-selection",
      title: "Thermo Print selection",
      contexts: ["selection"]
    });
    chrome.contextMenus.create({
      id: "thermo-print-image",
      title: "Thermo Print image",
      contexts: ["image"]
    });
    chrome.contextMenus.create({
      id: "thermo-print-link",
      title: "Thermo Print link (QR)",
      contexts: ["link"]
    });
    chrome.contextMenus.create({
      id: "thermo-print-page",
      title: "Thermo Print page URL (QR)",
      contexts: ["page"]
    });
  });
}

chrome.runtime.onInstalled.addListener(createMenus);
chrome.runtime.onStartup.addListener(createMenus);

function notify(title, message) {
  chrome.notifications.create({
    type: "basic",
    iconUrl: "icon.png",
    title,
    message
  });
}

async function getSelectedText(tab, fallback) {
  if (tab && typeof tab.id === "number" && tab.id >= 0) {
    try {
      const results = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: () => window.getSelection()?.toString() || ""
      });
      const exact = results?.[0]?.result;
      if (exact && exact.trim().length > 0) {
        return exact;
      }
    } catch {
      // Restricted pages (chrome://, PDF viewer, etc.) fall back to info.selectionText.
    }
  }
  return fallback || "";
}

async function sendToPip(payload) {
  try {
    const res = await fetch(`${BRIDGE}/print`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload)
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok || !data.ok) {
      notify("Thermo Print failed", data.error || `HTTP ${res.status}`);
    }
  } catch {
    notify(
      "Thermo Companion isn't running",
      "Start Thermo Companion (tray app) so Pip can receive print jobs from Chrome."
    );
  }
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "thermo-print-selection") {
    const text = await getSelectedText(tab, info.selectionText);
    if (!text.trim()) {
      notify("Nothing selected", "Highlight some text and try again.");
      return;
    }
    await sendToPip({
      kind: "text",
      text,
      title: "",
      url: info.pageUrl || tab?.url || ""
    });
  } else if (info.menuItemId === "thermo-print-image") {
    await sendToPip({
      kind: "image",
      url: info.srcUrl || "",
      title: tab?.title || ""
    });
  } else if (info.menuItemId === "thermo-print-link") {
    await sendToPip({
      kind: "qr",
      url: info.linkUrl || "",
      title: info.linkText || "LINK"
    });
  } else if (info.menuItemId === "thermo-print-page") {
    await sendToPip({
      kind: "qr",
      url: info.pageUrl || tab?.url || "",
      title: tab?.title || "PAGE"
    });
  }
});
