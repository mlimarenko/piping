//! Exact RF decimals at the shared domain and flattened native DTO boundaries.

use piping_backend::api::rest::routes::inventory::native::ipam::{
    InterfaceCreatePayload, InterfaceUpdatePayload,
};
use piping_backend::contexts::inventory::domain::dcim::interface::{
    InterfaceNetworking, InterfaceNetworkingPatch, InterfaceRfDecimal, InterfaceRfMeasurement,
};
use serde_json::{Number, Value};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn create_body(field: &str, token: &str) -> String {
    format!(r#"{{"device_id":"device","name":"wlan0","{field}":{token}}}"#)
}

#[test]
fn rf_exact_numbers_and_source_strings_preserve_thousandths() -> TestResult {
    for (measurement, boundary) in [
        (InterfaceRfMeasurement::Frequency, "99999.999"),
        (InterfaceRfMeasurement::Width, "9999.999"),
    ] {
        for raw in [
            "0",
            "-0.0000",
            "0.001",
            "-0.001",
            "0.029",
            "1.114",
            "5180.125",
            "1.1140000",
            "1114e-3",
            "111400e-5",
            "0.0111400e2",
            "-1114E-3",
            boundary,
            &format!("-{boundary}"),
        ] {
            let number: Number = serde_json::from_str(raw)?;
            let expected = raw.parse::<f64>()?;
            assert_eq!(measurement.validate(&number)?, expected, "{raw}");
            assert_eq!(
                measurement.validate_source(&serde_json::from_str::<Value>(raw)?)?,
                expected,
                "{raw}"
            );
            assert_eq!(
                measurement.validate_source(&Value::String(raw.into()))?,
                expected,
                "{raw}"
            );
        }
        for raw in ["+000.02900", "000.029", ".029", "29.e-3", "+0029e-0003"] {
            assert_eq!(measurement.validate_decimal(raw)?, "0.029".parse::<f64>()?);
        }
        for padding in [1, 30, 1000] {
            let raw = format!("29{}e-{}", "0".repeat(padding), padding + 3);
            assert_eq!(measurement.validate_decimal(&raw)?, "0.029".parse::<f64>()?);
        }
        for raw in [
            "0e999999999999999999999999999999",
            "-0e-999999999999999999999999999999",
        ] {
            assert_eq!(measurement.validate_decimal(raw)?, 0.0);
        }
    }
    Ok(())
}

#[test]
fn rf_exact_scale_range_and_invalid_source_shapes_are_rejected() -> TestResult {
    for (measurement, bound) in [
        (InterfaceRfMeasurement::Frequency, "100000"),
        (InterfaceRfMeasurement::Width, "10000"),
    ] {
        for raw in [
            "0.000000005",
            "-0.000000005",
            "0.0001",
            "-0.0001",
            "0.0290000000000000000000000000000000000001",
            "-1.114000000000000000000000000000000000001",
            "9999.999000000000000000000000000000000001",
            "1e-1000",
            "1e1000",
            bound,
            &format!("-{bound}"),
        ] {
            let number: Number = serde_json::from_str(raw)?;
            assert!(
                measurement.validate(&number).is_err(),
                "{measurement:?} {raw}"
            );
            assert!(measurement.validate_source(&Value::Number(number)).is_err());
            assert!(
                measurement
                    .validate_source(&Value::String(raw.into()))
                    .is_err()
            );
        }
        for raw in [
            "1e999999999999999999999999999999",
            "1e-999999999999999999999999999999",
            "NaN",
            "Infinity",
            " 0.029",
            "0.029 ",
            "0.029suffix",
            "0.029e",
            "0.029e+",
            "0.0.29",
            ".",
            "--0.029",
        ] {
            assert!(measurement.validate_decimal(raw).is_err(), "{raw}");
        }
        for raw in ["null", "true", "[]", "{}", r#"{"value":0.029}"#] {
            assert!(
                measurement
                    .validate_source(&serde_json::from_str(raw)?)
                    .is_err()
            );
        }
    }
    Ok(())
}

#[test]
fn flattened_native_rf_fields_validate_original_tokens_before_float_conversion() -> TestResult {
    for (field, bound, boundary) in [
        ("rf_channel_frequency", "100000", "99999.999"),
        ("rf_channel_width", "10000", "9999.999"),
    ] {
        for raw in [
            "0",
            "-0.000",
            "0.001",
            "-0.001",
            "0.029",
            "1.114",
            "5180.125",
            "1.114000",
            "1114e-3",
            "-1114e-3",
            " 0.029 ",
            boundary,
            &format!("-{boundary}"),
        ] {
            let create: InterfaceCreatePayload = serde_json::from_str(&create_body(field, raw))?;
            create.networking.validate()?;
            let expected = raw.trim().parse::<f64>()?;
            let serialized = serde_json::to_value(&create.networking)?;
            assert_eq!(serialized[field].as_f64(), Some(expected));
            // Value-based response/model decoding remains supported as well.
            let decoded: InterfaceNetworking = serde_json::from_value(serialized)?;
            decoded.validate()?;
            let update: InterfaceUpdatePayload =
                serde_json::from_str(&format!(r#"{{"{field}":{raw}}}"#))?;
            let updated = update.networking.apply(&InterfaceNetworking::default());
            updated.validate()?;
            assert_eq!(
                serde_json::to_value(updated)?[field].as_f64(),
                Some(expected)
            );
        }
        for raw in [
            "0.000000005",
            "-0.000000005",
            "0.0001",
            "0.0290000000000000000000000000000000000001",
            "9999.999000000000000000000000000000000001",
            "1e-1000",
            "1e1000",
            bound,
            &format!("-{bound}"),
        ] {
            assert!(
                serde_json::from_str::<InterfaceCreatePayload>(&create_body(field, raw)).is_err(),
                "create {field} {raw}"
            );
            assert!(
                serde_json::from_str::<InterfaceUpdatePayload>(&format!(r#"{{"{field}":{raw}}}"#))
                    .is_err(),
                "patch {field} {raw}"
            );
        }
        // The serde_json private numeric token is an existing Content-buffering
        // limitation, not an ordinary native JSON object contract tested here.
        for raw in [r#""0.029""#, "true", "[]", "{}", r#"{"value":0.029}"#] {
            assert!(
                serde_json::from_str::<InterfaceCreatePayload>(&create_body(field, raw)).is_err()
            );
            assert!(
                serde_json::from_str::<InterfaceUpdatePayload>(&format!(r#"{{"{field}":{raw}}}"#))
                    .is_err()
            );
        }
    }
    Ok(())
}

#[test]
fn native_rf_patch_preserves_omission_and_clears_null() -> TestResult {
    let current: InterfaceNetworking =
        serde_json::from_str(r#"{"rf_channel_frequency":5180.125,"rf_channel_width":20.125}"#)?;
    let omitted: InterfaceUpdatePayload = serde_json::from_str("{}")?;
    assert!(omitted.networking.rf_channel_frequency.is_none());
    assert!(omitted.networking.rf_channel_width.is_none());
    assert_eq!(omitted.networking.apply(&current), current);
    let clear: InterfaceUpdatePayload =
        serde_json::from_str(r#"{"rf_channel_frequency":null,"rf_channel_width":null}"#)?;
    assert_eq!(clear.networking.rf_channel_frequency, Some(None));
    assert_eq!(clear.networking.rf_channel_width, Some(None));
    let cleared = clear.networking.apply(&current);
    assert!(cleared.rf_channel_frequency.is_none());
    assert!(cleared.rf_channel_width.is_none());
    cleared.validate()?;
    let empty: InterfaceNetworking = serde_json::from_str("{}")?;
    assert!(empty.rf_channel_frequency.is_none());
    assert!(empty.rf_channel_width.is_none());
    let direct_patch: InterfaceNetworkingPatch = serde_json::from_str("{}")?;
    assert_eq!(direct_patch.apply(&current), current);
    Ok(())
}

#[test]
fn internally_constructed_rf_scalars_use_the_same_exact_policy() -> TestResult {
    for raw in ["0.000000005", "-0.000000005", "0.0001", "100000", "-100000"] {
        let networking = InterfaceNetworking {
            rf_channel_frequency: Some(InterfaceRfDecimal(raw.parse()?)),
            ..Default::default()
        };
        assert!(networking.validate().is_err(), "{raw}");
    }
    for raw in ["0.029", "-0.029", "1.114", "99999.999", "-99999.999"] {
        let networking = InterfaceNetworking {
            rf_channel_frequency: Some(InterfaceRfDecimal(raw.parse()?)),
            ..Default::default()
        };
        networking.validate()?;
    }
    Ok(())
}
