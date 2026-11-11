//! Inventory transfer entrypoint.
//!
//! Snapshot export and assessment (`capture-http`, `preflight`, `dry-run`) do not
//! mutate inventory. Descriptor preparation only observes inventory, but its
//! canonical authentication service refreshes token usage metadata.
//! `apply`/`resume` are the only commands that write
//! inventory, and they write only through the canonical typed writer under a
//! pinned descriptor. No server bootstrap, settings load, migration, seed,
//! scheduler or NetBox client is constructed for a mutation command.
//!
//! Every diagnostic is a static code: no SQL, HTTP, provider value, path or
//! credential text is formatted into the report or stderr.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use piping_backend::contexts::sync::application::inventory_preflight::{dry_run, preflight};
use piping_backend::contexts::sync::application::inventory_transfer::{
    ApplyRequest, PreparationRequest, TransferApplication, TransferReport, load_offline,
    pin_descriptor, prepare_descriptor,
};
use piping_backend::contexts::sync::domain::inventory_export::CaptureOutcome;
use piping_backend::contexts::sync::domain::inventory_preflight::{
    PreflightReport, Severity, catalog_digest, tool_digest,
};
use piping_backend::contexts::sync::domain::{TransferApproval, TransferScope};
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::LocalInventoryCatalog;
use piping_backend::contexts::sync::infrastructure::providers::netbox_snapshot_capture::MAX_SNAPSHOT_TRANSPORT_BYTES;
use piping_backend::contexts::sync::infrastructure::providers::{
    CaptureOptions, ImmutableInventorySource, capture_inventory, capture_snapshot,
};
use piping_backend::integrations::netbox::NetboxClient;
use piping_backend::storage::cache::redis::client::build_connection_manager;
use serde::Serialize;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const INPUT_LIMIT: u64 = 16 * 1024 * 1024;
const HELP: &str = "inventory-transfer: assessment, pinned descriptor, apply/resume\n\
  export     --snapshot-transport FILE --provider-id UUID --operator-id UUID\n\
             --endorse-provenance-digest SHA256 --output NEW_DIRECTORY\n\
  capture-http --url URL --provider-id UUID --exporter-source-sha SHA --output NEW_DIRECTORY\n\
             [--collection-grants JSON_FILE --authority-evidence PRIVATE_FILE]\n\
  preflight  --bundle DIRECTORY --bundle-digest SHA256 --provider-id UUID\n\
             --mapping JSON_FILE --profile JSON_FILE --profile-digest SHA256\n\
  dry-run    (same flags as preflight)\n\
  descriptor --bundle DIRECTORY --bundle-digest SHA256 --provider-id UUID\n\
             --mapping JSON_FILE --profile JSON_FILE --profile-digest SHA256\n\
             --run-id UUID --operator-id UUID --approval JSON_FILE\n\
             --scope JSON_FILE --endorse-provenance-digest SHA256 --output NEW_DESCRIPTOR_FILE\n\
  apply      --descriptor FILE --descriptor-digest SHA256 --bundle DIRECTORY\n\
             --mapping JSON_FILE --profile JSON_FILE --endorse-provenance-digest SHA256\n\
  resume     (same flags as apply)\n\
Secrets and connections are read only from the environment:\n\
  INVENTORY_TRANSFER_NETBOX_TOKEN      capture-http\n\
  INVENTORY_TRANSFER_DATABASE_URL      dry-run, descriptor, apply, resume\n\
  INVENTORY_TRANSFER_REDIS_URL         apply, resume (provider lease only)\n\
  INVENTORY_TRANSFER_OPERATOR_TOKEN    descriptor, apply, resume (bearer token)\n\
Offline assessment and descriptor observation use no Redis or NetBox client.\n\
Descriptor token authentication refreshes usage metadata, not inventory.\n\
apply/resume never contact NetBox, bootstrap, migrate, seed or schedule.\n\
A descriptor pins the exact export artifact, its operator-endorsed snapshot,\n\
mapping, profile, catalog, tool, schema and executable, plus a separate run\n\
approval window; a changed pin is a different run, never a re-plan.\n\
JSON is written to stdout; retain it privately. Exit 0: completed without\n\
blockers; exit 2: completed refusal/blocked/stale/busy/uncertain; exit 1:\n\
operational or input failure. No exit 0 authorizes an unverified import.\n";

// These errors are static: never format an argument, path, URL, SQL/HTTP error
// or provider value into terminal output.
type CliResult<T> = Result<T, &'static str>;

struct Arguments {
    command: String,
    values: BTreeMap<String, String>,
}

impl Arguments {
    fn parse(args: impl IntoIterator<Item = String>) -> CliResult<Self> {
        let mut args = args.into_iter();
        let command = args.next().ok_or("inventory_transfer:command_required")?;
        let allowed: &[&str] = match command.as_str() {
            "export" => &[
                "--snapshot-transport",
                "--provider-id",
                "--operator-id",
                "--endorse-provenance-digest",
                "--output",
            ],
            "capture-http" => &[
                "--url",
                "--provider-id",
                "--exporter-source-sha",
                "--output",
                "--collection-grants",
                "--authority-evidence",
            ],
            "preflight" | "dry-run" => &[
                "--bundle",
                "--bundle-digest",
                "--provider-id",
                "--mapping",
                "--profile",
                "--profile-digest",
            ],
            "descriptor" => &[
                "--bundle",
                "--bundle-digest",
                "--provider-id",
                "--mapping",
                "--profile",
                "--profile-digest",
                "--run-id",
                "--operator-id",
                "--approval",
                "--scope",
                "--endorse-provenance-digest",
                "--output",
            ],
            "apply" | "resume" => &[
                "--descriptor",
                "--descriptor-digest",
                "--bundle",
                "--mapping",
                "--profile",
                "--endorse-provenance-digest",
            ],
            _ => return Err("inventory_transfer:unknown_command"),
        };
        let mut values = BTreeMap::new();
        while let Some(flag) = args.next() {
            if !allowed.contains(&flag.as_str()) {
                return Err("inventory_transfer:unknown_option");
            }
            let value = args
                .next()
                .ok_or("inventory_transfer:option_value_required")?;
            if value.is_empty() || value.starts_with("--") {
                return Err("inventory_transfer:option_value_required");
            }
            if values.insert(flag, value).is_some() {
                return Err("inventory_transfer:duplicate_option");
            }
        }
        let required: &[&str] = match command.as_str() {
            "export" => &[
                "--snapshot-transport",
                "--provider-id",
                "--operator-id",
                "--endorse-provenance-digest",
                "--output",
            ],
            "capture-http" => &[
                "--url",
                "--provider-id",
                "--exporter-source-sha",
                "--output",
            ],
            "descriptor" => &[
                "--bundle",
                "--bundle-digest",
                "--provider-id",
                "--mapping",
                "--profile",
                "--profile-digest",
                "--run-id",
                "--operator-id",
                "--approval",
                "--scope",
                "--endorse-provenance-digest",
                "--output",
            ],
            "apply" | "resume" => &[
                "--descriptor",
                "--descriptor-digest",
                "--bundle",
                "--mapping",
                "--profile",
                "--endorse-provenance-digest",
            ],
            _ => &[
                "--bundle",
                "--bundle-digest",
                "--provider-id",
                "--mapping",
                "--profile",
                "--profile-digest",
            ],
        };
        if required.iter().any(|key| !values.contains_key(*key)) {
            return Err("inventory_transfer:required_option_missing");
        }
        if values.contains_key("--collection-grants") != values.contains_key("--authority-evidence")
        {
            return Err("inventory_transfer:grant_evidence_pair_required");
        }
        Ok(Self { command, values })
    }

    fn required(&self, key: &str) -> CliResult<&str> {
        self.values
            .get(key)
            .map(String::as_str)
            .ok_or("inventory_transfer:required_option_missing")
    }

    fn provider_id(&self) -> CliResult<Uuid> {
        let id = Uuid::parse_str(self.required("--provider-id")?)
            .map_err(|_| "inventory_transfer:provider_identity")?;
        if id.is_nil() {
            return Err("inventory_transfer:provider_identity");
        }
        Ok(id)
    }

    fn uuid(&self, key: &str) -> CliResult<Uuid> {
        let id = Uuid::parse_str(self.required(key)?)
            .map_err(|_| "inventory_transfer:identity_invalid")?;
        if id.is_nil() {
            return Err("inventory_transfer:identity_invalid");
        }
        Ok(id)
    }
}

fn read_input(path: &str) -> CliResult<Vec<u8>> {
    read_bounded_input(path, INPUT_LIMIT)
}

fn read_bounded_input(path: &str, limit: u64) -> CliResult<Vec<u8>> {
    let file = File::open(path).map_err(|_| "inventory_transfer:input_read")?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "inventory_transfer:input_read")?;
    if bytes.len() as u64 > limit {
        return Err("inventory_transfer:input_limit");
    }
    Ok(bytes)
}

/// Create a new private file. Descriptors are never overwritten: an existing
/// path is an operator mistake, not a reason to replace a pinned artifact.
fn write_new_private_file(path: &str, bytes: &[u8]) -> CliResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "inventory_transfer:output_create")?;
    file.write_all(bytes)
        .map_err(|_| "inventory_transfer:output_write")?;
    file.sync_all()
        .map_err(|_| "inventory_transfer:output_write")
}

fn output(value: &impl Serialize) -> CliResult<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)
        .map_err(|_| "inventory_transfer:report_write")?;
    stdout
        .write_all(b"\n")
        .map_err(|_| "inventory_transfer:report_write")
}

fn report_exit(report: &PreflightReport) -> u8 {
    if report
        .findings
        .iter()
        .any(|f| f.severity == Severity::Blocker)
    {
        2
    } else {
        0
    }
}

async fn connect_pool(max_connections: u32) -> CliResult<PgPool> {
    let database_url = std::env::var("INVENTORY_TRANSFER_DATABASE_URL")
        .map_err(|_| "inventory_transfer:database_url_required")?;
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(15))
        .connect(&database_url)
        .await
        .map_err(|_| "inventory_transfer:target_connection_failed")
}

async fn run(args: Arguments) -> CliResult<u8> {
    match args.command.as_str() {
        "export" => run_snapshot_export(&args),
        "capture-http" => run_http_capture(&args).await,
        "preflight" | "dry-run" => run_assessment(&args).await,
        "descriptor" => run_descriptor(&args).await,
        _ => run_apply(&args).await,
    }
}

fn run_snapshot_export(args: &Arguments) -> CliResult<u8> {
    let provider_id = args.provider_id()?;
    let capture = capture_snapshot(
        &read_bounded_input(
            args.required("--snapshot-transport")?,
            MAX_SNAPSHOT_TRANSPORT_BYTES,
        )?,
        provider_id,
        args.uuid("--operator-id")?,
        args.required("--endorse-provenance-digest")?,
    )
    .map_err(|_| "inventory_transfer:snapshot_refused")?;
    let bundle_digest = capture
        .seal(Path::new(args.required("--output")?))
        .map_err(|_| "inventory_transfer:seal_failed")?;
    output(&serde_json::json!({
        "bundle_digest": bundle_digest,
        "source_provider_id": provider_id,
        "capture_outcome": capture.manifest.capture.outcome,
        "authority": capture.manifest.authority.attestation_identity()
            .map_err(|_| "inventory_transfer:authority_invalid")?,
        "import_capability": "requires_verified_operator_descriptor"
    }))?;
    Ok(0)
}

async fn run_http_capture(args: &Arguments) -> CliResult<u8> {
    let provider_id = args.provider_id()?;
    let mut options = CaptureOptions::default();
    if let Some(path) = args.values.get("--collection-grants") {
        let grants: Vec<String> = serde_json::from_slice(&read_input(path)?)
            .map_err(|_| "inventory_transfer:grant_encoding")?;
        options.collection_grants = grants.iter().cloned().collect::<BTreeSet<_>>();
        if options.collection_grants.len() != grants.len() {
            return Err("inventory_transfer:duplicate_grant");
        }
        options.authority_evidence = Some(read_input(args.required("--authority-evidence")?)?);
    }
    let netbox_token = std::env::var("INVENTORY_TRANSFER_NETBOX_TOKEN")
        .map_err(|_| "inventory_transfer:netbox_token_required")?;
    if netbox_token.trim().is_empty() {
        return Err("inventory_transfer:netbox_token_required");
    }
    let client = NetboxClient::new(args.required("--url")?.into(), netbox_token)
        .map_err(|_| "inventory_transfer:netbox_configuration")?;
    let capture = capture_inventory(
        &client,
        provider_id,
        args.required("--exporter-source-sha")?,
        &options,
    )
    .await
    .map_err(|_| "inventory_transfer:capture_failed")?;
    let bundle_digest = capture
        .seal(Path::new(args.required("--output")?))
        .map_err(|_| "inventory_transfer:seal_failed")?;
    output(&serde_json::json!({
        "bundle_digest": bundle_digest,
        "source_provider_id": provider_id,
        "capture_outcome": capture.manifest.capture.outcome,
        "import_capability": "unavailable"
    }))?;
    Ok(
        if capture.manifest.capture.outcome == CaptureOutcome::Complete {
            0
        } else {
            2
        },
    )
}

async fn run_assessment(args: &Arguments) -> CliResult<u8> {
    let provider_id = args.provider_id()?;
    // Read and verify once. All subsequent assessment consumes verified memory.
    let source = ImmutableInventorySource::open(
        Path::new(args.required("--bundle")?),
        args.required("--bundle-digest")?,
        provider_id,
        100,
    )
    .map_err(|_| "inventory_transfer:bundle_verification_failed")?;
    let mapping = read_input(args.required("--mapping")?)?;
    let profile = read_input(args.required("--profile")?)?;
    let assessment = preflight(
        &source,
        &mapping,
        &profile,
        args.required("--profile-digest")?,
    )
    .map_err(|_| "inventory_transfer:preflight_failed")?;
    if args.command == "preflight" || !assessment.can_plan_target() {
        let exit = report_exit(&assessment.report);
        output(&assessment.report)?;
        return Ok(exit);
    }

    // This is the only read-only connection path. No Settings/AppState,
    // migrations, seed, lease, job, outbox, Redis or application writer is
    // constructed.
    let pool = connect_pool(1).await?;
    let result = dry_run(&LocalInventoryCatalog::new(pool.clone()), assessment).await;
    pool.close().await;
    let report = result.map_err(|_| "inventory_transfer:target_assessment_failed")?;
    let exit = report_exit(&report);
    output(&report)?;
    Ok(exit)
}

/// Prepare an immutable descriptor from verified inputs and a read-only target
/// observation. Prints the exact pinned digest; the descriptor bytes are written
/// to `--output` and never re-encoded afterwards.
async fn run_descriptor(args: &Arguments) -> CliResult<u8> {
    let run_id = args.uuid("--run-id")?;
    let operator_id = args.uuid("--operator-id")?;
    let provider_id = args.provider_id()?;
    let approval: TransferApproval =
        serde_json::from_slice(&read_input(args.required("--approval")?)?)
            .map_err(|_| "inventory_transfer:approval_encoding")?;
    let scope: TransferScope = serde_json::from_slice(&read_input(args.required("--scope")?)?)
        .map_err(|_| "inventory_transfer:scope_encoding")?;
    let mapping = read_input(args.required("--mapping")?)?;
    let profile = read_input(args.required("--profile")?)?;

    // Verified offline inputs first: a bad or incomplete artifact must fail
    // before any target connection exists.
    let offline = match load_offline(
        Path::new(args.required("--bundle")?),
        args.required("--bundle-digest")?,
        provider_id,
        &mapping,
        &profile,
        args.required("--profile-digest")?,
    ) {
        Ok(offline) => offline,
        Err(error) => {
            output(&TransferReport::from_error(&error, Some(run_id), None))?;
            return Ok(error.exit_code());
        }
    };

    let pool = connect_pool(1).await?;
    let result = prepare_descriptor(
        &pool,
        PreparationRequest {
            run_id,
            operator_id,
            operator_token: std::env::var("INVENTORY_TRANSFER_OPERATOR_TOKEN")
                .map_err(|_| "inventory_transfer:operator_token_required")?,
            endorsed_provenance_sha256: args.required("--endorse-provenance-digest")?.into(),
            approval,
            scope,
        },
        offline,
    )
    .await;
    pool.close().await;
    match result {
        Ok(prepared) => {
            write_new_private_file(args.required("--output")?, prepared.pinned.bytes())?;
            output(&serde_json::json!({
                "state": "prepared",
                "run_id": prepared.pinned.descriptor().run_id,
                "provider_id": prepared.pinned.descriptor().provider_id,
                "descriptor_digest": prepared.pinned.descriptor_digest(),
                "report": prepared.report,
            }))?;
            Ok(0)
        }
        Err(error) => {
            output(&TransferReport::from_error(&error, Some(run_id), None))?;
            Ok(error.exit_code())
        }
    }
}

async fn run_apply(args: &Arguments) -> CliResult<u8> {
    let descriptor_bytes = read_input(args.required("--descriptor")?)?;
    let descriptor_digest = args.required("--descriptor-digest")?;
    // Reject an unpinned or altered descriptor before any environment or
    // connection is touched.
    let pinned = match pin_descriptor(&descriptor_bytes, descriptor_digest) {
        Ok(pinned) => pinned,
        Err(error) => {
            output(&TransferReport::from_error(
                &error,
                None,
                Some(descriptor_digest.to_owned()),
            ))?;
            return Ok(error.exit_code());
        }
    };
    let run_id = pinned.descriptor().run_id;
    let digest_text = pinned.descriptor_digest().to_owned();
    let mapping = read_input(args.required("--mapping")?)?;
    let profile = read_input(args.required("--profile")?)?;
    let operator_token = std::env::var("INVENTORY_TRANSFER_OPERATOR_TOKEN")
        .map_err(|_| "inventory_transfer:operator_token_required")?;
    let redis_url = std::env::var("INVENTORY_TRANSFER_REDIS_URL")
        .map_err(|_| "inventory_transfer:redis_url_required")?;
    let redis = build_connection_manager(&redis_url)
        .await
        .map_err(|_| "inventory_transfer:redis_connection_failed")?;
    let pool = connect_pool(4).await?;
    let application = TransferApplication::new(pool.clone(), redis);
    let result = application
        .apply(ApplyRequest {
            descriptor_bytes: &descriptor_bytes,
            descriptor_digest,
            bundle: Path::new(args.required("--bundle")?),
            mapping_bytes: &mapping,
            profile_bytes: &profile,
            endorsed_provenance_sha256: args.required("--endorse-provenance-digest")?,
            operator_token: &operator_token,
            resume: args.command == "resume",
        })
        .await;
    pool.close().await;
    match result {
        Ok(report) => {
            output(&report)?;
            Ok(0)
        }
        Err(error) => {
            output(&TransferReport::from_error(
                &error,
                Some(run_id),
                Some(digest_text),
            ))?;
            Ok(error.exit_code())
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Result<Vec<_>, _> = std::env::args_os()
        .skip(1)
        .map(|s| s.into_string())
        .collect();
    let result = match args {
        Ok(args) if args == ["--help"] => {
            print!("{HELP}");
            output(&serde_json::json!({"catalog_digest": catalog_digest(), "tool_digest": tool_digest()}))
                .map(|()| 0)
        }
        Ok(args) => match Arguments::parse(args) {
            Ok(args) => run(args).await,
            Err(error) => Err(error),
        },
        Err(_) => Err("inventory_transfer:argument_encoding"),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> CliResult<Arguments> {
        Arguments::parse(args.iter().map(|s| (*s).to_owned()))
    }

    fn apply_arguments() -> Vec<&'static str> {
        vec![
            "apply",
            "--descriptor",
            "descriptor.json",
            "--descriptor-digest",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--bundle",
            "private-bundle",
            "--mapping",
            "private-mapping",
            "--profile",
            "private-profile",
            "--endorse-provenance-digest",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ]
    }

    #[test]
    fn mutation_commands_require_pinned_inputs_and_unknown_secret_flags_are_rejected() {
        assert!(matches!(
            parse(&["apply"]),
            Err("inventory_transfer:required_option_missing")
        ));
        assert!(matches!(
            parse(&["resume"]),
            Err("inventory_transfer:required_option_missing")
        ));
        assert!(parse(&apply_arguments()).is_ok());
        assert!(matches!(
            parse(&["export", "--token", "private-canary"]),
            Err("inventory_transfer:unknown_option")
        ));
        assert!(matches!(
            parse(&["apply", "--token", "private-canary"]),
            Err("inventory_transfer:unknown_option")
        ));
    }

    #[test]
    fn ambiguous_options_and_unpaired_grants_fail_closed() {
        assert!(matches!(
            parse(&["capture-http", "--url", "one", "--url", "two"]),
            Err("inventory_transfer:duplicate_option")
        ));
        let common = [
            "capture-http",
            "--url",
            "https://example.test",
            "--provider-id",
            "d766fd88-e7f8-4a28-97cb-9336e9f18b68",
            "--exporter-source-sha",
            "0000000000000000000000000000000000000000",
            "--output",
            "private-bundle",
        ];
        assert!(parse(&common).is_ok());
        let mut unpaired = common.to_vec();
        unpaired.extend(["--collection-grants", "grants.json"]);
        assert!(matches!(
            parse(&unpaired),
            Err("inventory_transfer:grant_evidence_pair_required")
        ));
        assert!(matches!(
            parse(&["preflight", "--bundle", "--profile"]),
            Err("inventory_transfer:option_value_required")
        ));
    }
}
