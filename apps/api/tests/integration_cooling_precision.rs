//! Exact cooling precision at native DTO, shared validation and REST boundaries.
mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie,
};
use piping_backend::api::rest::routes::inventory::dcim::{rack_types::*, racks::*};
use piping_backend::contexts::inventory::domain::dcim::cooling::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn cooling_exact_decimal_ranges_scale_and_float_representability() -> TestResult {
    for measurement in [
        CoolingMeasurement::Capacity,
        CoolingMeasurement::Diameter,
        CoolingMeasurement::MaxFlow,
    ] {
        for raw in [
            "0.01",
            "0.29",
            "1.14",
            "0.290000",
            "29e-2",
            "29e-0002",
            "11400e-4",
            "0.011400e+2",
            "1.1400E+0",
            "999999.99",
        ] {
            let number = serde_json::from_str::<serde_json::Number>(raw)?;
            assert_eq!(measurement.validate(&number)?, raw.parse::<f64>()?);
            assert_eq!(
                measurement.validate_source(&json!(raw))?,
                raw.parse::<f64>()?
            );
        }
        for raw in [
            "0.00000005",
            "0.001",
            "-0.01",
            "0.2900000000000000000000000000000000000001",
            "1.1400000000000000000000000000000000000001",
            "1e-1000",
            "1e1000",
        ] {
            let number = serde_json::from_str::<serde_json::Number>(raw)?;
            assert!(
                measurement.validate(&number).is_err(),
                "{measurement:?} {raw}"
            );
            assert!(measurement.validate_source(&json!(raw)).is_err());
        }
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.01,
            0.00000005,
            0.1 + 0.2,
        ] {
            assert!(
                measurement.validate_float(value).is_err(),
                "{measurement:?} {value}"
            );
        }
        assert!(measurement.validate_float(0.29).is_ok());
        assert!(measurement.validate_float(1.14).is_ok());
    }
    for raw in ["0", "-0.000", "0e1000", "0e-1000", "99999999.99"] {
        let number = serde_json::from_str::<serde_json::Number>(raw)?;
        assert!(CoolingMeasurement::Capacity.validate(&number).is_ok());
        assert!(CoolingMeasurement::Diameter.validate(&number).is_err());
        assert!(CoolingMeasurement::MaxFlow.validate(&number).is_err());
    }
    for raw in ["100000000", "99999999.990000000000000000000000000001"] {
        assert!(
            CoolingMeasurement::Capacity
                .validate_source(&json!(raw))
                .is_err()
        );
    }
    for raw in ["1000000", "999999.990000000000000000000000000001"] {
        assert!(
            CoolingMeasurement::Diameter
                .validate_source(&json!(raw))
                .is_err()
        );
        assert!(
            CoolingMeasurement::MaxFlow
                .validate_source(&json!(raw))
                .is_err()
        );
    }
    assert!(validate_rack_cooling(None, Some(0.29)).is_ok());
    assert!(validate_rack_cooling(None, Some(1.14)).is_ok());
    assert!(validate_rack_cooling(None, Some(0.00000005)).is_err());
    for raw in ["+000.29000", "000.29", ".29", "29.e-2", "+0029e-0002"] {
        assert_eq!(
            CoolingMeasurement::Capacity.validate_source(&json!(raw))?,
            0.29
        );
    }
    for padding in [1, 30, 1000] {
        let raw = format!("29{}e-{}", "0".repeat(padding), padding + 2);
        assert_eq!(
            CoolingMeasurement::Capacity.validate_source(&json!(raw))?,
            0.29
        );
    }
    for raw in [
        "0e999999999999999999999999999999",
        "0e-999999999999999999999999999999",
    ] {
        assert_eq!(
            CoolingMeasurement::Capacity.validate_source(&json!(raw))?,
            0.0
        );
    }
    for raw in [
        "1e999999999999999999999999999999",
        "1e-999999999999999999999999999999",
        "0.29suffix",
        "0.29 ",
        " 0.29",
        "0.29e",
        "NaN",
        "Infinity",
        "0.29e+",
        "0.2.9",
        ".",
        "--0.29",
    ] {
        assert!(
            CoolingMeasurement::Capacity
                .validate_source(&json!(raw))
                .is_err()
        );
    }
    Ok(())
}

fn retained_measurement<T: DeserializeOwned + Serialize>(
    prefix: &str,
    field: &str,
    raw: &str,
) -> TestResult<Value> {
    let body = format!("{prefix},\"{field}\":{raw}}}");
    let payload = serde_json::from_str::<T>(&body)?;
    let value = serde_json::to_value(payload)?;
    // Response/model decoding through Value must still accept ordinary numbers.
    let decoded = serde_json::from_value::<T>(value)?;
    Ok(serde_json::to_value(decoded)?[field].clone())
}

#[test]
fn all_cooling_create_and_patch_dtos_preserve_raw_measurements() -> TestResult {
    for raw in [
        "0.29",
        "1.14",
        "29e-2",
        "1.140000",
        " 0.29 ",
        "0.00000005",
        "0.2900000000000000000000000000000000000001",
    ] {
        let numeric = serde_json::from_str::<Value>(raw)?;
        macro_rules! check {
            ($create:ty, $patch:ty, $prefix:expr, $field:literal, $measurement:expr) => {{
                let expected = $measurement
                    .validate(numeric.as_number().ok_or("number")?)
                    .is_ok();
                for value in [
                    retained_measurement::<$create>($prefix, $field, raw)?,
                    retained_measurement::<$patch>(r#"{"description":"precision""#, $field, raw)?,
                ] {
                    assert_eq!(
                        $measurement
                            .validate(value.as_number().ok_or("retained number")?)
                            .is_ok(),
                        expected,
                        "{} {} {raw}",
                        stringify!($create),
                        $field
                    );
                }
                for nonnumeric in [
                    format!("\"{raw}\""),
                    r#"{"$serde_json::private::Number":"0.29"}"#.to_owned(),
                    r#"{"value":0.29}"#.to_owned(),
                    r#"{"value":}"#.to_owned(),
                    "{}".to_owned(),
                    "[0.29]".to_owned(),
                    "true".to_owned(),
                ] {
                    // Pass original JSON text directly to the typed decoder;
                    // Value decoding could erase the forged object's shape.
                    assert!(
                        retained_measurement::<$create>($prefix, $field, &nonnumeric).is_err(),
                        "{} {} must reject {nonnumeric}",
                        stringify!($create),
                        $field
                    );
                    assert!(
                        retained_measurement::<$patch>(
                            r#"{"description":"precision""#,
                            $field,
                            &nonnumeric
                        )
                        .is_err(),
                        "{} {} must reject {nonnumeric}",
                        stringify!($patch),
                        $field
                    );
                }
            }};
        }
        check!(
            CoolingSourceCreate,
            CoolingSourcePatch,
            r#"{"name":"precision","type":"chiller","site_id":"00000000-0000-0000-0000-000000000001""#,
            "cooling_capacity",
            CoolingMeasurement::Capacity
        );
        check!(
            CoolingFeedCreate,
            CoolingFeedPatch,
            r#"{"name":"precision","cooling_source_id":"00000000-0000-0000-0000-000000000001""#,
            "cooling_capacity",
            CoolingMeasurement::Capacity
        );
        check!(
            CoolingFeedCreate,
            CoolingFeedPatch,
            r#"{"name":"precision","cooling_source_id":"00000000-0000-0000-0000-000000000001""#,
            "max_flow",
            CoolingMeasurement::MaxFlow
        );
        check!(
            CoolingIntakeCreate,
            CoolingIntakePatch,
            r#"{"name":"precision","device_id":"00000000-0000-0000-0000-000000000001""#,
            "diameter",
            CoolingMeasurement::Diameter
        );
        check!(
            CoolingIntakeCreate,
            CoolingIntakePatch,
            r#"{"name":"precision","device_id":"00000000-0000-0000-0000-000000000001""#,
            "max_flow",
            CoolingMeasurement::MaxFlow
        );
        check!(
            CoolingOutflowCreate,
            CoolingOutflowPatch,
            r#"{"name":"precision","device_id":"00000000-0000-0000-0000-000000000001""#,
            "diameter",
            CoolingMeasurement::Diameter
        );
        check!(
            CoolingIntakeTemplateCreate,
            CoolingIntakeTemplatePatch,
            r#"{"name":"precision""#,
            "diameter",
            CoolingMeasurement::Diameter
        );
        check!(
            CoolingIntakeTemplateCreate,
            CoolingIntakeTemplatePatch,
            r#"{"name":"precision""#,
            "max_flow",
            CoolingMeasurement::MaxFlow
        );
        check!(
            CoolingOutflowTemplateCreate,
            CoolingOutflowTemplatePatch,
            r#"{"name":"precision""#,
            "diameter",
            CoolingMeasurement::Diameter
        );
    }
    let absent: CoolingSourcePatch = serde_json::from_str("{}")?;
    let cleared: CoolingSourcePatch = serde_json::from_str(r#"{"cooling_capacity":null}"#)?;
    assert!(absent.cooling_capacity.is_none());
    assert!(matches!(cleared.cooling_capacity, Some(None)));
    Ok(())
}

#[test]
fn cooling_read_models_decode_numeric_json_and_value_fields() -> TestResult {
    for raw in ["0.29", "1.14", "29e-2", "1.140000"] {
        let values = [
            retained_measurement::<CoolingSource>(
                r#"{"id":"00000000-0000-0000-0000-000000000002","coolingfeed_count":0,"name":"precision","type":"chiller","site_id":"00000000-0000-0000-0000-000000000001""#,
                "cooling_capacity",
                raw,
            )?,
            retained_measurement::<CoolingFeed>(
                r#"{"id":"00000000-0000-0000-0000-000000000002","name":"precision","cooling_source_id":"00000000-0000-0000-0000-000000000001""#,
                "max_flow",
                raw,
            )?,
            retained_measurement::<CoolingIntake>(
                r#"{"id":"00000000-0000-0000-0000-000000000002","name":"precision","device_id":"00000000-0000-0000-0000-000000000001""#,
                "diameter",
                raw,
            )?,
            retained_measurement::<CoolingOutflow>(
                r#"{"id":"00000000-0000-0000-0000-000000000002","name":"precision","device_id":"00000000-0000-0000-0000-000000000001""#,
                "diameter",
                raw,
            )?,
            retained_measurement::<CoolingIntakeTemplate>(
                r#"{"id":"00000000-0000-0000-0000-000000000002","name":"precision""#,
                "max_flow",
                raw,
            )?,
            retained_measurement::<CoolingOutflowTemplate>(
                r#"{"id":"00000000-0000-0000-0000-000000000002","name":"precision""#,
                "diameter",
                raw,
            )?,
        ];
        for value in values {
            assert_eq!(value.as_f64(), Some(raw.parse::<f64>()?));
        }
    }
    Ok(())
}

#[test]
fn rack_and_rack_type_create_patch_and_bulk_validate_before_float_conversion() -> TestResult {
    for raw in [
        "0",
        "0.29",
        "1.14",
        "29e-2",
        "1.140000",
        "99999999.99",
        "0.00000005",
        "0.2900000000000000000000000000000000000001",
        "-0.01",
        "100000000",
        "\"0.29\"",
        r#"{"$serde_json::private::Number":"0.29"}"#,
        r#"{"value":0.29}"#,
        r#"{"value":}"#,
        "{}",
        "[0.29]",
        "true",
    ] {
        let valid = matches!(
            raw,
            "0" | "0.29" | "1.14" | "29e-2" | "1.140000" | "99999999.99"
        );
        let rack = format!(
            r#"{{"name":"precision","site_id":"00000000-0000-0000-0000-000000000001","cooling_capacity":{raw}}}"#
        );
        let rack_type = format!(
            r#"{{"model":"precision","manufacturer_id":"00000000-0000-0000-0000-000000000001","cooling_capacity":{raw}}}"#
        );
        let patch = format!(r#"{{"cooling_capacity":{raw}}}"#);
        assert_eq!(
            serde_json::from_str::<RackCreatePayload>(&rack).is_ok(),
            valid,
            "rack create {raw}"
        );
        assert_eq!(
            serde_json::from_str::<RackTypeCreatePayload>(&rack_type).is_ok(),
            valid,
            "rack type create {raw}"
        );
        assert_eq!(
            serde_json::from_str::<RackUpdatePayload>(&patch).is_ok(),
            valid,
            "rack patch {raw}"
        );
        assert_eq!(
            serde_json::from_str::<RackTypeUpdatePayload>(&patch).is_ok(),
            valid,
            "rack type patch {raw}"
        );
        assert_eq!(
            serde_json::from_str::<RackBulkCreatePayload>(&format!(r#"{{"items":[{rack}]}}"#))
                .is_ok(),
            valid,
            "rack bulk {raw}"
        );
        assert_eq!(
            serde_json::from_str::<RackTypeBulkCreatePayload>(&format!(
                r#"{{"items":[{rack_type}]}}"#
            ))
            .is_ok(),
            valid,
            "rack type bulk {raw}"
        );
        if valid {
            // Only numeric cases enter Value here. Forged-object regressions
            // above always reach the typed boundary as untouched JSON text.
            assert!(
                serde_json::from_value::<RackCreatePayload>(serde_json::from_str(&rack)?).is_ok()
            );
            assert!(
                serde_json::from_value::<RackTypeCreatePayload>(serde_json::from_str(&rack_type)?)
                    .is_ok()
            );
            assert!(
                serde_json::from_value::<RackUpdatePayload>(serde_json::from_str(&patch)?).is_ok()
            );
            assert!(
                serde_json::from_value::<RackTypeUpdatePayload>(serde_json::from_str(&patch)?)
                    .is_ok()
            );
            assert!(
                serde_json::from_value::<RackBulkCreatePayload>(serde_json::from_str(&format!(
                    r#"{{"items":[{rack}]}}"#
                ))?)
                .is_ok()
            );
            assert!(
                serde_json::from_value::<RackTypeBulkCreatePayload>(serde_json::from_str(
                    &format!(r#"{{"items":[{rack_type}]}}"#)
                )?)
                .is_ok()
            );
        }
    }
    let rack: RackUpdatePayload = serde_json::from_str("{}")?;
    let rack_type: RackTypeUpdatePayload = serde_json::from_str("{}")?;
    assert!(rack.cooling_capacity.is_none());
    assert!(rack_type.cooling_capacity.is_none());
    let rack: RackUpdatePayload = serde_json::from_str(r#"{"cooling_capacity":null}"#)?;
    let rack_type: RackTypeUpdatePayload = serde_json::from_str(r#"{"cooling_capacity":null}"#)?;
    assert_eq!(rack.cooling_capacity, Some(None));
    assert_eq!(rack_type.cooling_capacity, Some(None));
    let rack: RackCreatePayload = serde_json::from_str(r#"{"name":"precision","site_id":"site"}"#)?;
    let rack_type: RackTypeCreatePayload = serde_json::from_str(
        r#"{"model":"precision","manufacturer_id":"manufacturer","cooling_capacity":null}"#,
    )?;
    assert_eq!(rack.cooling_capacity, None);
    assert_eq!(rack_type.cooling_capacity, None);
    let rack: RackCreatePayload = serde_json::from_str(
        r#"{"name":"precision","site_id":"site","cooling_capacity":0.29000}"#,
    )?;
    let rack_type: RackTypeUpdatePayload = serde_json::from_str(r#"{"cooling_capacity":114e-2}"#)?;
    assert_eq!(rack.cooling_capacity, Some(0.29));
    assert_eq!(rack_type.cooling_capacity, Some(Some(1.14)));
    Ok(())
}

async fn call_raw(
    app: &Router,
    cookie: &str,
    method: Method,
    path: &str,
    body: String,
    expected: StatusCode,
) -> TestResult<Value> {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body(Body::from(body))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    assert_eq!(
        status,
        expected,
        "{path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; exact native handler persistence"]
async fn cooling_handlers_reject_raw_tails_without_changing_persisted_capacity() -> TestResult {
    let app = piping_backend::app::build_router(build_test_app_state().await?);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let db = build_test_db_pool().await?;
    let site = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')").bind(site).bind(format!("precision-{site}")).execute(&db).await?;
    let collection = "/api/v1/inventory/cooling-sources";
    for raw in [
        "0.00000005",
        "0.2900000000000000000000000000000000000001",
        "-0.01",
        "100000000",
        "\"0.29\"",
        r#"{"$serde_json::private::Number":"0.29"}"#,
        r#"{"value":0.29}"#,
    ] {
        let body = format!(
            r#"{{"name":"precision-invalid","site_id":"{site}","type":"chiller","cooling_capacity":{raw}}}"#
        );
        call_raw(
            &app,
            &cookie,
            Method::POST,
            collection,
            body,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_coolingsource WHERE site_id=$1")
        .bind(site)
        .fetch_one(&db)
        .await?;
    assert_eq!(count, 0, "invalid create must not persist a rounded row");
    let source = call_raw(&app, &cookie, Method::POST, collection, format!(r#"{{"name":"precision-valid","site_id":"{site}","type":"chiller","cooling_capacity":0.29}}"#), StatusCode::CREATED).await?;
    let id: Uuid = source["id"].as_str().ok_or("source id")?.parse()?;
    let path = format!("{collection}/{id}");
    for raw in [
        "0.29",
        "1.14",
        "29e-2",
        "1.140000",
        "0",
        "0e999999999999999999999999999999",
        "0e-999999999999999999999999999999",
        "99999999.99",
    ] {
        call_raw(
            &app,
            &cookie,
            Method::PUT,
            &path,
            format!(r#"{{"cooling_capacity":{raw}}}"#),
            StatusCode::OK,
        )
        .await?;
        let stored: String =
            sqlx::query_scalar("SELECT cooling_capacity::text FROM dcim_coolingsource WHERE id=$1")
                .bind(id)
                .fetch_one(&db)
                .await?;
        let expected = if raw.starts_with("0e") {
            0.0
        } else {
            raw.parse::<f64>()?
        };
        assert_eq!(stored, format!("{expected:.2}"));
    }
    for raw in [
        "0.00000005",
        "1.1400000000000000000000000000000000000001",
        "99999999.99000000000000000000000000000001",
        "-0.01",
        "100000000",
        "\"1.14\"",
        r#"{"$serde_json::private::Number":"0.29"}"#,
        r#"{"value":0.29}"#,
    ] {
        call_raw(
            &app,
            &cookie,
            Method::PUT,
            &path,
            format!(r#"{{"cooling_capacity":{raw}}}"#),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let stored: String =
            sqlx::query_scalar("SELECT cooling_capacity::text FROM dcim_coolingsource WHERE id=$1")
                .bind(id)
                .fetch_one(&db)
                .await?;
        assert_eq!(
            stored, "99999999.99",
            "invalid patch must not change storage"
        );
    }
    sqlx::query("DELETE FROM dcim_coolingsource WHERE id=$1")
        .bind(id)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site)
        .execute(&db)
        .await?;
    Ok(())
}
