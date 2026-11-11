//! Explicit offline operation; never runs migrations or starts the backend.

use piping_backend::contexts::inventory::infrastructure::persistence::repositories::ipam::fhrp_auth_key_conversion::convert_fhrp_auth_keys_offline;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        != [
            "--offline-confirmed",
            "--backup-confirmed",
            "--convert-fhrp-auth-keys",
        ]
    {
        eprintln!(
            "Requires --offline-confirmed --backup-confirmed --convert-fhrp-auth-keys; stop every writer and verify a private backup first. Database and cipher configuration must match the backend; never pass secrets in argv."
        );
        return std::process::ExitCode::FAILURE;
    }
    let result = async {
        // An explicit target is required, but resolve it and the cipher reference
        // through the backend's configuration owner. Using the default cipher
        // reference here could encrypt with a different key from the installation.
        std::env::var("PIPING_DATABASE__URL").map_err(|_| ())?;
        let settings = piping_backend::config::load_settings().map_err(|_| ())?;
        piping_backend::contexts::secrets::infrastructure::crypto::configure_cipher_key_material_ref(
            &settings.secrets.cipher.key_material_ref,
        ).map_err(|_| ())?;
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&settings.database.url)
            .await
            .map_err(|_| ())?;
        convert_fhrp_auth_keys_offline(&pool).await.map_err(|_| ())
    }
    .await;
    match result {
        Ok(count) => {
            println!(
                "Converted {count} FHRP auth keys atomically; plaintext cleared. Apply migration 0004 before restarting writers."
            );
            std::process::ExitCode::SUCCESS
        }
        Err(()) => {
            // Deliberately do not print database/config errors: they may embed
            // connection details or bind values. The transaction fails closed.
            eprintln!(
                "FHRP conversion failed; no conversion transaction was committed. Check private configuration, legacy key validity and conflicting bindings."
            );
            std::process::ExitCode::FAILURE
        }
    }
}
