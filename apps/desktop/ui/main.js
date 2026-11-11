import { getInvoke, getLanguage, initChrome, t } from "./chrome.js";

const form = document.querySelector("#service-form");
const input = document.querySelector("#service-url");
const submitButton = document.querySelector("#submit-button");
const statusText = document.querySelector("#status");
const versionText = document.querySelector("#version");
const invoke = getInvoke();
const savedServices = document.querySelector("#saved-services");
const serviceList = document.querySelector("#service-list");

let busy = false;
let checking = null;
let opening = false;

function setStatus(message, tone = "muted", pending = false) {
  statusText.replaceChildren();
  if (pending) statusText.append(spinner());
  statusText.append(document.createTextNode(message));
  statusText.dataset.tone = tone;
}

function spinner() {
  const indicator = document.createElement("span");
  indicator.className = "spinner";
  indicator.setAttribute("aria-hidden", "true");
  return indicator;
}

function showChecking() {
  setStatus(`${t("status.checking")} ${checking}`, "pending", true);
}

function setBusy(nextBusy) {
  busy = nextBusy;
  input.disabled = busy;
  submitButton.disabled = busy;
  submitButton.textContent = checking !== null || opening ? t("setup.opening") : t("setup.open");
  submitButton.setAttribute("aria-busy", String(checking !== null || opening));
  serviceList.querySelectorAll("button").forEach((button) => { button.disabled = busy; });
}

async function openService(url) {
  if (busy) return;
  checking = url;
  showChecking();
  setBusy(true);
  updateSelectedService();
  try {
    await invoke("set_service_url", { url, language: getLanguage() });
    checking = null;
    opening = true;
    setBusy(true);
    updateSelectedService();
    setStatus(t("status.opening"), "success", true);
  } catch (error) {
    checking = null;
    setStatus(`${t("status.checkFailed")} ${String(error)}`, "error");
    setBusy(false);
    updateSelectedService();
    input.focus();
  }
}

function updateSelectedService() {
  serviceList.querySelectorAll(".saved-service-open").forEach((button) => {
    button.querySelector(".spinner")?.remove();
    if (checking !== null && button.dataset.url === checking) button.prepend(spinner());
  });
}

async function renderServices() {
  const services = await invoke("get_services");
  serviceList.replaceChildren();
  savedServices.hidden = services.length === 0;
  for (const url of services) {
    const row = document.createElement("li");
    const open = document.createElement("button");
    open.type = "button";
    open.className = "saved-service-open";
    open.dataset.url = url;
    open.textContent = url;
    open.addEventListener("click", () => void openService(url));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "saved-service-remove";
    remove.textContent = t("setup.forget");
    remove.setAttribute("aria-label", `${t("setup.forget")}: ${url}`);
    remove.addEventListener("click", async () => {
      if (busy) return;
      setBusy(true);
      try {
        await invoke("forget_service", { url });
        await renderServices();
      } catch (error) {
        setStatus(String(error), "error");
      } finally {
        setBusy(false);
      }
    });
    row.append(open, remove);
    serviceList.append(row);
  }
  serviceList.querySelectorAll("button").forEach((button) => { button.disabled = busy; });
  updateSelectedService();
}

async function bootstrap() {
  await initChrome({
    onLanguageChange: () => {
      setBusy(busy);
      if (checking !== null) showChecking();
      else if (opening) setStatus(t("status.opening"), "success", true);
      if (invoke) void renderServices().catch((error) => setStatus(String(error), "error"));
      if (!statusText.textContent) {
        setStatus("");
      }
    },
  });
  setBusy(false);

  if (!invoke) {
    setStatus(t("status.runtimeUnavailable"), "error");
    submitButton.disabled = true;
    return;
  }

  try {
    const [version, savedUrl] = await Promise.all([
      invoke("get_app_version").catch(() => ""),
      invoke("get_service_url").catch(() => null),
    ]);
    if (version) {
      versionText.textContent = `v${version}`;
    }
    if (savedUrl) {
      input.value = savedUrl;
    }
    await renderServices();
  } catch (error) {
    setStatus(String(error), "error");
  }
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  await openService(input.value);
});

void bootstrap();
