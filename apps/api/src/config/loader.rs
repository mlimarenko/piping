use std::error::Error;

use config::{Config, Environment, File};

use super::Settings;

/// Environment source for `PIPING_*` variables.
///
/// `Vec<String>` settings (e.g. `server.cors_allowed_origins`,
/// `auth.oidc_scopes`) are populated from a comma-separated env value —
/// `PIPING_SERVER__CORS_ALLOWED_ORIGINS=https://a.example,https://b.example`.
/// The list separator is scoped to the known list keys so ordinary string
/// settings that legitimately contain commas are never split.
fn piping_env_source() -> Environment {
    Environment::with_prefix("PIPING")
        .prefix_separator("_")
        .separator("__")
        .try_parsing(true)
        .list_separator(",")
        .with_list_parse_key("server.cors_allowed_origins")
        .with_list_parse_key("auth.oidc_scopes")
}

/// Load settings from config files and environment variables
///
/// Loads configuration in the following order (later sources override earlier):
/// 1. `config/default.toml` (base configuration)
/// 2. `config/local.toml` (optional, for local overrides)
/// 3. Environment variables with `PIPING_` prefix and `__` separator
///
/// Environment variables are converted from `PIPING_DATABASE__URL` to `database.url`,
/// `PIPING_REDIS__URL` to `redis.url`, etc.
/// Use double underscores between nesting levels so single underscores remain in keys,
/// e.g. `PIPING_AUTH__LOCAL_ENABLED` maps to `auth.local_enabled`.
pub fn load_settings() -> Result<Settings, Box<dyn Error + Send + Sync>> {
    let builder = Config::builder()
        .add_source(File::with_name("config/default").required(true))
        .add_source(File::with_name("config/local").required(false))
        .add_source(piping_env_source());

    let config = builder.build()?;

    let settings: Settings = config.try_deserialize()?;

    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(serde::Deserialize)]
    struct ServerOnly {
        server: CorsOnly,
    }

    #[derive(serde::Deserialize)]
    struct CorsOnly {
        cors_allowed_origins: Vec<String>,
    }

    #[test]
    fn cors_allowed_origins_parse_from_comma_separated_env() {
        let source = piping_env_source().source(Some(HashMap::from([(
            "PIPING_SERVER__CORS_ALLOWED_ORIGINS".to_string(),
            "https://a.example,https://b.example".to_string(),
        )])));
        let config = Config::builder()
            .add_source(source)
            .build()
            .expect("build config");
        let parsed: ServerOnly = config.try_deserialize().expect("deserialize");
        assert_eq!(
            parsed.server.cors_allowed_origins,
            vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ],
            "a comma-separated env value must populate the CORS origin list so \
             env-driven deployments can restrict origins"
        );
    }
}
