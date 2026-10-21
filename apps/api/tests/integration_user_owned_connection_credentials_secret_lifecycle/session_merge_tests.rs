use super::*;

#[tokio::test]
async fn user_owned_credentials_override_selected_entry_and_are_resolved_server_side()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host_secret_password_1 = Uuid::now_v7();
    let host_secret_key_1 = Uuid::now_v7();
    let user_secret_password = Uuid::now_v7();
    let user_secret_key = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "ssh": [
                    {
                        "host": "10.0.0.1",
                        "port": SSH_PORT_DEFAULT,
                        "credentials": {
                            "username": "root",
                            "password": Uuid::now_v7().to_string(),
                            "private_key": Uuid::now_v7().to_string()
                        }
                    },
                    {
                        "host": "10.0.0.2",
                        "port": SSH_PORT_ALT,
                        "credentials": {
                            "username": "admin",
                            "password": host_secret_password_1.to_string(),
                            "private_key": host_secret_key_1.to_string()
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(host_secret_password_1, "host-pass-1")
            .with_secret_value(host_secret_key_1, "host-key-1")
            .with_secret_value(user_secret_password, "user-pass")
            .with_secret_value(user_secret_key, "user-key")
            .with_bindings(
                credential_set_id,
                HashMap::from([
                    (FIELD_KEY_SSH_PASSWORD, user_secret_password),
                    (FIELD_KEY_SSH_PRIVATE_KEY, user_secret_key),
                ]),
            )
            .with_host_secret_ids([host_secret_password_1, host_secret_key_1])
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Ssh,
                "Primary",
                json!({}),
                false,
            )),
    );

    let target_id = Uuid::now_v7();
    // Launch accounts are presented highest-priority first (descending
    // specificity/weight — see resolve_launch_accounts step 6), so the
    // weight-1 entry (host 10.0.0.2, the one carrying host secrets) is launch
    // index 0, not its array position in the seeded envelope.
    let entry_id = format!("user:config-context:ssh:0:{credential_set_id}");

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Ssh,
        Some(session_metadata_with_entry_id(entry_id)),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    assert_eq!(
        payload.get("host").and_then(Value::as_str),
        Some("10.0.0.2")
    );
    assert_eq!(
        payload.get("port").and_then(Value::as_i64),
        Some(SSH_PORT_ALT)
    );

    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("admin")
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("user-pass"),
        "user secret must override host secret and be resolved before proxy call"
    );
    assert_eq!(
        credentials.get("private_key").and_then(Value::as_str),
        Some("user-key"),
        "user private key must override host secret and be resolved before proxy call"
    );
    Ok(())
}

#[tokio::test]
async fn selected_user_credential_set_private_key_is_resolved_server_side()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let set_secret_key = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "ssh": [
                    {
                        "host": "192.168.0.10",
                        "port": SSH_PORT_DEFAULT,
                        "credentials": {
                            "username": "root"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(
                set_secret_key,
                "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n",
            )
            .with_bindings(
                credential_set_id,
                HashMap::from([(FIELD_KEY_SSH_PRIVATE_KEY, set_secret_key)]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Ssh,
                "Primary key",
                json!({}),
                true,
            )),
    );

    let entry_id = format!("user:config-context:ssh:0:{credential_set_id}");
    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Ssh,
        Some(session_metadata_with_entry_id(entry_id)),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    let private_key = credentials
        .get("private_key")
        .and_then(Value::as_str)
        .expect("private key present");

    assert_eq!(
        private_key,
        "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n",
        "selected user credential set secret must be resolved before proxy call"
    );
    assert_ne!(
        private_key,
        set_secret_key.to_string(),
        "private key UUID link must not leak into SSH launch payload"
    );
    Ok(())
}

#[tokio::test]
async fn selected_rdp_user_credential_set_preserves_params_username_without_username_secret()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host_secret_password = Uuid::now_v7();
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "rdp": [
                    {
                        "host": "10.10.1.230",
                        "port": RDP_PORT_DEFAULT,
                        "credentials": {
                            "username": "host-user",
                            "password": host_secret_password.to_string()
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(host_secret_password, "host-pass")
            .with_secret_value(user_secret_password, "user-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([(FIELD_KEY_RDP_PASSWORD, user_secret_password)]),
            )
            .with_host_secret_ids([host_secret_password])
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Rdp,
                "RDP set",
                json!({
                    "credentials": {
                        "username": "set-user"
                    }
                }),
                false,
            )),
    );

    let entry_id = format!("user:config-context:rdp:0:{credential_set_id}");
    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Rdp,
        Some(session_metadata_with_entry_id(entry_id)),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");

    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("set-user"),
        "rdp username from set params must remain when no username secret binding exists"
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("user-pass"),
        "rdp password must be resolved from selected user set secret binding"
    );
    Ok(())
}

#[tokio::test]
async fn default_ssh_user_credential_set_merges_with_context_entry()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "ssh": [
                    {
                        "host": "10.10.1.230",
                        "port": SSH_PORT_DEFAULT,
                        "credentials": {
                            "username": "root"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_password, "ssh-user-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([(FIELD_KEY_SSH_PASSWORD, user_secret_password)]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Ssh,
                "SSH default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
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
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("root"),
        "context username must remain"
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("ssh-user-pass"),
        "default personal set password must be merged and resolved"
    );
    Ok(())
}

#[tokio::test]
async fn selected_ssh_user_credential_set_requires_shared_connection_config() {
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "host": {
            "name": "ssh-only-personal"
        }
    });

    let ConnectionServiceFixture { service, .. } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config).with_user_set(test_user_set(
            credential_set_id,
            user_id,
            ConnectionProtocol::Ssh,
            "SSH personal only",
            json!({
                "host": LOOPBACK_HOST,
                "port": SSH_PORT_DEFAULT,
                "credentials": {
                    "username": "personal-user"
                }
            }),
            false,
        )),
    );

    let entry_id = format!("user:config-context:ssh:0:{credential_set_id}");
    let error = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Ssh,
        Some(session_metadata_with_entry_id(entry_id)),
    )
    .await
    .expect_err("personal credentials do not create protocol availability");

    match error {
        AppError::Validation(message) => assert!(
            message.contains("No default ssh connection config found"),
            "unexpected validation message: {message}"
        ),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn default_rdp_user_credential_set_merges_with_context_entry()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_username = Uuid::now_v7();
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "rdp": [
                    {
                        "host": "10.5.0.5",
                        "port": RDP_PORT_DEFAULT
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_username, "m.limarenko@grad.net")
            .with_secret_value(user_secret_password, "Pass1806")
            .with_bindings(
                credential_set_id,
                HashMap::from([
                    (FIELD_KEY_RDP_USERNAME, user_secret_username),
                    (FIELD_KEY_RDP_PASSWORD, user_secret_password),
                ]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Rdp,
                "RDP default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Rdp,
        Some(session_metadata_with_entry_id(
            ENTRY_ID_CONFIG_CONTEXT_RDP_PRIMARY,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    assert_eq!(
        payload.get("host").and_then(Value::as_str),
        Some("10.5.0.5"),
        "context host must be used"
    );
    assert_eq!(
        payload.get("port").and_then(Value::as_i64),
        Some(RDP_PORT_DEFAULT)
    );
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("m.limarenko@grad.net"),
        "default personal username must be merged and resolved"
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("Pass1806"),
        "default personal password must be merged and resolved"
    );
    Ok(())
}

#[tokio::test]
async fn default_rdp_user_credential_set_does_not_override_context_credentials()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_username = Uuid::now_v7();
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "rdp": [
                    {
                        "host": "192.168.0.3",
                        "port": RDP_PORT_DEFAULT,
                        "credentials": {
                            "username": "leader",
                            "password": "pass1806"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_username, "m.limarenko@grad.net")
            .with_secret_value(user_secret_password, "not-used-default-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([
                    (FIELD_KEY_RDP_USERNAME, user_secret_username),
                    (FIELD_KEY_RDP_PASSWORD, user_secret_password),
                ]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Rdp,
                "RDP default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Rdp,
        Some(session_metadata_with_entry_id(
            ENTRY_ID_CONFIG_CONTEXT_RDP_PRIMARY,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("leader"),
        "default personal username must not override context username"
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("pass1806"),
        "default personal password must not override context password"
    );
    Ok(())
}

#[tokio::test]
async fn selected_default_rdp_user_credential_set_overrides_context_credentials()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_username = Uuid::now_v7();
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "rdp": [
                    {
                        "host": "192.168.0.3",
                        "port": RDP_PORT_DEFAULT,
                        "credentials": {
                            "username": "leader",
                            "password": "pass1806"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_username, "m.limarenko@grad.net")
            .with_secret_value(user_secret_password, "not-used-default-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([
                    (FIELD_KEY_RDP_USERNAME, user_secret_username),
                    (FIELD_KEY_RDP_PASSWORD, user_secret_password),
                ]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Rdp,
                "RDP default",
                json!({}),
                true,
            )),
    );

    let entry_id = format!("user:config-context:rdp:0:{credential_set_id}");
    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Rdp,
        Some(session_metadata_with_entry_id(entry_id)),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("m.limarenko@grad.net"),
        "explicitly selected personal username must override context username"
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("not-used-default-pass"),
        "explicitly selected personal password must override context password"
    );
    Ok(())
}

#[tokio::test]
async fn default_rdp_user_credential_set_overrides_context_credentials_for_personal_first_policy()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_username = Uuid::now_v7();
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "rdp": [
                    {
                        "host": "192.168.0.3",
                        "port": RDP_PORT_DEFAULT,
                        "credentials": {
                            "username": "leader",
                            "password": "pass1806"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_username, "m.limarenko@grad.net")
            .with_secret_value(user_secret_password, "personal-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([
                    (FIELD_KEY_RDP_USERNAME, user_secret_username),
                    (FIELD_KEY_RDP_PASSWORD, user_secret_password),
                ]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Rdp,
                "RDP default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Rdp,
        Some(session_metadata_with_policy(
            ENTRY_ID_CONFIG_CONTEXT_RDP_PRIMARY,
            SESSION_MERGE_POLICY_PERSONAL_FIRST,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("username").and_then(Value::as_str),
        Some("m.limarenko@grad.net"),
        "personal-first policy must override context username"
    );
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("personal-pass"),
        "personal-first policy must override context password"
    );
    Ok(())
}

#[tokio::test]
async fn default_vnc_user_credential_set_merges_with_context_entry()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "vnc": [
                    {
                        "host": "10.5.0.7",
                        "port": VNC_PORT_DEFAULT
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_password, "vnc-user-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([(FIELD_KEY_VNC_PASSWORD, user_secret_password)]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Vnc,
                "VNC default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Vnc,
        Some(session_metadata_with_entry_id(
            ENTRY_ID_CONFIG_CONTEXT_VNC_PRIMARY,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    assert_eq!(
        payload.get("host").and_then(Value::as_str),
        Some("10.5.0.7"),
        "context host must be used"
    );
    assert_eq!(
        payload.get("port").and_then(Value::as_i64),
        Some(VNC_PORT_DEFAULT)
    );
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("vnc-user-pass"),
        "default personal password must be merged and resolved"
    );
    Ok(())
}

#[tokio::test]
async fn default_vnc_user_credential_set_does_not_override_context_password_for_context_first()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "vnc": [
                    {
                        "host": "10.5.0.7",
                        "port": VNC_PORT_DEFAULT,
                        "credentials": {
                            "password": "context-vnc-pass"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_password, "personal-vnc-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([(FIELD_KEY_VNC_PASSWORD, user_secret_password)]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Vnc,
                "VNC default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Vnc,
        Some(session_metadata_with_entry_id(
            ENTRY_ID_CONFIG_CONTEXT_VNC_PRIMARY,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("context-vnc-pass"),
        "context-first policy must keep context password"
    );
    Ok(())
}

#[tokio::test]
async fn default_vnc_user_credential_set_overrides_context_password_for_personal_first_policy()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_secret_password = Uuid::now_v7();
    let credential_set_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();

    let host_config = json!({
        "piping": {
            "connections": {
                "vnc": [
                    {
                        "host": "10.5.0.7",
                        "port": VNC_PORT_DEFAULT,
                        "credentials": {
                            "password": "context-vnc-pass"
                        }
                    }
                ]
            }
        }
    });

    let ConnectionServiceFixture {
        service,
        proxy_client,
    } = build_connection_service_fixture(
        ConnectionServiceTestData::new(host_config)
            .with_secret_value(user_secret_password, "personal-vnc-pass")
            .with_bindings(
                credential_set_id,
                HashMap::from([(FIELD_KEY_VNC_PASSWORD, user_secret_password)]),
            )
            .with_user_set(test_user_set(
                credential_set_id,
                user_id,
                ConnectionProtocol::Vnc,
                "VNC default",
                json!({}),
                true,
            )),
    );

    let _session = create_session_for_test(
        &service,
        target_id,
        user_id,
        ConnectionProtocol::Vnc,
        Some(session_metadata_with_policy(
            ENTRY_ID_CONFIG_CONTEXT_VNC_PRIMARY,
            SESSION_MERGE_POLICY_PERSONAL_FIRST,
        )),
    )
    .await
    .expect(SESSION_CREATED_EXPECTATION);

    let payload = proxy_client.take_payload().await?;
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .expect("credentials object");
    assert_eq!(
        credentials.get("password").and_then(Value::as_str),
        Some("personal-vnc-pass"),
        "personal-first policy must override context password"
    );
    Ok(())
}
