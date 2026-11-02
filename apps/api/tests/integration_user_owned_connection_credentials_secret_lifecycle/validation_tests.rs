use super::*;

#[tokio::test]
async fn create_session_preserves_rdp_options_width_height()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let proxy_client = Arc::new(CapturingProxyClient::default());
    let service = build_connection_service_with_defaults(
        proxy_client.clone(),
        json!({
            "piping": {
                "connections": {
                    "rdp": [{
                        "host": "10.10.1.230",
                        "port": RDP_PORT_DEFAULT,
                        "credentials": {
                            "username": "operator",
                            "password": "secret"
                        },
                        "options": {
                            "width": SCREEN_WIDTH_1366,
                            "height": SCREEN_HEIGHT_768
                        }
                    }]
                }
            }
        }),
    );

    let _session = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Rdp,
        None,
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    assert_eq!(
        payload
            .get("options")
            .and_then(Value::as_object)
            .and_then(|options| options.get("width"))
            .and_then(Value::as_i64),
        Some(SCREEN_WIDTH_1366)
    );
    assert_eq!(
        payload
            .get("options")
            .and_then(Value::as_object)
            .and_then(|options| options.get("height"))
            .and_then(Value::as_i64),
        Some(SCREEN_HEIGHT_768)
    );
    Ok(())
}

#[tokio::test]
async fn create_session_rejects_unknown_launch_param_keys() {
    let service = build_connection_service_with_defaults(
        Arc::new(CapturingProxyClient::default()),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "label": "Operator SSH",
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "legacy_mode": true,
                        "credentials": { "username": "root" }
                    }]
                }
            }
        }),
    );

    let error = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Ssh,
        None,
    )
    .await
    .expect_err("unknown launch keys must be rejected");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("Unsupported launch params"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn create_session_rejects_string_timeout_value() {
    let service = build_connection_service_with_defaults(
        Arc::new(CapturingProxyClient::default()),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "timeout": "5",
                        "credentials": { "username": "root" }
                    }]
                }
            }
        }),
    );

    let error = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Ssh,
        None,
    )
    .await
    .expect_err("string timeout must be rejected");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("Invalid timeout value"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn create_session_rejects_string_probe_only_value() {
    let service = build_connection_service_with_defaults(
        Arc::new(CapturingProxyClient::default()),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "probe_only": "true",
                        "credentials": { "username": "root" }
                    }]
                }
            }
        }),
    );

    let error = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Ssh,
        None,
    )
    .await
    .expect_err("string probe_only must be rejected");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("Invalid probe_only value"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn create_session_rejects_ssh_entry_without_username() {
    let service = build_connection_service_with_defaults(
        Arc::new(CapturingProxyClient::default()),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "label": "Operator SSH",
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "credentials": { "password": "secret" }
                    }]
                }
            }
        }),
    );

    let error = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Ssh,
        None,
    )
    .await
    .expect_err("ssh username must be required");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("credentials.username"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn create_session_rejects_legacy_launch_params_metadata() {
    let service = build_connection_service_with_defaults(
        Arc::new(CapturingProxyClient::default()),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "credentials": { "username": "root" }
                    }]
                }
            }
        }),
    );

    let error = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Ssh,
        Some(json!({ "launch_params": {} })),
    )
    .await
    .expect_err("legacy launch_params metadata must be rejected");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("launch_params"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn create_session_rejects_unknown_metadata_key() {
    let service = build_connection_service_with_defaults(
        Arc::new(CapturingProxyClient::default()),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "credentials": { "username": "root" }
                    }]
                }
            }
        }),
    );

    let error = create_session_for_test(
        &service,
        Uuid::now_v7(),
        Uuid::now_v7(),
        ConnectionProtocol::Ssh,
        Some(json!({ "foo": "bar" })),
    )
    .await
    .expect_err("unknown metadata key must be rejected");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("Unsupported session_metadata key"),
                "unexpected validation message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn create_session_uses_context_first_merge_policy_by_default()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_id = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "credentials": { "username": "context-user" }
                    }]
                }
            }
        }))
        .with_user_set(test_user_set(
            credential_set_id,
            user_id,
            ConnectionProtocol::Ssh,
            "Default SSH",
            json!({
                "port": SSH_PORT_PERSONAL_FIRST,
                "credentials": {
                    "username": "personal-user"
                }
            }),
            true,
        )),
    );

    let session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Ssh,
        Some(session_metadata_with_entry_id(
            ENTRY_ID_CONFIG_CONTEXT_SSH_PRIMARY,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);
    let payload = proxy_client.take_payload().await?;

    assert_eq!(session.target_id, target_id);
    assert_eq!(
        payload.get("port").and_then(Value::as_i64),
        Some(SSH_PORT_DEFAULT)
    );
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("context-user")
    );
    Ok(())
}

#[tokio::test]
async fn create_session_uses_personal_first_merge_policy_when_requested()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_id = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "credentials": { "username": "context-user" }
                    }]
                }
            }
        }))
        .with_user_set(test_user_set(
            credential_set_id,
            user_id,
            ConnectionProtocol::Ssh,
            "Default SSH",
            json!({
                "port": SSH_PORT_PERSONAL_FIRST,
                "credentials": {
                    "username": "personal-user"
                }
            }),
            true,
        )),
    );

    let session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Ssh,
        Some(session_metadata_with_policy(
            ENTRY_ID_CONFIG_CONTEXT_SSH_PRIMARY,
            SESSION_MERGE_POLICY_PERSONAL_FIRST,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);
    let payload = proxy_client.take_payload().await?;

    assert_eq!(session.target_id, target_id);
    assert_eq!(
        payload.get("port").and_then(Value::as_i64),
        Some(SSH_PORT_PERSONAL_FIRST)
    );
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("personal-user")
    );
    Ok(())
}
