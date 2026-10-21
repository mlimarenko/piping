use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::menu::MenuBuilder;
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, LogicalPosition, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    Window,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const DESKTOP_CONFIG_FILE: &str = "desktop-config.json";
const DESKTOP_SERVICE_URL_SEED_FILE: &str = "desktop-service-url.txt";
const LINUX_CONFIG_DIR_NAME: &str = "piping";
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
const SERVICE_CHROME_SCRIPT: &str = include_str!("../../ui/service-chrome.js");

static DETACHED_WINDOW_COUNTER: AtomicUsize = AtomicUsize::new(1);
static LAST_MENU_WINDOW_LABEL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopConfig {
    service_url: Option<String>,
}

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
    write_desktop_config(
        &app,
        &DesktopConfig {
            service_url: Some(service_url.clone()),
        },
    )?;
    schedule_service_window_transition(app, window.label().to_string(), service_url.clone())?;
    Ok(service_url)
}

#[tauri::command]
fn change_service_address(app: AppHandle) -> Result<(), String> {
    reset_service_address(&app)
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
    let (title, current_prefix, available_prefix, unavailable_message, current_message) = if russian
    {
        (
            "Проверка обновлений",
            "Текущая версия",
            "Доступна версия",
            "Канал обновлений пока не настроен для этой сборки.",
            "Установлена актуальная версия.",
        )
    } else {
        (
            "Update check",
            "Current version",
            "Version available",
            "The update channel is not configured for this build yet.",
            "You are running the latest version.",
        )
    };

    let message = match app.updater() {
        Ok(updater) => match updater.check().await {
            Ok(Some(update)) => format!(
                "{available_prefix}: {}\n{current_prefix}: {}",
                update.version, update.current_version
            ),
            Ok(None) => format!(
                "{current_message}\n{current_prefix}: {}",
                env!("CARGO_PKG_VERSION")
            ),
            Err(error) => format!("{unavailable_message}\n\n{error}"),
        },
        Err(error) => format!("{unavailable_message}\n\n{error}"),
    };

    show_message(&app, title, &message, MessageDialogKind::Info);
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
            .text(MENU_CHANGE_SERVICE_ID, "Адрес сервиса")
            .build()
    } else {
        MenuBuilder::new(&app)
            .text(MENU_LANG_EN_ID, "English")
            .text(MENU_LANG_RU_ID, "Русский")
            .separator()
            .text(MENU_OPEN_WEB_CONSOLE_ID, "Open web console")
            .text(MENU_CHECK_UPDATES_ID, "Check updates")
            .text(MENU_ABOUT_ID, "About")
            .text(MENU_CHANGE_SERVICE_ID, "Change address")
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
    path: String,
    handoff: Option<serde_json::Value>,
    title: Option<String>,
) -> Result<(), String> {
    let detached_path = normalize_detached_route_path(&path)?;
    let handoff_script = detached_handoff_initialization_script(handoff)?;
    let window_title = normalize_window_title(title);
    let config = read_desktop_config(&app)?;
    let service_url = config
        .service_url
        .ok_or_else(|| "service URL is not configured".to_string())?;
    let service_url = Url::parse(&service_url)
        .map_err(|error| format!("saved service URL is invalid: {error}"))?;
    let detached_url = service_url
        .join(&detached_path)
        .map_err(|error| format!("failed to build detached window URL: {error}"))?;

    schedule_detached_service_window(app, detached_url, handoff_script, Some(window_title))?;
    Ok(())
}

#[tauri::command]
async fn open_app_uri(app: AppHandle, uri: String) -> Result<(), String> {
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

fn reset_service_address(app: &AppHandle) -> Result<(), String> {
    write_desktop_config(app, &DesktopConfig::default())?;
    open_bootstrap_window(app)?;

    for (label, window) in app.windows() {
        if label != BOOTSTRAP_WINDOW_LABEL {
            window
                .close()
                .map_err(|error| format!("failed to close {label} window: {error}"))?;
        }
    }

    Ok(())
}

fn desktop_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = desktop_config_dir(app)?;
    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("failed to create desktop configuration directory: {error}"))?;
    Ok(config_dir.join(DESKTOP_CONFIG_FILE))
}

fn desktop_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    if let Some(config_home) = linux_config_home(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    ) {
        return Ok(config_home.join(LINUX_CONFIG_DIR_NAME));
    }

    app.path()
        .app_config_dir()
        .map_err(|error| format!("failed to resolve desktop configuration directory: {error}"))
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

fn read_desktop_config(app: &AppHandle) -> Result<DesktopConfig, String> {
    let path = desktop_config_path(app)?;
    if !path.exists() {
        return Ok(DesktopConfig::default());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read desktop configuration: {error}"))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse desktop configuration: {error}"))
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

fn write_desktop_config(app: &AppHandle, config: &DesktopConfig) -> Result<(), String> {
    let path = desktop_config_path(app)?;
    let raw = serde_json::to_string_pretty(config)
        .map_err(|error| format!("failed to encode desktop configuration: {error}"))?;
    fs::write(path, raw).map_err(|error| format!("failed to write desktop configuration: {error}"))
}

fn schedule_service_window_transition(
    app: AppHandle,
    source_window_label: String,
    service_url: String,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("piping-service-window-transition".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(
                SERVICE_WINDOW_TRANSITION_DELAY_MS,
            ));
            let app_for_main = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                if let Err(error) = open_configured_service_window(
                    &app_for_main,
                    &source_window_label,
                    &service_url,
                ) {
                    eprintln!("failed to open configured service window: {error}");
                }
            }) {
                eprintln!("failed to schedule configured service window: {error}");
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to schedule service window transition: {error}"))
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
            let app_for_window = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                if let Err(error) = open_detached_service_window(
                    &app_for_window,
                    url,
                    extra_initialization_script,
                    title,
                ) {
                    eprintln!("failed to open detached Piping window: {error}");
                }
            }) {
                eprintln!("failed to schedule detached Piping window: {error}");
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

    match validate_piping_service_api(&client, url) {
        Ok(()) => Ok(ServiceUrlProbe::Accepted),
        Err(_error) => {
            if url.scheme() == "https"
                && let Ok(insecure_client) = build_service_probe_client(
                    ServiceProbeTlsMode::AcceptInvalidCertificates,
                    &address,
                )
                && validate_piping_service_api(&insecure_client, url).is_ok()
            {
                return Ok(ServiceUrlProbe::RequiresConfirmation(
                    ServiceUrlConfirmation::InvalidCertificate {
                        service_url: url.as_str().trim_end_matches('/').to_string(),
                    },
                ));
            }
            if url.scheme() == "https"
                && let Some(suggested_url) = reachable_http_variant(&client, url)
            {
                return Ok(ServiceUrlProbe::RequiresConfirmation(
                    ServiceUrlConfirmation::PlainHttpEndpoint { suggested_url },
                ));
            }
            Err(_error)
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
) -> Result<(), ServiceUrlIssue> {
    let address = service_origin(url);
    validate_piping_health_endpoint(client, url, &address)?;
    validate_piping_auth_providers_endpoint(client, url, &address)?;
    Ok(())
}

fn validate_piping_health_endpoint(
    client: &reqwest::blocking::Client,
    url: &Url,
    address: &str,
) -> Result<(), ServiceUrlIssue> {
    let health_url = service_api_url(url, SERVICE_HEALTH_PATH)?;
    let response =
        client
            .get(health_url.as_str())
            .send()
            .map_err(|_| ServiceUrlIssue::CannotConnect {
                address: address.to_string(),
            })?;
    if !response.status().is_success() {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        });
    }

    let payload = read_probe_json_response(response, address)?;
    if payload.get("status").and_then(|value| value.as_str()) == Some("ok") {
        Ok(())
    } else {
        Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        })
    }
}

fn validate_piping_auth_providers_endpoint(
    client: &reqwest::blocking::Client,
    url: &Url,
    address: &str,
) -> Result<(), ServiceUrlIssue> {
    let providers_url = service_api_url(url, SERVICE_AUTH_PROVIDERS_PATH)?;
    let response =
        client
            .get(providers_url.as_str())
            .send()
            .map_err(|_| ServiceUrlIssue::CannotConnect {
                address: address.to_string(),
            })?;
    if !response.status().is_success() {
        return Err(ServiceUrlIssue::NotPipingService {
            address: address.to_string(),
        });
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
        })
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
    Ok(Some(format!(
        r#"Object.defineProperty(window, "{DETACHED_HANDOFF_GLOBAL}", {{ value: {serialized}, writable: true, configurable: true }});"#
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

fn build_bootstrap_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    WebviewWindowBuilder::new(app, BOOTSTRAP_WINDOW_LABEL, WebviewUrl::default())
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
    if let Some(window) = app.get_window(MAIN_WINDOW_LABEL) {
        window
            .set_focus()
            .map_err(|error| format!("failed to focus main window: {error}"))?;
        return Ok(());
    }

    let url = Url::parse(service_url)
        .map_err(|error| format!("failed to parse saved service URL: {error}"))?;
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
    let page_load_label = window_label.to_string();
    let initialization_script = match extra_initialization_script {
        Some(extra) => format!("{SERVICE_CHROME_SCRIPT}\n;{extra}"),
        None => SERVICE_CHROME_SCRIPT.to_string(),
    };
    let window = WebviewWindowBuilder::new(app, window_label, WebviewUrl::External(url))
        .title(title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 620.0)
        .resizable(true)
        .decorations(false)
        .visible(!reveal_after_load)
        .initialization_script(initialization_script.as_str())
        .on_page_load(move |window, payload| {
            if reveal_after_load
                && matches!(payload.event(), PageLoadEvent::Finished)
                && let Err(error) = reveal_service_window(&window, &page_load_label)
            {
                eprintln!("failed to reveal {page_load_label} window after page load: {error}");
            }
        })
        .on_new_window(move |requested_url, _features| {
            match validate_popup_url(&popup_app, &requested_url) {
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
            if let Err(error) = reset_service_address(app) {
                eprintln!("failed to reset service address from menu: {error}");
            }
        }
        MENU_LANG_EN_ID => set_local_shell_language(app, "en"),
        MENU_LANG_RU_ID => set_local_shell_language(app, "ru"),
        _ => {}
    }
}

fn validate_popup_url(app: &AppHandle, requested_url: &Url) -> Result<Url, String> {
    let config = read_desktop_config(app)?;
    let service_url = config
        .service_url
        .ok_or_else(|| "service URL is not configured".to_string())?;
    let service_url = Url::parse(&service_url)
        .map_err(|error| format!("saved service URL is invalid: {error}"))?;

    if !same_origin(&service_url, requested_url) {
        return Err(format!(
            "{} is outside configured service origin {}",
            requested_url.as_str(),
            service_origin(&service_url)
        ));
    }

    Ok(requested_url.clone())
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
    match url.port() {
        Some(port) => format!("{}://{}:{port}", url.scheme(), url.host_str().unwrap_or("")),
        None => format!("{}://{}", url.scheme(), url.host_str().unwrap_or("")),
    }
}

fn open_startup_window(app: &AppHandle) -> Result<(), String> {
    let config = resolve_startup_config(app)?;
    match config.service_url {
        Some(service_url) => open_main_window(app, &service_url),
        None => open_bootstrap_window(app),
    }
}

fn resolve_startup_config(app: &AppHandle) -> Result<DesktopConfig, String> {
    if let Some(raw) = startup_service_url_override()? {
        let service_url =
            normalize_service_url(&raw).map_err(|issue| issue.message(DesktopLanguage::En))?;
        if let Err(issue) = validate_saved_service_url(&service_url) {
            eprintln!(
                "ignoring startup service URL override: {}",
                issue.message(DesktopLanguage::En)
            );
            return Ok(DesktopConfig::default());
        }
        let config = DesktopConfig {
            service_url: Some(service_url),
        };
        write_desktop_config(app, &config)?;
        return Ok(config);
    }

    let config = read_desktop_config(app)?;
    if let Some(service_url) = config.service_url.as_deref() {
        if let Err(issue) = validate_saved_service_url(service_url) {
            eprintln!(
                "saved service URL is not opened: {}",
                issue.message(DesktopLanguage::En)
            );
            return Ok(DesktopConfig::default());
        }
        return Ok(config);
    }

    let Some(raw_seed) = read_installed_service_url_seed()? else {
        return Ok(config);
    };

    match normalize_service_url(&raw_seed) {
        Ok(service_url) => {
            if let Err(issue) = validate_saved_service_url(&service_url) {
                eprintln!(
                    "ignoring installed service URL seed: {}",
                    issue.message(DesktopLanguage::En)
                );
                return Ok(config);
            }
            let seeded_config = DesktopConfig {
                service_url: Some(service_url),
            };
            write_desktop_config(app, &seeded_config)?;
            Ok(seeded_config)
        }
        Err(error) => {
            eprintln!(
                "ignoring invalid installed service URL seed: {}",
                error.message(DesktopLanguage::En)
            );
            Ok(config)
        }
    }
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_app_version,
            get_service_url,
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
        let (address, server) = spawn_probe_server(4, piping_probe_response);

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
    fn detached_handoff_script_injects_json_value() {
        let script = detached_handoff_initialization_script(Some(json!({
            "sessionId": "session-1",
            "hostName": "Cash \"A\"",
            "protocol": "ssh",
            "token": "token-1",
        })))
        .unwrap()
        .unwrap();

        assert!(script.contains("__PIPING_DETACHED_HANDOFF__"));
        assert!(script.contains(r#""sessionId":"session-1""#));
        assert!(script.contains(r#""hostName":"Cash \"A\"""#));
        assert!(!script.contains("token-1\";"));
    }

    #[test]
    fn detached_handoff_script_rejects_oversized_payload() {
        let payload = json!({ "token": "x".repeat(super::DETACHED_HANDOFF_MAX_BYTES) });
        assert!(detached_handoff_initialization_script(Some(payload)).is_err());
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
