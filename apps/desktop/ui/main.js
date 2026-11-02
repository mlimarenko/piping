import { getInvoke, getLanguage, initChrome, t } from "./chrome.js";

const form = document.querySelector("#service-form");
const input = document.querySelector("#service-url");
const submitButton = document.querySelector("#submit-button");
const statusText = document.querySelector("#status");
const versionText = document.querySelector("#version");
const invoke = getInvoke();

let busy = false;

function setStatus(message, tone = "muted") {
  statusText.textContent = message;
  statusText.dataset.tone = tone;
}

function setBusy(nextBusy) {
  busy = nextBusy;
  input.disabled = busy;
  submitButton.disabled = busy;
  submitButton.textContent = busy ? t("setup.opening") : t("setup.open");
}

async function bootstrap() {
  await initChrome({
    onLanguageChange: () => {
      setBusy(busy);
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
  } catch (error) {
    setStatus(String(error), "error");
  }
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  setStatus("");
  setBusy(true);

  try {
    await invoke("set_service_url", { url: input.value, language: getLanguage() });
    setStatus(t("status.opening"), "success");
  } catch (error) {
    setStatus(String(error), "error");
    setBusy(false);
    input.focus();
    input.select();
  }
});

void bootstrap();
