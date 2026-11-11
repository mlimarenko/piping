use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::menu::MenuBuilder;
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, LogicalPosition, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    Window,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

const DESKTOP_CONFIG_FILE: &str = "app.json";
const LEGACY_DESKTOP_CONFIG_FILE: &str = "desktop-config.json";
const DESKTOP_SERVICE_URL_SEED_FILE: &str = "desktop-service-url.txt";
const CONFIG_DIR_NAME: &str = "piping";
const CONFIG_DIR_ENV: &str = "PIPING_DESKTOP_CONFIG_DIR";
const SERVICE_URL_ENV: &str = "PIPING_DESKTOP_SERVICE_URL";
const SERVICE_URL_ARG_NAMES: [&str; 2] = ["--service-url", "--piping-service-url"];
const BOOTSTRAP_WINDOW_LABEL: &str = "bootstrap";
const MAIN_WINDOW_LABEL: &str = "main";
const TOOLBAR_HEIGHT_LOGICAL: f64 = 28.0;
const SERVICE_WINDOW_TRANSITION_DELAY_MS: u64 = 75;
const DETACHED_WINDOW_REVEAL_FALLBACK_MS: u64 = 6_000;
const SERVICE_URL_CONNECT_TIMEOUT_MS: u64 = 2_500;
const SERVICE_URL_REQUEST_TIMEOUT_MS: u64 = 4_000;
const SERVICE_HEALTH_PATH: &str = "/api/v1/health";
const SERVICE_AUTH_PROVIDERS_PATH: &str = "/api/v1/auth/providers";
const SERVICE_PROBE_BODY_MAX_BYTES: u64 = 16 * 1024;
const WINDOW_TITLE_MAX_CHARS: usize = 120;
const MENU_OPEN_WEB_CONSOLE_ID: &str = "desktop-menu-open-web-console";
const MENU_CHECK_UPDATES_ID: &str = "desktop-menu-check-updates";
const MENU_ABOUT_ID: &str = "desktop-menu-about";
const MENU_CHANGE_SERVICE_ID: &str = "desktop-menu-change-service";
const MENU_LANG_EN_ID: &str = "desktop-menu-language-en";
const MENU_LANG_RU_ID: &str = "desktop-menu-language-ru";
const DETACHED_HANDOFF_GLOBAL: &str = "__PIPING_DETACHED_HANDOFF__";
const DETACHED_HANDOFF_MAX_BYTES: usize = 64 * 1024;
const SERVICE_CHROME_SCRIPT: &str = concat!(
    include_str!("../../ui/desktop-menu.js"),
    "\n;",
    include_str!("../../ui/service-chrome.js")
);

static DETACHED_WINDOW_COUNTER: AtomicUsize = AtomicUsize::new(1);
static LAST_MENU_WINDOW_LABEL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopConfig {
    service_url: Option<String>,
    #[serde(default)]
    services: Vec<String>,
}

impl DesktopConfig {
    fn remember(&mut self, service_url: String) {
        if !self.services.contains(&service_url) {
            self.services.push(service_url.clone());
        }
        self.service_url = Some(service_url);
    }

    fn forget(&mut self, service_url: &str) {
        self.services.retain(|saved| saved != service_url);
        if self.service_url.as_deref() == Some(service_url) {
            self.service_url = None;
        }
    }
}

#[derive(Default)]
struct ServiceWindows(Mutex<HashMap<String, Url>>);

#[derive(Default)]
// Canceled downloads may omit their final path; retain reserved names until exit.
struct DownloadPaths(Mutex<HashSet<PathBuf>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopLanguage {
    En,
    Ru,
}

impl DesktopLanguage {
    fn from_optional(value: Option<&str>) -> Self {
        if value
            .map(|language| language.to_ascii_lowercase().starts_with("ru"))
            .unwrap_or(false)
        {
            Self::Ru
        } else {
            Self::En
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceUrlIssue {
    Empty,
    ContainsWhitespace,
    InvalidUrl,
    UnsupportedScheme,
    MissingHost,
    ContainsCredentials,
    ContainsQuery,
    CannotConnect { address: String },
    NotPipingService { address: String },
    HttpsEndpointSpeaksHttp { suggested_url: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceProbeTlsMode {
    Strict,
    AcceptInvalidCertificates,
}

#[derive(Debug)]
struct ServiceProbeError {
    issue: ServiceUrlIssue,
    invalid_certificate: bool,
}

impl From<ServiceUrlIssue> for ServiceProbeError {
    fn from(issue: ServiceUrlIssue) -> Self {
        Self {
            issue,
            invalid_certificate: false,
        }
    }
}

fn service_probe_transport_error(error: &reqwest::Error, address: &str) -> ServiceProbeError {
    let invalid_certificate = is_certificate_validation_error(error);
    ServiceProbeError {
        issue: ServiceUrlIssue::CannotConnect {
            address: address.to_string(),
        },
        invalid_certificate,
    }
}

fn is_certificate_validation_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if matches!(
            current.downcast_ref::<rustls::Error>(),
            Some(rustls::Error::InvalidCertificate(_))
        ) {
            return true;
        }
        // io::Error::source skips its boxed error and exposes that error's source.
        source = current
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| inner as &(dyn std::error::Error + 'static))
            .or_else(|| current.source());
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceUrlProbe {
    Accepted,
    RequiresConfirmation(ServiceUrlConfirmation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceUrlConfirmation {
    InvalidCertificate { service_url: String },
    PlainHttpEndpoint { suggested_url: String },
}

impl ServiceUrlConfirmation {
    fn confirmed_service_url(&self) -> &str {
        match self {
            Self::InvalidCertificate { service_url } => service_url,
            Self::PlainHttpEndpoint { suggested_url } => suggested_url,
        }
    }

    fn title(&self, language: DesktopLanguage) -> &'static str {
        match (language, self) {
            (DesktopLanguage::En, Self::InvalidCertificate { .. }) => {
                "Certificate cannot be verified"
            }
            (DesktopLanguage::Ru, Self::InvalidCertificate { .. }) => {
                "Сертификат не удалось проверить"
            }
            (DesktopLanguage::En, Self::PlainHttpEndpoint { .. }) => "Use plain HTTP",
            (DesktopLanguage::Ru, Self::PlainHttpEndpoint { .. }) => "Использовать HTTP",
        }
    }

    fn message(&self, language: DesktopLanguage) -> String {
        match (language, self) {
            (DesktopLanguage::En, Self::InvalidCertificate { service_url }) => format!(
                "The Piping API was found at {service_url}, but its TLS certificate is not trusted by this system.\n\nContinue only if this is your Piping instance and you trust this network."
            ),
            (DesktopLanguage::Ru, Self::InvalidCertificate { service_url }) => format!(
                "Piping API найден по адресу {service_url}, но система не доверяет его TLS-сертификату.\n\nПродолжайте только если это ваш Piping-инстанс и вы доверяете этой сети."
            ),
            (DesktopLanguage::En, Self::PlainHttpEndpoint { suggested_url }) => format!(
                "The HTTPS address did not answer as a Piping service, but the Piping API was found over plain HTTP:\n\n{suggested_url}\n\nUse this HTTP address?"
            ),
            (DesktopLanguage::Ru, Self::PlainHttpEndpoint { suggested_url }) => format!(
                "HTTPS-адрес не ответил как Piping-сервис, но Piping API найден по обычному HTTP:\n\n{suggested_url}\n\nИспользовать этот HTTP-адрес?"
            ),
        }
    }

    fn confirm_label(&self, language: DesktopLanguage) -> &'static str {
        match (language, self) {
            (DesktopLanguage::En, Self::InvalidCertificate { .. }) => "Trust and continue",
            (DesktopLanguage::Ru, Self::InvalidCertificate { .. }) => "Доверять и продолжить",
            (DesktopLanguage::En, Self::PlainHttpEndpoint { .. }) => "Use HTTP",
            (DesktopLanguage::Ru, Self::PlainHttpEndpoint { .. }) => "Использовать HTTP",
        }
    }

    fn cancel_label(language: DesktopLanguage) -> &'static str {
        match language {
            DesktopLanguage::En => "Cancel",
            DesktopLanguage::Ru => "Отмена",
        }
    }

    fn cancelled_message(&self, language: DesktopLanguage) -> &'static str {
        match (language, self) {
            (DesktopLanguage::En, Self::InvalidCertificate { .. }) => {
                "Service address was not saved because the certificate warning was cancelled."
            }
            (DesktopLanguage::Ru, Self::InvalidCertificate { .. }) => {
                "Адрес сервиса не сохранен: предупреждение о сертификате отменено."
            }
            (DesktopLanguage::En, Self::PlainHttpEndpoint { .. }) => {
                "Service address was not saved because HTTP fallback was cancelled."
            }
            (DesktopLanguage::Ru, Self::PlainHttpEndpoint { .. }) => {
                "Адрес сервиса не сохранен: переход на HTTP отменен."
            }
        }
    }
}

impl ServiceUrlIssue {
    fn message(&self, language: DesktopLanguage) -> String {
        match (language, self) {
            (DesktopLanguage::En, Self::Empty) => "Enter the Piping service address.".to_string(),
            (DesktopLanguage::Ru, Self::Empty) => "Введите адрес Piping-сервиса.".to_string(),
            (DesktopLanguage::En, Self::ContainsWhitespace) => {
                "Service address must not contain spaces.".to_string()
            }
            (DesktopLanguage::Ru, Self::ContainsWhitespace) => {
                "В адресе сервиса не должно быть пробелов.".to_string()
            }
            (DesktopLanguage::En, Self::InvalidUrl) => {
                "Enter a valid HTTP or HTTPS address, for example https://piping.example.com."
                    .to_string()
            }
            (DesktopLanguage::Ru, Self::InvalidUrl) => {
                "Введите корректный HTTP или HTTPS адрес, например https://piping.example.com."
                    .to_string()
            }
            (DesktopLanguage::En, Self::UnsupportedScheme) => {
                "Service address must start with http:// or https://.".to_string()
            }
            (DesktopLanguage::Ru, Self::UnsupportedScheme) => {
                "Адрес сервиса должен начинаться с http:// или https://.".to_string()
            }
            (DesktopLanguage::En, Self::MissingHost) => {
                "Service address must include a host name or IP address.".to_string()
            }
            (DesktopLanguage::Ru, Self::MissingHost) => {
                "В адресе сервиса должен быть хост или IP-адрес.".to_string()
            }
            (DesktopLanguage::En, Self::ContainsCredentials) => {
                "Service address must not include a username or password.".to_string()
            }
            (DesktopLanguage::Ru, Self::ContainsCredentials) => {
                "В адресе сервиса не должно быть логина или пароля.".to_string()
            }
            (DesktopLanguage::En, Self::ContainsQuery) => {
                "Service address must not include query parameters.".to_string()
            }
            (DesktopLanguage::Ru, Self::ContainsQuery) => {
                "В адресе сервиса не должно быть query-параметров.".to_string()
            }
            (DesktopLanguage::En, Self::CannotConnect { address, .. }) => format!(
                "Could not connect to {address}. Check the address, VPN/network access, and that the Piping service is running."
            ),
            (DesktopLanguage::Ru, Self::CannotConnect { address, .. }) => format!(
                "Не удалось подключиться к {address}. Проверьте адрес, VPN/сеть и что Piping-сервис запущен."
            ),
            (DesktopLanguage::En, Self::NotPipingService { address, .. }) => format!(
                "The address {address} is reachable, but the Piping API was not found there. Enter the base URL of your Piping instance, not the public website or another page."
            ),
            (DesktopLanguage::Ru, Self::NotPipingService { address, .. }) => format!(
                "Адрес {address} доступен, но Piping API по нему не найден. Укажите базовый URL вашего Piping-инстанса, а не публичный сайт или другую страницу."
            ),
            (DesktopLanguage::En, Self::HttpsEndpointSpeaksHttp { suggested_url }) => format!(
                "This address responds over plain HTTP, not HTTPS. Use {suggested_url} or choose the HTTPS port."
            ),
            (DesktopLanguage::Ru, Self::HttpsEndpointSpeaksHttp { suggested_url }) => format!(
                "Этот адрес отвечает по обычному HTTP, а не HTTPS. Укажите {suggested_url} или выберите HTTPS-порт."
            ),
        }
    }
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn get_service_url(app: AppHandle) -> Result<Option<String>, String> {
    Ok(read_desktop_config(&app)?.service_url)
}

#[tauri::command]
fn get_services(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(read_desktop_config(&app)?.services)
}

#[tauri::command]
fn forget_service(app: AppHandle, url: String) -> Result<(), String> {
    let mut config = read_desktop_config(&app)?;
    config.forget(&url);
    write_desktop_config(&app, &config)
}

#[tauri::command]
fn get_system_language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

#[tauri::command]
async fn set_service_url(
    app: AppHandle,
    window: WebviewWindow,
    url: String,
    language: Option<String>,
) -> Result<String, String> {
    let language = DesktopLanguage::from_optional(language.as_deref());
    let mut service_url = normalize_service_url(&url).map_err(|issue| issue.message(language))?;
    match probe_service_url_before_opening(&service_url)
        .await
        .map_err(|issue| issue.message(language))?
    {
        ServiceUrlProbe::Accepted => {}
        ServiceUrlProbe::RequiresConfirmation(confirmation) => {
            confirm_service_url_warning(&app, &confirmation, language).await?;
            service_url = confirmation.confirmed_service_url().to_string();
        }
    }
    let mut config = read_desktop_config(&app)?;
    config.remember(service_url.clone());
    write_desktop_config(&app, &config)?;
    transition_service_window(app, window.label().to_string(), service_url.clone()).await?;
    Ok(service_url)
}

#[tauri::command]
async fn change_service_address(app: AppHandle) -> Result<(), String> {
    // WebView2 construction can deadlock in a synchronous IPC/event callback.
    tauri::async_runtime::spawn_blocking(move || open_bootstrap_window(&app))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn show_about(app: AppHandle, language: Option<String>) {
    let (title, message) = if is_russian(&language) {
        (
            "О Piping",
            format!(
                "Piping Desktop\nВерсия {}\n\nТонкая desktop-оболочка для подключений через ваш Piping-сервис.",
                env!("CARGO_PKG_VERSION")
            ),
        )
    } else {
        (
            "About Piping",
            format!(
                "Piping Desktop\nVersion {}\n\nA thin desktop shell for connections through your Piping service.",
                env!("CARGO_PKG_VERSION")
            ),
        )
    };

    show_message(&app, title, &message, MessageDialogKind::Info);
}

#[tauri::command]
async fn check_for_updates(app: AppHandle, language: Option<String>) {
    let russian = is_russian(&language);
    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = tauri::async_runtime::spawn_blocking(fetch_latest_public_version)
        .await
        .ok()
        .and_then(Result::ok);

    let (title, message, download, later) = if russian {
        (
            "Проверка обновлений",
            match &latest {
                Some(version) if version_is_newer(version, &current) => {
                    format!("Доступна версия {version}. У вас установлена {current}.")
                }
                Some(_) => format!("У вас последняя версия: {current}."),
                None => {
                    "Не удалось проверить обновления. Проверьте сеть и повторите позже.".to_string()
                }
            },
            "Скачать",
            "Позже",
        )
    } else {
        (
            "Update check",
            match &latest {
                Some(version) if version_is_newer(version, &current) => {
                    format!("Version {version} is available. You have {current}.")
                }
                Some(_) => format!("You are running the latest version: {current}."),
                None => {
                    "Could not check for updates. Check your network and try again.".to_string()
                }
            },
            "Download",
            "Later",
        )
    };

    let newer = latest
        .as_deref()
        .is_some_and(|version| version_is_newer(version, &current));
    let target = latest
        .as_deref()
        .filter(|_| newer)
        .map(|version| format!("https://github.com/mlimarenko/piping/releases/tag/{version}"));
    let dialog = app
        .dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Info);
    let dialog = if newer {
        dialog.buttons(MessageDialogButtons::OkCancelCustom(
            download.to_string(),
            later.to_string(),
        ))
    } else {
        dialog
    };
    dialog.show(move |confirmed| {
        if confirmed
            && let Some(url) = target
            && let Err(error) = open::that_detached(&url)
        {
            eprintln!("failed to open the release page: {error}");
        }
    });
}

fn fetch_latest_public_version() -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_millis(4_000))
        .timeout(Duration::from_millis(8_000))
        .user_agent(concat!("Piping-Desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get("https://api.github.com/repos/mlimarenko/piping/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", response.status()));
    }
    let document: serde_json::Value = response.json().map_err(|error| error.to_string())?;
    document
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(|tag| tag.trim_start_matches('v').to_string())
        .ok_or_else(|| "the release has no tag".to_string())
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    fn parts(version: &str) -> (u64, u64, u64) {
        let mut numbers = version
            .split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0));
        (
            numbers.next().unwrap_or(0),
            numbers.next().unwrap_or(0),
            numbers.next().unwrap_or(0),
        )
    }
    parts(candidate) > parts(current)
}

#[tauri::command]
fn minimize_window(window: Window) -> Result<(), String> {
    window
        .minimize()
        .map_err(|error| format!("failed to minimize window: {error}"))
}

#[tauri::command]
fn toggle_maximize_window(window: Window) -> Result<(), String> {
    let maximized = window
        .is_maximized()
        .map_err(|error| format!("failed to inspect window state: {error}"))?;
    if maximized {
        window
            .unmaximize()
            .map_err(|error| format!("failed to restore window: {error}"))
    } else {
        window
            .maximize()
            .map_err(|error| format!("failed to maximize window: {error}"))
    }
}

#[tauri::command]
fn close_window(window: Window) -> Result<(), String> {
    window
        .close()
        .map_err(|error| format!("failed to close window: {error}"))
}

#[tauri::command]
fn start_window_drag(window: Window) -> Result<(), String> {
    window
        .start_dragging()
        .map_err(|error| format!("failed to start window drag: {error}"))
}

#[tauri::command]
fn show_app_menu(app: AppHandle, window: Window, language: Option<String>) -> Result<(), String> {
    let russian = is_russian(&language);
    let menu = if russian {
        MenuBuilder::new(&app)
            .text(MENU_LANG_RU_ID, "Русский")
            .text(MENU_LANG_EN_ID, "English")
            .separator()
            .text(MENU_OPEN_WEB_CONSOLE_ID, "Открыть веб-консоль")
            .text(MENU_CHECK_UPDATES_ID, "Обновления")
            .text(MENU_ABOUT_ID, "О приложении")
            .text(MENU_CHANGE_SERVICE_ID, "Инстансы…")
            .build()
    } else {
        MenuBuilder::new(&app)
            .text(MENU_LANG_EN_ID, "English")
            .text(MENU_LANG_RU_ID, "Русский")
            .separator()
            .text(MENU_OPEN_WEB_CONSOLE_ID, "Open web console")
            .text(MENU_CHECK_UPDATES_ID, "Check updates")
            .text(MENU_ABOUT_ID, "About")
            .text(MENU_CHANGE_SERVICE_ID, "Instances…")
            .build()
    }
    .map_err(|error| format!("failed to build app menu: {error}"))?;

    remember_menu_window_label(window.label());
    window
        .popup_menu_at(&menu, LogicalPosition::new(4.0, TOOLBAR_HEIGHT_LOGICAL))
        .map_err(|error| format!("failed to show app menu: {error}"))
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let url = normalize_external_web_url(&url)?;
    open::that_detached(url.as_str())
        .map_err(|error| format!("failed to open external URL: {error}"))
}

#[tauri::command]
fn open_detached_window(
    app: AppHandle,
    window: WebviewWindow,
    path: String,
    handoff: Option<serde_json::Value>,
    title: Option<String>,
) -> Result<(), String> {
    let detached_path = normalize_detached_route_path(&path)?;
    let window_title = normalize_window_title(title);
    let service_url = service_for_window(&app, &window)?;
    let handoff_script = detached_handoff_initialization_script(&service_url, handoff)?;
    let detached_url = service_url
        .join(&detached_path)
        .map_err(|error| format!("failed to build detached window URL: {error}"))?;

    schedule_detached_service_window(app, detached_url, handoff_script, Some(window_title))?;
    Ok(())
}

#[tauri::command]
async fn open_app_uri(app: AppHandle, window: WebviewWindow, uri: String) -> Result<(), String> {
    service_for_window(&app, &window)?;
    match normalize_app_launcher_target(&uri)? {
        AppLauncherTarget::Uri(target) => {
            open::that_detached(target).map_err(|error| format!("failed to open app URI: {error}"))
        }
        AppLauncherTarget::LocalCommand(command) => {
            confirm_local_app_command(&app, &command).await?;
            launch_local_app_command(&command)
        }
    }
}

fn desktop_config_path() -> Result<PathBuf, String> {
    let config_dir = desktop_config_dir()?;
    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("failed to create desktop configuration directory: {error}"))?;
    Ok(config_dir.join(DESKTOP_CONFIG_FILE))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
// Each build constructs one variant; tests exercise all three.
#[allow(dead_code)]
enum DesktopPlatform {
    Linux,
    MacOs,
    Windows,
}

impl DesktopPlatform {
    fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }
}

fn desktop_config_dir() -> Result<PathBuf, String> {
    desktop_config_dir_from(
        std::env::var_os(CONFIG_DIR_ENV),
        DesktopPlatform::current(),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("APPDATA"),
        std::env::var_os("USERPROFILE"),
    )
}

/// One directory name on every platform: `<platform config root>/piping`.
///
/// A managed installation may point `PIPING_DESKTOP_CONFIG_DIR` at another
/// absolute directory. The bundle identifier identifies the application
/// package and the updater, never the user's configuration location.
fn desktop_config_dir_from(
    override_dir: Option<std::ffi::OsString>,
    platform: DesktopPlatform,
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    appdata: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if let Some(value) = override_dir.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(format!("{CONFIG_DIR_ENV} must be an absolute path"));
        }
        return Ok(path);
    }

    platform_config_root(platform, xdg_config_home, home, appdata, userprofile)
        .map(|root| root.join(CONFIG_DIR_NAME))
        .ok_or_else(|| "failed to resolve the desktop configuration directory".to_string())
}

fn platform_config_root(
    platform: DesktopPlatform,
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    appdata: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    match platform {
        DesktopPlatform::Linux => linux_config_home(xdg_config_home, home),
        DesktopPlatform::MacOs => non_empty(home)
            .map(PathBuf::from)
            .map(|home| home.join("Library").join("Application Support")),
        DesktopPlatform::Windows => non_empty(appdata).map(PathBuf::from).or_else(|| {
            non_empty(userprofile)
                .map(PathBuf::from)
                .map(|home| home.join("AppData").join("Roaming"))
        }),
    }
}

fn non_empty(value: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    value.filter(|value| !value.is_empty())
}

#[cfg(target_os = "linux")]
fn linux_config_home(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(value) = xdg_config_home.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Some(path);
        }
    }

    home.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config"))
}

#[cfg(not(target_os = "linux"))]
fn linux_config_home(
    _xdg_config_home: Option<std::ffi::OsString>,
    _home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    None
}

/// Moves the pre-PIP-612 layout into `<config dir>/app.json`.
///
/// The old layout used the bundle identifier as the directory name
/// (`space.piping.desktop`) and `desktop-config.json` as the file name. The
/// migration merges instead of replacing: an `app.json` created by an installer
/// or an earlier partial run does not stop saved instances and `webviews/`
/// profiles from being imported, and a failed run is retried on the next start.
fn migrate_legacy_desktop_config(
    config_dir: &Path,
    legacy_bundle_dir: Option<&Path>,
) -> Result<bool, String> {
    let new_file = config_dir.join(DESKTOP_CONFIG_FILE);
    let mut config = load_config_for_migration(&new_file)?;
    let mut imported = false;
    let mut touched = false;

    let legacy_file = config_dir.join(LEGACY_DESKTOP_CONFIG_FILE);
    if legacy_file.exists() {
        match read_desktop_config_file(&legacy_file) {
            Ok(Some(legacy)) => {
                merge_desktop_config(&mut config, &legacy);
                imported = true;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("quarantining an unreadable legacy desktop config: {error}");
                quarantine_path(&legacy_file, "corrupt")?;
            }
        }
        touched = true;
    }

    let legacy_dir = legacy_bundle_dir.filter(|dir| *dir != config_dir && dir.exists());
    if let Some(legacy_dir) = legacy_dir {
        let legacy_config = legacy_dir.join(LEGACY_DESKTOP_CONFIG_FILE);
        match read_desktop_config_file(&legacy_config) {
            Ok(Some(legacy)) => {
                merge_desktop_config(&mut config, &legacy);
                imported = true;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("quarantining an unreadable legacy desktop config: {error}");
                quarantine_path(&legacy_config, "corrupt")?;
            }
        }
        touched = true;
    }

    // The merged file must be durable before any legacy copy is removed.
    if imported {
        write_desktop_config_file(config_dir, &config)?;
    }

    if legacy_file.exists() {
        fs::remove_file(&legacy_file)
            .map_err(|error| format!("failed to remove the legacy desktop config: {error}"))?;
    }

    if let Some(legacy_dir) = legacy_dir {
        let fully_merged = merge_directory(legacy_dir, config_dir)?;
        if legacy_dir.exists() {
            if fully_merged {
                let _ = fs::remove_dir_all(legacy_dir);
            } else {
                // A conflicting profile (for example a session the previous
                // client refreshed while it was still running) is preserved
                // instead of being deleted or mixed into the destination store.
                let quarantined = quarantine_path(legacy_dir, "legacy")?;
                eprintln!(
                    "kept conflicting legacy desktop data at {}",
                    quarantined.display()
                );
            }
        }
    }

    Ok(touched)
}

fn load_config_for_migration(path: &Path) -> Result<DesktopConfig, String> {
    match read_desktop_config_file(path) {
        Ok(Some(config)) => Ok(config),
        Ok(None) => Ok(DesktopConfig::default()),
        Err(error) => {
            eprintln!("quarantining an unreadable desktop config: {error}");
            quarantine_path(path, "corrupt")?;
            Ok(DesktopConfig::default())
        }
    }
}

fn quarantine_path(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "legacy".to_string());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let target = path.with_file_name(format!("{name}.{suffix}-{stamp}"));
    fs::rename(path, &target)
        .map_err(|error| format!("failed to preserve the unreadable desktop data: {error}"))?;
    Ok(target)
}

fn merge_desktop_config(target: &mut DesktopConfig, legacy: &DesktopConfig) {
    if target.service_url.is_none()
        && let Some(active) = legacy.service_url.clone()
    {
        target.remember(active);
    }
    for service in &legacy.services {
        if !target.services.contains(service) {
            target.services.push(service.clone());
        }
    }
}

/// Moves legacy entries into `destination`.
///
/// `webviews/` is merged per instance: an instance directory that already
/// exists at the destination is a conflict and is preserved by the caller,
/// because mixing two WebKit/WebView2 stores would corrupt both. Any other
/// existing entry is also a conflict; returning `false` tells the caller to
/// keep the remaining legacy data instead of deleting it.
fn merge_directory(source: &Path, destination: &Path) -> Result<bool, String> {
    merge_directory_level(source, destination, true)
}

fn merge_directory_level(
    source: &Path,
    destination: &Path,
    containers: bool,
) -> Result<bool, String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create the desktop directory: {error}"))?;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("failed to read the legacy desktop directory: {error}"))?;
    let mut complete = true;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read a legacy entry: {error}"))?;
        if entry.file_name() == LEGACY_DESKTOP_CONFIG_FILE {
            continue;
        }
        let target = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect a legacy entry: {error}"))?;
        if file_type.is_dir() {
            if target.exists() {
                if containers && (entry.file_name() == "webviews" || directory_is_empty(&target)) {
                    complete &= merge_directory_level(&entry.path(), &target, false)?;
                } else {
                    complete = false;
                }
            } else {
                fs::rename(entry.path(), &target)
                    .map_err(|error| format!("failed to move a legacy directory: {error}"))?;
            }
        } else if target.exists() {
            complete = false;
        } else {
            fs::rename(entry.path(), &target)
                .map_err(|error| format!("failed to move a legacy desktop entry: {error}"))?;
        }
    }
    Ok(complete)
}

fn directory_is_empty(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

fn read_desktop_config_file(path: &Path) -> Result<Option<DesktopConfig>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read desktop configuration: {error}"))?;
    let mut config: DesktopConfig = serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse desktop configuration: {error}"))?;
    if let Some(active) = config.service_url.clone() {
        config.remember(active);
    }
    Ok(Some(config))
}

fn read_desktop_config(_app: &AppHandle) -> Result<DesktopConfig, String> {
    let path = desktop_config_path()?;
    Ok(read_desktop_config_file(&path)?.unwrap_or_default())
}

fn installed_service_url_seed_path() -> Result<Option<PathBuf>, String> {
    let executable_path = std::env::current_exe()
        .map_err(|error| format!("failed to resolve desktop executable path: {error}"))?;
    Ok(executable_path
        .parent()
        .map(|directory| directory.join(DESKTOP_SERVICE_URL_SEED_FILE)))
}

fn read_installed_service_url_seed() -> Result<Option<String>, String> {
    let Some(path) = installed_service_url_seed_path()? else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read installed service URL seed: {error}"))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn write_desktop_config(_app: &AppHandle, config: &DesktopConfig) -> Result<(), String> {
    write_desktop_config_file(&desktop_config_dir()?, config)
}

fn write_desktop_config_file(config_dir: &Path, config: &DesktopConfig) -> Result<(), String> {
    fs::create_dir_all(config_dir)
        .map_err(|error| format!("failed to create desktop configuration directory: {error}"))?;
    let path = config_dir.join(DESKTOP_CONFIG_FILE);
    let raw = serde_json::to_string_pretty(config)
        .map_err(|error| format!("failed to encode desktop configuration: {error}"))?;
    let pending = path.with_extension("json.pending");
    fs::write(&pending, raw)
        .map_err(|error| format!("failed to write desktop configuration: {error}"))?;
    fs::rename(pending, path)
        .map_err(|error| format!("failed to save desktop configuration: {error}"))
}

async fn transition_service_window(
    app: AppHandle,
    source_window_label: String,
    service_url: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        open_configured_service_window(&app, &source_window_label, &service_url)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn schedule_detached_service_window(
    app: AppHandle,
    url: Url,
    extra_initialization_script: Option<String>,
    title: Option<String>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("piping-detached-window-open".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(
                SERVICE_WINDOW_TRANSITION_DELAY_MS,
            ));
            if let Err(error) =
                open_detached_service_window(&app, url, extra_initialization_script, title)
            {
                eprintln!("failed to open detached Piping window: {error}");
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to schedule detached window: {error}"))
}

fn open_configured_service_window(
    app: &AppHandle,
    source_window_label: &str,
    service_url: &str,
) -> Result<(), String> {
    open_main_window(app, service_url)?;
    if source_window_label == BOOTSTRAP_WINDOW_LABEL
        && let Some(window) = app.get_webview_window(source_window_label)
    {
        window
            .close()
            .map_err(|error| format!("failed to close bootstrap window: {error}"))?;
    }
    Ok(())
}

fn command_line_service_url() -> Result<Option<String>, String> {
    extract_service_url_arg(
        std::env::args_os()
            .skip(1)
            .map(|value| value.to_string_lossy().into_owned()),
    )
}

fn startup_service_url_override() -> Result<Option<String>, String> {
    if let Some(raw) = command_line_service_url()? {
        return Ok(Some(raw));
    }

    Ok(std::env::var(SERVICE_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn extract_service_url_arg<I>(args: I) -> Result<Option<String>, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        for name in SERVICE_URL_ARG_NAMES {
            if arg == name {
                return args
                    .next()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .map(Some)
                    .ok_or_else(|| format!("{name} requires a non-empty service URL value"));
            }

            let prefix = format!("{name}=");
            if let Some(value) = arg.strip_prefix(&prefix) {
                let value = value.trim();
                if value.is_empty() {
                    return Err(format!("{name} requires a non-empty service URL value"));
                }
                return Ok(Some(value.to_string()));
            }
        }
    }

    Ok(None)
}

fn normalize_service_url(raw: &str) -> Result<String, ServiceUrlIssue> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(ServiceUrlIssue::Empty);
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(ServiceUrlIssue::ContainsWhitespace);
    }

    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let mut url = Url::parse(&candidate).map_err(|_| ServiceUrlIssue::InvalidUrl)?;

    match url.scheme() {
        "http" | "https" => {}
        _ => return Err(ServiceUrlIssue::UnsupportedScheme),
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err(ServiceUrlIssue::MissingHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ServiceUrlIssue::ContainsCredentials);
    }
    if url.query().is_some() {
        return Err(ServiceUrlIssue::ContainsQuery);
    }
    url.set_fragment(None);

    Ok(url.as_str().trim_end_matches('/').to_string())
}

async fn probe_service_url_before_opening(
    service_url: &str,
) -> Result<ServiceUrlProbe, ServiceUrlIssue> {
    let url = Url::parse(service_url).map_err(|_| ServiceUrlIssue::InvalidUrl)?;
    probe_service_url_before_opening_url(url).await
}

async fn probe_service_url_before_opening_url(
    url: Url,
) -> Result<ServiceUrlProbe, ServiceUrlIssue> {
    let address = service_origin(&url);
    tauri::async_runtime::spawn_blocking(move || probe_service_url_endpoint(&url))
        .await
        .map_err(|_| ServiceUrlIssue::CannotConnect { address })?
}

fn validate_service_url_endpoint(url: &Url) -> Result<(), ServiceUrlIssue> {
    match probe_service_url_endpoint(url)? {
        ServiceUrlProbe::Accepted
        | ServiceUrlProbe::RequiresConfirmation(ServiceUrlConfirmation::InvalidCertificate {
            ..
        }) => Ok(()),
        ServiceUrlProbe::RequiresConfirmation(ServiceUrlConfirmation::PlainHttpEndpoint {
            suggested_url,
        }) => Err(ServiceUrlIssue::HttpsEndpointSpeaksHttp { suggested_url }),
    }
}

fn probe_service_url_endpoint(url: &Url) -> Result<ServiceUrlProbe, ServiceUrlIssue> {
    let address = service_origin(url);
    let client = build_service_probe_client(ServiceProbeTlsMode::Strict, &address)?;

    resolve_service_probe(
        url,
        validate_piping_service_api(&client, url),
        || {
            build_service_probe_client(ServiceProbeTlsMode::AcceptInvalidCertificates, &address)
                .is_ok_and(|client| validate_piping_service_api(&client, url).is_ok())
        },
        || reachable_http_variant(&client, url),
    )
}

fn resolve_service_probe(
    url: &Url,
    strict: Result<(), ServiceProbeError>,
    insecure_probe: impl FnOnce() -> bool,
    http_probe: impl FnOnce() -> Option<String>,
) -> Result<ServiceUrlProbe, ServiceUrlIssue> {
    match strict {
        Ok(()) => Ok(ServiceUrlProbe::Accepted),
        Err(error) => {
            if url.scheme() == "https" && error.invalid_certificate && insecure_probe() {
                return Ok(ServiceUrlProbe::RequiresConfirmation(
                    ServiceUrlConfirmation::InvalidCertificate {
                        service_url: url.as_str().trim_end_matches('/').to_string(),
                    },
                ));
            }
            if url.scheme() == "https"
                && let Some(suggested_url) = http_probe()
            {
                return Ok(ServiceUrlProbe::RequiresConfirmation(
                    ServiceUrlConfirmation::PlainHttpEndpoint { suggested_url },
                ));
            }
            Err(error.issue)
        }
    }
}

fn build_service_probe_client(
    tls_mode: ServiceProbeTlsMode,
    address: &str,
) -> Result<reqwest::blocking::Client, ServiceUrlIssue> {
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_millis(SERVICE_URL_CONNECT_TIMEOUT_MS))
        .timeout(Duration::from_millis(SERVICE_URL_REQUEST_TIMEOUT_MS))
        .redirect(reqwest::redirect::Policy::none());
    if tls_mode == ServiceProbeTlsMode::AcceptInvalidCertificates {
        builder = builder
            .tls_danger_accept_invalid_certs(true)
            .tls_danger_accept_invalid_hostnames(true);
    }
    builder.build().map_err(|_| ServiceUrlIssue::CannotConnect {
        address: address.to_string(),
    })
}

fn reachable_http_variant(client: &reqwest::blocking::Client, url: &Url) -> Option<String> {
    if !allows_http_variant_suggestion(url) {
        return None;
    }
    let suggested_url = http_variant_url(url).ok()?;
    let suggested = Url::parse(&suggested_url).ok()?;
    validate_piping_service_api(client, &suggested).ok()?;
    Some(suggested_url)
}

fn validate_piping_service_api(
    client: &reqwest::blocking::Client,
    url: &Url,
) -> Result<(), ServiceProbeError> {
    let address = service_origin(url);
    validate_piping_health_endpoint(client, url, &address)?;
    validate_piping_auth_providers_endpoint(client, url, &address)?;
    Ok(())
}

fn validate_piping_health_endpoint(
    client: &reqwest::blocking::Client,
    url: &Url,
    address: &str,
) -> Result<(), ServiceProbeError> {
    let health_url = service_api_url(url, SERVICE_HEALTH_PATH)?;
    let response = client
        .get(health_url.as_str())
        .send()
        .map_err(|error| service_probe_transport_error(&error, address))?;
    if !response.status().is_success() {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        }
        .into());
    }

    let payload = read_probe_json_response(response, address)?;
    if payload.get("status").and_then(|value| value.as_str()) == Some("ok") {
        Ok(())
    } else {
        Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        }
        .into())
    }
}

fn validate_piping_auth_providers_endpoint(
    client: &reqwest::blocking::Client,
    url: &Url,
    address: &str,
) -> Result<(), ServiceProbeError> {
    let providers_url = service_api_url(url, SERVICE_AUTH_PROVIDERS_PATH)?;
    let response = client
        .get(providers_url.as_str())
        .send()
        .map_err(|error| service_probe_transport_error(&error, address))?;
    if !response.status().is_success() {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        }
        .into());
    }

    let payload = read_probe_json_response(response, address)?;
    let has_expected_shape = [
        "local_enabled",
        "oidc_enabled",
        "ldap_enabled",
        "ad_enabled",
    ]
    .into_iter()
    .all(|key| payload.get(key).and_then(|value| value.as_bool()).is_some());
    if has_expected_shape {
        Ok(())
    } else {
        Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        }
        .into())
    }
}

fn read_probe_json_response(
    response: reqwest::blocking::Response,
    address: &str,
) -> Result<serde_json::Value, ServiceUrlIssue> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("");
    if !content_type.eq_ignore_ascii_case("application/json") {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        });
    }

    if response
        .content_length()
        .is_some_and(|length| length > SERVICE_PROBE_BODY_MAX_BYTES)
    {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        });
    }

    let body = response
        .text()
        .map_err(|_| ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        })?;
    if body.len() as u64 > SERVICE_PROBE_BODY_MAX_BYTES {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        });
    }

    serde_json::from_str(&body).map_err(|_| ServiceUrlIssue::NotPipingService {
        address: address.to_string(),
    })
}

fn service_api_url(url: &Url, path: &str) -> Result<Url, ServiceUrlIssue> {
    url.join(path).map_err(|_| ServiceUrlIssue::InvalidUrl)
}

fn allows_http_variant_suggestion(url: &Url) -> bool {
    url.host_str().is_some()
}

fn http_variant_url(url: &Url) -> Result<String, ServiceUrlIssue> {
    let mut suggested_url = url.clone();
    suggested_url
        .set_scheme("http")
        .map_err(|()| ServiceUrlIssue::InvalidUrl)?;
    Ok(suggested_url.as_str().trim_end_matches('/').to_string())
}

fn normalize_detached_route_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with("/connections/detached/") {
        return Err("detached window path must target a detached connection route".to_string());
    }
    if trimmed.starts_with("//") || trimmed.contains("://") {
        return Err("detached window path must be service-relative".to_string());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("detached window path must not contain whitespace".to_string());
    }

    Ok(trimmed.to_string())
}

fn detached_handoff_initialization_script(
    service_url: &Url,
    handoff: Option<serde_json::Value>,
) -> Result<Option<String>, String> {
    let Some(handoff) = handoff else {
        return Ok(None);
    };
    let serialized = serde_json::to_string(&handoff)
        .map_err(|error| format!("failed to serialize detached handoff: {error}"))?;
    if serialized.len() > DETACHED_HANDOFF_MAX_BYTES {
        return Err("detached handoff is too large".to_string());
    }
    let origin = serde_json::to_string(&service_url.origin().ascii_serialization())
        .map_err(|error| format!("failed to serialize service origin: {error}"))?;
    Ok(Some(format!(
        r#"(() => {{
            if (window.top !== window || window.location.origin !== {origin}) return;
            Object.defineProperty(window, "{DETACHED_HANDOFF_GLOBAL}", {{ value: {serialized}, writable: true, configurable: true }});
        }})();"#
    )))
}

fn normalize_window_title(title: Option<String>) -> String {
    let cleaned: String = title
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(WINDOW_TITLE_MAX_CHARS)
        .collect();
    if cleaned.is_empty() {
        "Piping".to_string()
    } else {
        format!("{cleaned} | Piping")
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AppLauncherTarget {
    Uri(String),
    LocalCommand(String),
}

fn normalize_app_launcher_target(raw: &str) -> Result<AppLauncherTarget, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("app launcher target must not be empty".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("app launcher target must not contain control characters".to_string());
    }

    let Some((scheme, _rest)) = split_app_launcher_uri(trimmed) else {
        return Ok(AppLauncherTarget::LocalCommand(trimmed.to_string()));
    };
    let normalized_scheme = scheme.to_ascii_lowercase();
    if is_blocked_app_uri_scheme(&normalized_scheme) {
        return Err("in-page URI schemes are not valid app launchers".to_string());
    }
    if strict_app_uri_scheme(&normalized_scheme)
        || !trimmed
            .chars()
            .any(|character| character == '"' || character.is_whitespace())
    {
        return normalize_app_launcher_uri(trimmed).map(AppLauncherTarget::Uri);
    }

    Ok(AppLauncherTarget::LocalCommand(trimmed.to_string()))
}

fn normalize_app_launcher_uri(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed
        .chars()
        .any(|character| character == '"' || character.is_whitespace() || character.is_control())
    {
        return Err(
            "app URI must not contain quotes, whitespace, or control characters".to_string(),
        );
    }
    if !has_valid_percent_encoding(trimmed) {
        return Err("app URI contains invalid percent encoding".to_string());
    }

    let (scheme, rest) = split_app_launcher_uri(trimmed)
        .ok_or_else(|| "app URI must include a scheme".to_string())?;
    if !is_valid_uri_scheme(scheme) {
        return Err("app URI scheme is invalid".to_string());
    }

    let normalized_scheme = scheme.to_ascii_lowercase();
    if is_blocked_app_uri_scheme(&normalized_scheme) {
        return Err("in-page URI schemes are not valid app launchers".to_string());
    }
    if matches!(normalized_scheme.as_str(), "http" | "https") {
        validate_web_launcher_uri(trimmed)?;
    }
    if matches!(
        normalized_scheme.as_str(),
        "microsoft-edge" | "microsoft-edge-holographic"
    ) {
        validate_wrapped_web_launcher_uri(rest)?;
    }

    Ok(format!(
        "{normalized_scheme}:{}",
        &trimmed[scheme.len() + 1..]
    ))
}

fn split_app_launcher_uri(raw: &str) -> Option<(&str, &str)> {
    if looks_like_windows_drive_path(raw) {
        return None;
    }
    let (scheme, rest) = raw.split_once(':')?;
    is_valid_uri_scheme(scheme).then_some((scheme, rest))
}

fn strict_app_uri_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "file" | "http" | "https" | "mailto" | "microsoft-edge" | "microsoft-edge-holographic"
    )
}

fn is_blocked_app_uri_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "about" | "blob" | "data" | "intent" | "javascript" | "vbscript"
    )
}

fn looks_like_windows_drive_path(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn has_valid_percent_encoding(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

fn validate_web_launcher_uri(raw: &str) -> Result<(), String> {
    let url = Url::parse(raw)
        .map_err(|_| "app launcher URI must contain a valid http(s) URL".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("app launcher URI must use http or https".to_string()),
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err("app launcher URI must include a host".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("app launcher URI must not include credentials".to_string());
    }
    Ok(())
}

fn validate_wrapped_web_launcher_uri(raw: &str) -> Result<(), String> {
    let url = Url::parse(raw)
        .map_err(|_| "wrapped app launcher URI must contain a valid http(s) URL".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("wrapped app launcher URI must contain http or https".to_string()),
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err("wrapped app launcher URI must include a host".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("wrapped app launcher URI must not include credentials".to_string());
    }
    Ok(())
}

fn is_valid_uri_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }

    chars.all(|character| {
        character.is_ascii_alphanumeric()
            || character == '+'
            || character == '-'
            || character == '.'
    })
}

fn launch_local_app_command(command: &str) -> Result<(), String> {
    let argv =
        shlex::split(command).ok_or_else(|| "failed to parse local app command".to_string())?;
    let Some((program, args)) = argv.split_first() else {
        return Err("local app command must include an executable".to_string());
    };
    Command::new(program)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to launch local app command: {error}"))
}

async fn confirm_local_app_command(app: &AppHandle, command: &str) -> Result<(), String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog()
        .message(format!(
            "Piping Desktop will launch this local command:\n\n{command}"
        ))
        .title("Launch local application")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Launch".to_string(),
            "Cancel".to_string(),
        ))
        .show(move |confirmed| {
            let _ = tx.blocking_send(confirmed);
        });
    let confirmed = rx
        .recv()
        .await
        .ok_or_else(|| "app launch confirmation failed".to_string())?;
    if confirmed {
        Ok(())
    } else {
        Err("app launch cancelled by user".to_string())
    }
}

async fn confirm_service_url_warning(
    app: &AppHandle,
    confirmation: &ServiceUrlConfirmation,
    language: DesktopLanguage,
) -> Result<(), String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog()
        .message(confirmation.message(language))
        .title(confirmation.title(language))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            confirmation.confirm_label(language).to_string(),
            ServiceUrlConfirmation::cancel_label(language).to_string(),
        ))
        .show(move |confirmed| {
            let _ = tx.blocking_send(confirmed);
        });
    let confirmed = rx
        .recv()
        .await
        .ok_or_else(|| "service URL confirmation failed".to_string())?;
    if confirmed {
        Ok(())
    } else {
        Err(confirmation.cancelled_message(language).to_string())
    }
}

fn window_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("icons/128x128.png")
}

fn build_bootstrap_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    let builder = WebviewWindowBuilder::new(app, BOOTSTRAP_WINDOW_LABEL, WebviewUrl::default())
        .icon(window_icon())
        .map_err(|error| format!("failed to set the window icon: {error}"))?;
    builder
        .title("Piping")
        .inner_size(760.0, 560.0)
        .min_inner_size(560.0, 460.0)
        .resizable(true)
        .decorations(false)
        .build()
        .map_err(|error| format!("failed to create bootstrap window: {error}"))
}

fn open_bootstrap_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(BOOTSTRAP_WINDOW_LABEL) {
        window
            .set_focus()
            .map_err(|error| format!("failed to focus bootstrap window: {error}"))?;
        return Ok(());
    }

    build_bootstrap_window(app)?;
    Ok(())
}

fn open_main_window(app: &AppHandle, service_url: &str) -> Result<(), String> {
    let existing = app
        .state::<ServiceWindows>()
        .0
        .lock()
        .map_err(|_| "service window registry is unavailable".to_string())?
        .iter()
        .find_map(|(label, url)| {
            (Url::parse(service_url).as_ref() == Ok(url) && app.get_window(label).is_some())
                .then(|| label.clone())
        });
    if let Some(window) = existing.and_then(|label| app.get_window(&label)) {
        window
            .set_focus()
            .map_err(|error| format!("failed to focus main window: {error}"))?;
        return Ok(());
    }

    let url = Url::parse(service_url)
        .map_err(|error| format!("failed to parse saved service URL: {error}"))?;
    if app.get_window(MAIN_WINDOW_LABEL).is_some() {
        return open_detached_service_window(app, url, None, None);
    }
    open_service_window(
        app,
        MAIN_WINDOW_LABEL,
        url,
        None,
        "Piping".to_string(),
        false,
    )
}

fn open_detached_service_window(
    app: &AppHandle,
    url: Url,
    extra_initialization_script: Option<String>,
    title: Option<String>,
) -> Result<(), String> {
    let index = DETACHED_WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed);
    let window_label = format!("detached-{index}");
    open_service_window(
        app,
        &window_label,
        url,
        extra_initialization_script,
        title.unwrap_or_else(|| "Piping".to_string()),
        true,
    )
}

fn open_service_window(
    app: &AppHandle,
    window_label: &str,
    url: Url,
    extra_initialization_script: Option<String>,
    title: String,
    reveal_after_load: bool,
) -> Result<(), String> {
    let popup_app = app.clone();
    let instance_url = url.clone();
    let popup_origin = url.clone();
    let download_origin = url.origin();
    let profile = desktop_config_dir()?
        .join("webviews")
        .join(service_profile_key(&url));
    let page_load_label = window_label.to_string();
    let initialization_script = match extra_initialization_script {
        Some(extra) => format!("{SERVICE_CHROME_SCRIPT}\n;{extra}"),
        None => SERVICE_CHROME_SCRIPT.to_string(),
    };
    let builder = WebviewWindowBuilder::new(app, window_label, WebviewUrl::External(url))
        .data_directory(profile)
        .disable_drag_drop_handler();
    #[cfg(target_os = "macos")]
    let builder = builder.data_store_identifier(service_profile_identifier(&instance_url));
    let builder = builder
        .icon(window_icon())
        .map_err(|error| format!("failed to set the window icon: {error}"))?;
    let window = builder
        .title(title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 620.0)
        .resizable(true)
        .decorations(false)
        .visible(!reveal_after_load)
        .initialization_script(initialization_script.as_str())
        // WebKitGTK does not select a download destination without a handler.
        .on_download(move |webview, event| {
            let app = webview.app_handle();
            match event {
                DownloadEvent::Requested { destination, .. } => {
                    if webview.url().map(|url| url.origin()).ok().as_ref() != Some(&download_origin) {
                        return false;
                    }
                    let path = app.path().download_dir()
                        .map_err(|error| error.to_string())
                        .and_then(|directory| {
                            let state = app.state::<DownloadPaths>();
                            let mut paths = state.0.lock().map_err(|_| "Download destination is unavailable.".to_string())?;
                            download_destination(&directory, destination, &mut paths)
                        });
                    match path {
                        Ok(path) => *destination = path,
                        Err(error) => {
                            show_message(app, "Download failed", &error, MessageDialogKind::Error);
                            return false;
                        }
                    }
                }
                DownloadEvent::Finished { path, success, .. } => {
                    if success {
                        if let Some(path) = path {
                            show_message(app, "Download complete", &format!("Saved to {}", path.display()), MessageDialogKind::Info);
                        }
                    } else {
                        show_message(app, "Download failed", "The file could not be saved. Try downloading it again.", MessageDialogKind::Error);
                    }
                }
                _ => {}
            }
            true
        })
        .on_page_load(move |window, payload| {
            if reveal_after_load
                && matches!(payload.event(), PageLoadEvent::Finished)
                && let Err(error) = reveal_service_window(&window, &page_load_label)
            {
                eprintln!("failed to reveal {page_load_label} window after page load: {error}");
            }
        })
        .on_new_window(move |requested_url, _features| {
            match validate_popup_url(&popup_origin, &requested_url) {
                Ok(url) => {
                    if let Err(error) =
                        schedule_detached_service_window(popup_app.clone(), url, None, None)
                    {
                        eprintln!("failed to schedule detached Piping popup: {error}");
                    }
                }
                Err(error) => {
                    if let Err(open_error) = open_external_url(requested_url.as_str().to_string()) {
                        eprintln!(
                            "blocked Piping popup request: {error}; external open failed: {open_error}"
                        );
                    }
                }
            }

            NewWindowResponse::Deny
        })
        .build()
        .map_err(|error| format!("failed to create {window_label} webview window: {error}"))?;

    app.state::<ServiceWindows>()
        .0
        .lock()
        .map_err(|_| "service window registry is unavailable".to_string())?
        .insert(window_label.to_string(), instance_url);

    if reveal_after_load {
        schedule_detached_window_reveal_fallback(app.clone(), window_label.to_string())?;
    } else {
        reveal_service_window(&window, window_label)?;
    }
    Ok(())
}

fn reveal_service_window(window: &WebviewWindow, window_label: &str) -> Result<(), String> {
    window
        .show()
        .map_err(|error| format!("failed to show {window_label} window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus {window_label} window: {error}"))
}

fn schedule_detached_window_reveal_fallback(
    app: AppHandle,
    window_label: String,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("piping-detached-window-reveal-fallback".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(
                DETACHED_WINDOW_REVEAL_FALLBACK_MS,
            ));
            let app_for_window = app.clone();
            let label_for_lookup = window_label.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                let Some(window) = app_for_window.get_webview_window(&label_for_lookup) else {
                    return;
                };
                match window.is_visible() {
                    Ok(true) => {}
                    Ok(false) => {
                        if let Err(error) = reveal_service_window(&window, &label_for_lookup) {
                            eprintln!(
                                "failed to reveal {label_for_lookup} window after fallback timeout: {error}"
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!("failed to inspect {label_for_lookup} visibility: {error}");
                    }
                }
            }) {
                eprintln!("failed to schedule detached window reveal fallback: {error}");
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to schedule detached window reveal fallback: {error}"))
}

fn set_local_shell_language(app: &AppHandle, language: &str) {
    let script =
        format!("window.__setDesktopLanguage && window.__setDesktopLanguage({language:?});");
    for (_, webview) in app.webviews() {
        let label = webview.label();
        if label == BOOTSTRAP_WINDOW_LABEL
            || label == MAIN_WINDOW_LABEL
            || label.starts_with("detached-")
        {
            let _ = webview.eval(script.clone());
        }
    }
}

fn last_menu_window_label() -> &'static Mutex<Option<String>> {
    LAST_MENU_WINDOW_LABEL.get_or_init(|| Mutex::new(None))
}

fn remember_menu_window_label(label: &str) {
    if let Ok(mut stored) = last_menu_window_label().lock() {
        *stored = Some(label.to_string());
    }
}

fn open_web_console_for_menu_window(app: &AppHandle) -> Result<(), String> {
    let label = last_menu_window_label()
        .lock()
        .ok()
        .and_then(|stored| stored.clone());
    let candidates = [
        label.as_deref(),
        Some(MAIN_WINDOW_LABEL),
        Some(BOOTSTRAP_WINDOW_LABEL),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Some(window) = app.get_webview_window(candidate) {
            window.open_devtools();
            return Ok(());
        }
    }

    Err("no desktop webview window is available".to_string())
}

fn handle_app_menu_event(app: &AppHandle, id: &str) {
    match id {
        MENU_OPEN_WEB_CONSOLE_ID => {
            if let Err(error) = open_web_console_for_menu_window(app) {
                eprintln!("failed to open web console: {error}");
            }
        }
        MENU_CHECK_UPDATES_ID => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                check_for_updates(app, None).await;
            });
        }
        MENU_ABOUT_ID => show_about(app.clone(), None),
        MENU_CHANGE_SERVICE_ID => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = change_service_address(app).await {
                    eprintln!("failed to open instances from menu: {error}");
                }
            });
        }
        MENU_LANG_EN_ID => set_local_shell_language(app, "en"),
        MENU_LANG_RU_ID => set_local_shell_language(app, "ru"),
        _ => {}
    }
}

fn validate_popup_url(service_url: &Url, requested_url: &Url) -> Result<Url, String> {
    if !same_origin(service_url, requested_url) {
        return Err(format!(
            "{} is outside configured service origin {}",
            requested_url.as_str(),
            service_origin(service_url)
        ));
    }

    Ok(requested_url.clone())
}

fn service_for_window(app: &AppHandle, window: &WebviewWindow) -> Result<Url, String> {
    let registry = app.state::<ServiceWindows>();
    let windows = registry
        .0
        .lock()
        .map_err(|_| "service window registry is unavailable".to_string())?;
    let service = windows
        .get(window.label())
        .ok_or_else(|| "window does not belong to a Piping instance".to_string())?;
    validate_popup_url(service, &window.url().map_err(|error| error.to_string())?)?;
    Ok(service.clone())
}

fn service_profile_key(url: &Url) -> String {
    format!(
        "{:x}",
        Sha256::digest(url.origin().ascii_serialization().as_bytes())
    )
}

fn download_destination(
    directory: &Path,
    suggested: &Path,
    reserved: &mut HashSet<PathBuf>,
) -> Result<PathBuf, String> {
    let name = suggested
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "The download has no file name.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("The Downloads folder could not be opened: {error}"))?;
    let path = directory.join(name);
    if !path.exists() && !reserved.contains(&path) {
        reserved.insert(path.clone());
        return Ok(path);
    }
    let stem = path.file_stem().unwrap_or(name).to_string_lossy();
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..=10_000 {
        let candidate = directory.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() && !reserved.contains(&candidate) {
            reserved.insert(candidate.clone());
            return Ok(candidate);
        }
    }
    Err("Too many files with this name in Downloads. Rename or move an earlier download.".into())
}

#[cfg(target_os = "macos")]
fn service_profile_identifier(url: &Url) -> [u8; 16] {
    let digest = Sha256::digest(url.origin().ascii_serialization().as_bytes());
    let mut identifier = [0; 16];
    identifier.copy_from_slice(&digest[..16]);
    identifier
}

fn normalize_external_web_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("external URL is empty".to_string());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("external URL must not contain whitespace".to_string());
    }

    let url = Url::parse(trimmed).map_err(|error| format!("external URL is invalid: {error}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("external URL must start with http:// or https://".to_string());
    }
    if url.host_str().is_none() {
        return Err("external URL must include a host name or IP address".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("external URL must not include a username or password".to_string());
    }
    Ok(url)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn service_origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupWindow {
    Service,
    Chooser,
}

/// A managed launcher that names the service still opens it directly; an
/// ordinary launch always asks which instance to use.
fn startup_window_mode(has_service_override: bool) -> StartupWindow {
    if has_service_override {
        StartupWindow::Service
    } else {
        StartupWindow::Chooser
    }
}

fn open_startup_window(app: &AppHandle) -> Result<(), String> {
    let override_url = startup_service_url_override()?;
    let mode = startup_window_mode(override_url.is_some());
    if let Some(raw) = override_url.as_deref() {
        let service_url =
            normalize_service_url(raw).map_err(|issue| issue.message(DesktopLanguage::En))?;
        if let Err(issue) = validate_saved_service_url(&service_url) {
            eprintln!(
                "ignoring startup service URL override: {}",
                issue.message(DesktopLanguage::En)
            );
            return open_bootstrap_window(app);
        }
        match read_desktop_config(app) {
            Ok(mut config) => {
                config.remember(service_url.clone());
                write_desktop_config(app, &config)?;
            }
            Err(error) => {
                // The bootstrap window reports the same configuration error in
                // its status line, so a broken config directory stays visible.
                eprintln!("failed to read desktop configuration: {error}");
                return open_bootstrap_window(app);
            }
        }
        if mode == StartupWindow::Service {
            return open_main_window(app, &service_url);
        }
    }
    if let Err(error) = apply_installed_service_url_seed(app) {
        eprintln!("failed to apply the installed service URL seed: {error}");
    }
    open_bootstrap_window(app)
}

/// The installer seed only preselects the address in the chooser; it never
/// bypasses the user's choice on an ordinary launch.
fn apply_installed_service_url_seed(app: &AppHandle) -> Result<(), String> {
    let config = read_desktop_config(app)?;
    if config.service_url.is_some() {
        return Ok(());
    }
    let Some(raw_seed) = read_installed_service_url_seed()? else {
        return Ok(());
    };
    match normalize_service_url(&raw_seed) {
        Ok(service_url) => {
            if let Err(issue) = validate_saved_service_url(&service_url) {
                eprintln!(
                    "ignoring installed service URL seed: {}",
                    issue.message(DesktopLanguage::En)
                );
                return Ok(());
            }
            let mut seeded_config = config;
            seeded_config.remember(service_url);
            write_desktop_config(app, &seeded_config)?;
        }
        Err(error) => {
            eprintln!(
                "ignoring invalid installed service URL seed: {}",
                error.message(DesktopLanguage::En)
            );
        }
    }
    Ok(())
}

fn validate_saved_service_url(service_url: &str) -> Result<(), ServiceUrlIssue> {
    let url = Url::parse(service_url).map_err(|_| ServiceUrlIssue::InvalidUrl)?;
    validate_service_url_endpoint(&url)
}

fn show_message(app: &AppHandle, title: &str, message: &str, kind: MessageDialogKind) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(kind)
        .show(|_| {});
}

fn is_russian(language: &Option<String>) -> bool {
    language
        .as_deref()
        .map(|value| value.to_ascii_lowercase().starts_with("ru"))
        .unwrap_or(false)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure_linux_webkit_runtime();

    tauri::Builder::default()
        .manage(ServiceWindows::default())
        .manage(DownloadPaths::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_app_version,
            get_service_url,
            get_services,
            forget_service,
            get_system_language,
            set_service_url,
            change_service_address,
            show_about,
            check_for_updates,
            minimize_window,
            toggle_maximize_window,
            close_window,
            start_window_drag,
            show_app_menu,
            open_external_url,
            open_detached_window,
            open_app_uri,
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            app_handle.on_menu_event(|app, event| {
                handle_app_menu_event(app, event.id().as_ref());
            });
            // Tauri resolves the legacy bundle-identifier directory exactly as
            // the previous release did, including redirected Windows profiles.
            let legacy_dir = app.path().app_config_dir().ok();
            match desktop_config_dir() {
                Ok(config_dir) => {
                    if let Err(error) =
                        migrate_legacy_desktop_config(&config_dir, legacy_dir.as_deref())
                    {
                        eprintln!("desktop configuration migration failed: {error}");
                        show_message(
                            &app_handle,
                            "Piping",
                            &format!("Desktop configuration could not be migrated: {error}"),
                            MessageDialogKind::Error,
                        );
                    }
                }
                Err(error) => {
                    eprintln!("desktop configuration directory unavailable: {error}");
                    show_message(
                        &app_handle,
                        "Piping",
                        &format!("Desktop configuration directory is unavailable: {error}"),
                        MessageDialogKind::Error,
                    );
                }
            }
            open_startup_window(&app_handle)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(target_os = "linux")]
fn configure_linux_webkit_runtime() {
    // WebKitGTK's DMABUF renderer can stutter on some Mesa/driver/AppImage
    // combinations. Keep this as an overridable default so operators can still
    // A/B test by exporting WEBKIT_DISABLE_DMABUF_RENDERER before launch.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_webkit_runtime() {}

#[cfg(test)]
mod tests {
    #[test]
    fn downloads_stay_in_downloads_and_do_not_replace_existing_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Downloads");
        let mut reserved = std::collections::HashSet::new();
        let first = super::download_destination(
            &directory,
            std::path::Path::new("/untrusted/report.bin"),
            &mut reserved,
        )
        .unwrap();
        assert_eq!(first, directory.join("report.bin"));
        let second = super::download_destination(
            &directory,
            std::path::Path::new("report.bin"),
            &mut reserved,
        )
        .unwrap();
        assert_eq!(second, directory.join("report (1).bin"));
        std::fs::write(&first, b"original").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let third = super::download_destination(
            &directory,
            std::path::Path::new("report.bin"),
            &mut reserved,
        )
        .unwrap();
        assert_eq!(third, directory.join("report (2).bin"));
        assert_eq!(std::fs::read(&first).unwrap(), b"original");
        assert!(
            super::download_destination(&directory, std::path::Path::new("/"), &mut reserved)
                .is_err()
        );
    }

    #[test]
    fn startup_asks_for_an_instance_unless_a_launcher_names_one() {
        assert_eq!(
            super::startup_window_mode(false),
            super::StartupWindow::Chooser
        );
        assert_eq!(
            super::startup_window_mode(true),
            super::StartupWindow::Service
        );
    }

    #[test]
    fn forgetting_one_instance_keeps_the_others_and_their_order() {
        let mut config = super::DesktopConfig::default();
        config.remember("https://first.example".into());
        config.remember("https://second.example".into());
        config.forget("https://first.example");
        assert_eq!(config.services, ["https://second.example"]);
        assert_eq!(
            config.service_url.as_deref(),
            Some("https://second.example")
        );
        config.forget("https://second.example");
        assert!(config.services.is_empty());
        assert_eq!(config.service_url, None);
    }

    #[test]
    fn instance_selection_preserves_saved_instances_without_duplicates() {
        let mut config = super::DesktopConfig::default();
        config.remember("https://first.example".into());
        config.remember("https://second.example".into());
        config.remember("https://first.example".into());
        let restored: super::DesktopConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(
            restored.service_url.as_deref(),
            Some("https://first.example")
        );
        assert_eq!(
            restored.services,
            ["https://first.example", "https://second.example"]
        );
    }

    #[test]
    fn detached_connections_stay_in_their_instance_and_share_its_profile() {
        let first = tauri::Url::parse("https://first.example/").unwrap();
        let connection = first.join("/connections/ssh?session=one").unwrap();
        let second = tauri::Url::parse("https://second.example/").unwrap();
        assert!(super::validate_popup_url(&first, &connection).is_ok());
        assert!(super::validate_popup_url(&second, &connection).is_err());
        assert_eq!(
            super::service_profile_key(&first),
            super::service_profile_key(&connection)
        );
        assert_ne!(
            super::service_profile_key(&first),
            super::service_profile_key(&second)
        );
        let other_port = tauri::Url::parse("https://first.example:8443/").unwrap();
        assert_ne!(
            super::service_profile_key(&first),
            super::service_profile_key(&other_port)
        );
    }

    use super::{
        AppLauncherTarget, ServiceUrlConfirmation, ServiceUrlIssue, ServiceUrlProbe,
        detached_handoff_initialization_script, extract_service_url_arg, launch_local_app_command,
        normalize_app_launcher_target, normalize_external_web_url, normalize_service_url,
        normalize_window_title, probe_service_url_endpoint, validate_service_url_endpoint,
    };
    use std::{
        io::{Read, Write},
        net::{SocketAddr, TcpListener},
        thread,
    };

    use serde_json::json;

    #[test]
    fn app_launcher_accepts_uris_and_local_commands() {
        assert_eq!(
            normalize_app_launcher_target("https://10.20.0.1").unwrap(),
            AppLauncherTarget::Uri("https://10.20.0.1".to_string())
        );
        assert_eq!(
            normalize_app_launcher_target("microsoft-edge:http://10.20.0.1").unwrap(),
            AppLauncherTarget::Uri("microsoft-edge:http://10.20.0.1".to_string())
        );
        assert_eq!(
            normalize_app_launcher_target("notepad").unwrap(),
            AppLauncherTarget::LocalCommand("notepad".to_string())
        );
        assert_eq!(
            normalize_app_launcher_target(r#"C:\Program Files\Vendor\App.exe --host 10.20.0.1"#)
                .unwrap(),
            AppLauncherTarget::LocalCommand(
                r#"C:\Program Files\Vendor\App.exe --host 10.20.0.1"#.to_string()
            )
        );
        assert_eq!(
            normalize_app_launcher_target(r#""/opt/vendor/app" --host 10.20.0.1"#).unwrap(),
            AppLauncherTarget::LocalCommand(r#""/opt/vendor/app" --host 10.20.0.1"#.to_string())
        );
    }

    #[test]
    fn app_launcher_rejects_browser_page_schemes_and_invalid_strict_uris() {
        assert!(normalize_app_launcher_target("javascript:alert(1)").is_err());
        assert!(normalize_app_launcher_target("data:text/html,<script>alert(1)</script>").is_err());
        assert!(normalize_app_launcher_target("https://example.com/a b").is_err());
        assert!(normalize_app_launcher_target("microsoft-edge:file:///etc/passwd").is_err());
    }

    #[test]
    fn service_url_arg_supports_space_and_equals_forms() {
        assert_eq!(
            extract_service_url_arg(vec![
                "--service-url".to_string(),
                "piping.example.com".to_string()
            ])
            .unwrap(),
            Some("piping.example.com".to_string())
        );
        assert_eq!(
            extract_service_url_arg(vec![
                "--piping-service-url=https://piping.example.com".to_string()
            ])
            .unwrap(),
            Some("https://piping.example.com".to_string())
        );
    }

    #[test]
    fn service_url_arg_rejects_missing_values() {
        assert!(extract_service_url_arg(vec!["--service-url".to_string()]).is_err());
        assert!(extract_service_url_arg(vec!["--service-url=".to_string()]).is_err());
    }

    #[test]
    fn service_url_normalization_defaults_to_https() {
        assert_eq!(
            normalize_service_url("piping.example.com/").unwrap(),
            "https://piping.example.com"
        );
    }

    fn spawn_probe_server<F>(
        request_count: usize,
        responder: F,
    ) -> (SocketAddr, thread::JoinHandle<()>)
    where
        F: Fn(&str) -> (u16, &'static str, String) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let bytes_read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..bytes_read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, content_type, body) = responder(path);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (address, server)
    }

    fn piping_probe_response(path: &str) -> (u16, &'static str, String) {
        match path {
            super::SERVICE_HEALTH_PATH => (
                200,
                "application/json",
                r#"{"status":"ok"}"#.to_string(),
            ),
            super::SERVICE_AUTH_PROVIDERS_PATH => (
                200,
                "application/json",
                r#"{"local_enabled":true,"oidc_enabled":false,"ldap_enabled":false,"ad_enabled":false}"#
                    .to_string(),
            ),
            _ => (404, "text/plain", "not found".to_string()),
        }
    }

    #[test]
    fn service_url_endpoint_accepts_piping_api_fingerprint() {
        let (address, server) = spawn_probe_server(2, piping_probe_response);

        let url = tauri::Url::parse(&format!("http://{address}")).unwrap();
        validate_service_url_endpoint(&url).unwrap();

        server.join().unwrap();
    }

    #[test]
    fn service_probe_does_not_retry_non_certificate_failures_insecurely() {
        let url = tauri::Url::parse("https://piping.example").unwrap();
        for issue in [
            ServiceUrlIssue::CannotConnect {
                address: "https://piping.example".into(),
            },
            ServiceUrlIssue::NotPipingService {
                address: "https://piping.example".into(),
            },
        ] {
            let result = super::resolve_service_probe(
                &url,
                Err(issue.clone().into()),
                || panic!("non-certificate failure must not trigger an insecure probe"),
                || None,
            );
            assert_eq!(result, Err(issue));
        }
        assert_eq!(
            super::resolve_service_probe(
                &url,
                Ok(()),
                || panic!("unexpected probe"),
                || panic!("unexpected probe")
            ),
            Ok(ServiceUrlProbe::Accepted)
        );
    }

    #[test]
    fn certificate_validation_errors_require_successful_probe_and_confirmation() {
        for certificate_error in [
            rustls::CertificateError::UnknownIssuer,
            rustls::CertificateError::NotValidForName,
        ] {
            let error = std::io::Error::other(rustls::Error::InvalidCertificate(certificate_error));
            assert!(super::is_certificate_validation_error(&error));
        }
        assert!(!super::is_certificate_validation_error(
            &std::io::Error::other("network error")
        ));
        let url = tauri::Url::parse("https://piping.example").unwrap();
        let failure = || super::ServiceProbeError {
            issue: ServiceUrlIssue::CannotConnect {
                address: "https://piping.example".into(),
            },
            invalid_certificate: true,
        };
        assert_eq!(
            super::resolve_service_probe(&url, Err(failure()), || true, || None),
            Ok(ServiceUrlProbe::RequiresConfirmation(
                ServiceUrlConfirmation::InvalidCertificate {
                    service_url: "https://piping.example".into()
                }
            ))
        );
        assert_eq!(
            super::resolve_service_probe(&url, Err(failure()), || false, || None),
            Err(failure().issue)
        );
    }

    #[test]
    fn service_probe_client_supports_permissive_tls_mode() {
        assert!(
            super::build_service_probe_client(
                super::ServiceProbeTlsMode::Strict,
                "https://example.com",
            )
            .is_ok()
        );
        assert!(
            super::build_service_probe_client(
                super::ServiceProbeTlsMode::AcceptInvalidCertificates,
                "https://example.com",
            )
            .is_ok()
        );
    }

    #[test]
    fn service_url_endpoint_rejects_plain_html_site() {
        let (address, server) = spawn_probe_server(1, |_| {
            (
                200,
                "text/html",
                "<!doctype html><title>Piping landing</title>".to_string(),
            )
        });

        let url = tauri::Url::parse(&format!("http://{address}")).unwrap();
        let issue = validate_service_url_endpoint(&url).unwrap_err();
        assert_eq!(
            issue,
            ServiceUrlIssue::NotPipingService {
                address: format!("http://{address}")
            }
        );

        server.join().unwrap();
    }

    #[test]
    fn service_url_endpoint_rejects_generic_health_endpoint() {
        let (address, server) = spawn_probe_server(2, |path| match path {
            super::SERVICE_HEALTH_PATH => {
                (200, "application/json", r#"{"status":"ok"}"#.to_string())
            }
            _ => (
                200,
                "text/html",
                "<!doctype html><title>Login</title>".to_string(),
            ),
        });

        let url = tauri::Url::parse(&format!("http://{address}")).unwrap();
        let issue = validate_service_url_endpoint(&url).unwrap_err();
        assert_eq!(
            issue,
            ServiceUrlIssue::NotPipingService {
                address: format!("http://{address}")
            }
        );

        server.join().unwrap();
    }

    #[test]
    fn service_url_endpoint_rejects_https_when_plain_http_answers() {
        let (address, server) = spawn_probe_server(3, piping_probe_response);

        let url = tauri::Url::parse(&format!("https://{address}")).unwrap();
        let probe = probe_service_url_endpoint(&url).unwrap();
        assert_eq!(
            probe,
            ServiceUrlProbe::RequiresConfirmation(ServiceUrlConfirmation::PlainHttpEndpoint {
                suggested_url: format!("http://{address}")
            })
        );

        server.join().unwrap();
    }

    #[test]
    fn http_variant_suggestion_allows_dns_names() {
        assert!(super::allows_http_variant_suggestion(
            &tauri::Url::parse("https://127.0.0.1:8443").unwrap()
        ));
        assert!(super::allows_http_variant_suggestion(
            &tauri::Url::parse("https://10.51.7.115").unwrap()
        ));
        assert!(super::allows_http_variant_suggestion(
            &tauri::Url::parse("https://piping.local").unwrap()
        ));
        assert!(super::allows_http_variant_suggestion(
            &tauri::Url::parse("https://piping").unwrap()
        ));
        assert!(super::allows_http_variant_suggestion(
            &tauri::Url::parse("https://piping.space").unwrap()
        ));
        assert!(super::allows_http_variant_suggestion(
            &tauri::Url::parse("https://example.com").unwrap()
        ));
    }

    #[test]
    fn external_url_normalization_accepts_only_plain_web_urls() {
        assert_eq!(
            normalize_external_web_url("https://github.com/mlimarenko/piping")
                .unwrap()
                .as_str(),
            "https://github.com/mlimarenko/piping"
        );
        assert!(normalize_external_web_url("piping://local").is_err());
        assert!(normalize_external_web_url("file:///etc/passwd").is_err());
        assert!(normalize_external_web_url("https://user:pass@example.com").is_err());
    }

    #[test]
    fn window_title_includes_context_and_app_name() {
        assert_eq!(
            normalize_window_title(Some("SSH · cash-1".to_string())),
            "SSH · cash-1 | Piping"
        );
        assert_eq!(normalize_window_title(Some(" \n\t".to_string())), "Piping");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_config_home_prefers_absolute_xdg_config_home() {
        assert_eq!(
            super::linux_config_home(Some("/tmp/xdg".into()), Some("/home/user".into())).unwrap(),
            std::path::PathBuf::from("/tmp/xdg")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_config_home_falls_back_to_home_config() {
        assert_eq!(
            super::linux_config_home(Some("relative".into()), Some("/home/user".into())).unwrap(),
            std::path::PathBuf::from("/home/user/.config")
        );
    }

    #[test]
    fn detached_handoff_is_confined_to_the_instance_top_frame() {
        let service = tauri::Url::parse("https://instance.example/").unwrap();
        let handoff = json!({
            "sessionId": "session-1",
            "hostName": "Cash \"A\"",
            "protocol": "ssh",
            "token": "token-1",
        });
        let script = detached_handoff_initialization_script(&service, Some(handoff.clone()))
            .unwrap()
            .unwrap();
        for (origin, top_level, allowed) in [
            ("https://instance.example", true, true),
            ("https://identity.example", true, false),
            ("https://instance.example:8443", true, false),
            ("https://instance.example", false, false),
        ] {
            let origin = serde_json::to_string(origin).unwrap();
            let top = if top_level { "window" } else { "{}" };
            let program = format!(
                "globalThis.window = {{ location: {{ origin: {origin} }} }}; window.top = {top};\n{script}\nconsole.log(JSON.stringify(window.__PIPING_DETACHED_HANDOFF__ ?? null));"
            );
            let output = std::process::Command::new("node")
                .arg("-e")
                .arg(program)
                .output()
                .expect("desktop tests require Node.js");
            assert!(output.status.success());
            let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                actual,
                if allowed {
                    handoff.clone()
                } else {
                    serde_json::Value::Null
                }
            );
        }
    }

    #[test]
    fn detached_handoff_script_rejects_oversized_payload() {
        let payload = json!({ "token": "x".repeat(super::DETACHED_HANDOFF_MAX_BYTES) });
        let service = tauri::Url::parse("https://instance.example/").unwrap();
        assert!(detached_handoff_initialization_script(&service, Some(payload)).is_err());
    }

    #[test]
    fn config_directory_has_the_same_shape_on_every_platform() {
        let text = |value: &str| Some(std::ffi::OsString::from(value));
        assert_eq!(
            super::desktop_config_dir_from(
                None,
                super::DesktopPlatform::Linux,
                text("/xdg"),
                text("/home/user"),
                None,
                None
            )
            .unwrap(),
            std::path::PathBuf::from("/xdg/piping")
        );
        assert_eq!(
            super::desktop_config_dir_from(
                None,
                super::DesktopPlatform::Linux,
                text("relative-xdg"),
                text("/home/user"),
                None,
                None
            )
            .unwrap(),
            std::path::PathBuf::from("/home/user/.config/piping")
        );
        assert_eq!(
            super::desktop_config_dir_from(
                None,
                super::DesktopPlatform::MacOs,
                None,
                text("/Users/user"),
                None,
                None
            )
            .unwrap(),
            std::path::PathBuf::from("/Users/user/Library/Application Support/piping")
        );
        assert_eq!(
            super::desktop_config_dir_from(
                None,
                super::DesktopPlatform::Windows,
                None,
                text("C:\\Users\\user"),
                text("C:\\Users\\user\\AppData\\Roaming"),
                None
            )
            .unwrap(),
            std::path::PathBuf::from("C:\\Users\\user\\AppData\\Roaming").join("piping")
        );
        assert_eq!(
            super::desktop_config_dir_from(
                None,
                super::DesktopPlatform::Windows,
                None,
                None,
                None,
                text("C:\\Users\\user")
            )
            .unwrap(),
            std::path::PathBuf::from("C:\\Users\\user")
                .join("AppData")
                .join("Roaming")
                .join("piping")
        );
    }

    #[test]
    fn config_directory_override_must_be_absolute() {
        let text = |value: &str| Some(std::ffi::OsString::from(value));
        assert_eq!(
            super::desktop_config_dir_from(
                text("/srv/piping"),
                super::DesktopPlatform::Linux,
                None,
                text("/home/user"),
                None,
                None
            )
            .unwrap(),
            std::path::PathBuf::from("/srv/piping")
        );
        assert!(
            super::desktop_config_dir_from(
                text("relative/piping"),
                super::DesktopPlatform::Linux,
                None,
                text("/home/user"),
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn migration_renames_the_legacy_file_and_keeps_profiles() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("piping");
        std::fs::create_dir_all(config_dir.join("webviews").join("origin")).unwrap();
        std::fs::write(
            config_dir.join("desktop-config.json"),
            r#"{"serviceUrl":"https://a.example"}"#,
        )
        .unwrap();
        std::fs::write(
            config_dir.join("webviews").join("origin").join("cookie"),
            "secret",
        )
        .unwrap();

        assert!(super::migrate_legacy_desktop_config(&config_dir, None).unwrap());
        assert!(!config_dir.join("desktop-config.json").exists());
        let migrated: super::DesktopConfig =
            serde_json::from_str(&std::fs::read_to_string(config_dir.join("app.json")).unwrap())
                .unwrap();
        assert_eq!(migrated.service_url.as_deref(), Some("https://a.example"));
        assert_eq!(migrated.services, ["https://a.example"]);
        assert_eq!(
            std::fs::read_to_string(config_dir.join("webviews").join("origin").join("cookie"))
                .unwrap(),
            "secret"
        );
        assert!(!super::migrate_legacy_desktop_config(&config_dir, None).unwrap());
    }

    #[test]
    fn migration_moves_the_bundle_identifier_directory_with_profiles() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("space.piping.desktop");
        std::fs::create_dir_all(legacy.join("webviews").join("origin")).unwrap();
        std::fs::write(
            legacy.join("desktop-config.json"),
            r#"{"services":["https://b.example"]}"#,
        )
        .unwrap();
        std::fs::write(
            legacy.join("webviews").join("origin").join("cookie"),
            "kept",
        )
        .unwrap();
        let config_dir = root.path().join("piping");

        assert!(super::migrate_legacy_desktop_config(&config_dir, Some(&legacy)).unwrap());
        assert!(!legacy.exists());
        let migrated: super::DesktopConfig =
            serde_json::from_str(&std::fs::read_to_string(config_dir.join("app.json")).unwrap())
                .unwrap();
        assert_eq!(migrated.services, ["https://b.example"]);
        assert_eq!(
            std::fs::read_to_string(config_dir.join("webviews").join("origin").join("cookie"))
                .unwrap(),
            "kept"
        );
    }

    #[test]
    fn migration_imports_legacy_instances_beside_an_existing_app_json() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("space.piping.desktop");
        let config_dir = root.path().join("piping");
        std::fs::create_dir_all(legacy.join("webviews").join("old")).unwrap();
        std::fs::create_dir_all(config_dir.join("webviews").join("new")).unwrap();
        std::fs::write(
            legacy.join("desktop-config.json"),
            r#"{"serviceUrl":"https://old.example","services":["https://old.example","https://new.example"]}"#,
        )
        .unwrap();
        std::fs::write(legacy.join("webviews").join("old").join("cookie"), "old").unwrap();
        std::fs::write(
            config_dir.join("webviews").join("new").join("cookie"),
            "new",
        )
        .unwrap();
        std::fs::write(
            config_dir.join("app.json"),
            r#"{"serviceUrl":"https://new.example","services":["https://new.example"]}"#,
        )
        .unwrap();

        assert!(super::migrate_legacy_desktop_config(&config_dir, Some(&legacy)).unwrap());
        assert!(!legacy.exists());
        let migrated: super::DesktopConfig =
            serde_json::from_str(&std::fs::read_to_string(config_dir.join("app.json")).unwrap())
                .unwrap();
        assert_eq!(migrated.service_url.as_deref(), Some("https://new.example"));
        assert_eq!(
            migrated.services,
            ["https://new.example", "https://old.example"]
        );
        assert_eq!(
            std::fs::read_to_string(config_dir.join("webviews").join("old").join("cookie"))
                .unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(config_dir.join("webviews").join("new").join("cookie"))
                .unwrap(),
            "new"
        );
    }

    #[test]
    fn migration_quarantines_a_conflicting_profile_instead_of_mixing_or_deleting_it() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("space.piping.desktop");
        let config_dir = root.path().join("piping");
        std::fs::create_dir_all(legacy.join("webviews").join("same")).unwrap();
        std::fs::create_dir_all(config_dir.join("webviews").join("same")).unwrap();
        std::fs::write(legacy.join("webviews").join("same").join("Cookies"), "old").unwrap();
        std::fs::write(legacy.join("webviews").join("same").join("extra"), "legacy").unwrap();
        std::fs::write(
            config_dir.join("webviews").join("same").join("Cookies"),
            "new",
        )
        .unwrap();
        std::fs::write(
            legacy.join("desktop-config.json"),
            r#"{"services":["https://old.example"]}"#,
        )
        .unwrap();
        std::fs::write(
            config_dir.join("app.json"),
            r#"{"serviceUrl":"https://new.example","services":["https://new.example"]}"#,
        )
        .unwrap();

        assert!(super::migrate_legacy_desktop_config(&config_dir, Some(&legacy)).unwrap());
        assert!(!legacy.exists());
        assert_eq!(
            std::fs::read_to_string(config_dir.join("webviews").join("same").join("Cookies"))
                .unwrap(),
            "new"
        );
        let quarantined: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("space.piping.desktop.legacy-")
                })
            })
            .collect();
        assert_eq!(
            quarantined.len(),
            1,
            "conflicting legacy data must be preserved"
        );
        assert_eq!(
            std::fs::read_to_string(quarantined[0].join("webviews").join("same").join("Cookies"))
                .unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(quarantined[0].join("webviews").join("same").join("extra"))
                .unwrap(),
            "legacy"
        );
        let migrated: super::DesktopConfig =
            serde_json::from_str(&std::fs::read_to_string(config_dir.join("app.json")).unwrap())
                .unwrap();
        assert_eq!(
            migrated.services,
            ["https://new.example", "https://old.example"]
        );
    }

    #[test]
    #[ignore = "requires a local desktop application; set PIPING_DESKTOP_APP_LAUNCH_SMOKE_CMD"]
    fn app_launcher_launches_local_command_from_env() {
        let Ok(command) = std::env::var("PIPING_DESKTOP_APP_LAUNCH_SMOKE_CMD") else {
            eprintln!("PIPING_DESKTOP_APP_LAUNCH_SMOKE_CMD is not set; skipping");
            return;
        };

        launch_local_app_command(&command).expect("local app command should launch");
        if let Ok(wait_ms) = std::env::var("PIPING_DESKTOP_APP_LAUNCH_SMOKE_WAIT_MS") {
            let wait_ms = wait_ms
                .parse::<u64>()
                .expect("PIPING_DESKTOP_APP_LAUNCH_SMOKE_WAIT_MS must be an integer");
            std::thread::sleep(std::time::Duration::from_millis(wait_ms));
        }
    }
}
