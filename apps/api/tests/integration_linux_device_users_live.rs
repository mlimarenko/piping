//! Live check of the Linux device-users driver against a real sshd.
//!
//! Ignored by default. Point it at the `ssh-target` compose profile:
//! `PIPING_LIVE_LINUX_HOST`, `PIPING_LIVE_LINUX_PORT`, `PIPING_LIVE_LINUX_USER`,
//! `PIPING_LIVE_LINUX_PASSWORD` (also used as the sudo password). The test-only
//! transport shells out to `sshpass`/`ssh`; production execution goes through
//! the proxy runtime.

use std::process::Stdio;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use piping_backend::contexts::device_management::application::{
    SshExecOutput, SshExecPort, SshExecRequest,
};
use piping_backend::contexts::device_management::domain::{
    DeviceDriver, DriverError, NewUser, Secret, UserUpdate,
};
use piping_backend::contexts::device_management::infrastructure::drivers::linux::LinuxDriver;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use uuid::Uuid;

struct LiveSsh {
    host: String,
    port: String,
    user: String,
    password: String,
    commands: Mutex<Vec<String>>,
}

#[async_trait]
impl SshExecPort for LiveSsh {
    async fn exec(&self, request: SshExecRequest) -> Result<SshExecOutput, DriverError> {
        self.commands
            .lock()
            .map_err(|_| DriverError::Transport("command log poisoned".to_owned()))?
            .push(request.command.clone());
        let mut child = Command::new("sshpass")
            .arg("-e")
            .arg("ssh")
            .args([
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
            ])
            .args(["-o", "PubkeyAuthentication=no", "-o", "IdentitiesOnly=yes"])
            .args(["-o", "PreferredAuthentications=password"])
            .args(["-o", "LogLevel=ERROR", "-p", &self.port])
            .arg(format!("{}@{}", self.user, self.host))
            .arg(&request.command)
            .env("SSHPASS", &self.password)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| DriverError::Transport(error.to_string()))?;
        if let Some(mut stdin) = child.stdin.take()
            && let Some(bytes) = request.stdin
        {
            stdin
                .write_all(&bytes)
                .await
                .map_err(|error| DriverError::Transport(error.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|error| DriverError::Transport(error.to_string()))?;
        Ok(SshExecOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code(),
            success: output.status.success(),
            timed_out: false,
        })
    }
}

fn env(name: &str) -> Result<String, std::env::VarError> {
    std::env::var(name)
}

#[tokio::test]
#[ignore = "requires the ssh-target compose profile"]
async fn live_linux_user_lifecycle() -> Result<(), std::env::VarError> {
    let password = env("PIPING_LIVE_LINUX_PASSWORD")?;
    let ssh = Arc::new(LiveSsh {
        host: env("PIPING_LIVE_LINUX_HOST")?,
        port: env("PIPING_LIVE_LINUX_PORT")?,
        user: env("PIPING_LIVE_LINUX_USER")?,
        password: password.clone(),
        commands: Mutex::new(Vec::new()),
    });
    let driver = LinuxDriver::new(
        ssh.clone(),
        Uuid::now_v7(),
        env("PIPING_LIVE_LINUX_USER")?,
        Some(Secret::new(password.clone())),
    );
    let name = format!("pwa64t{}", &Uuid::now_v7().simple().to_string()[24..]);
    let user_secret = format!("Pw-{}", Uuid::now_v7().simple());

    let created = driver
        .create_users(&[NewUser {
            name: name.clone(),
            password: Secret::new(user_secret.clone()),
            group: None,
            authorities: Vec::new(),
        }])
        .await
        .expect("create");
    assert_eq!(created.succeeded, vec![name.clone()], "{created:?}");
    let listed = driver.list_users().await.expect("list after create");
    assert!(listed.iter().any(|user| user.name == name));

    let updated = driver
        .update_users(&[UserUpdate {
            name: name.clone(),
            password: Some(Secret::new(format!("{user_secret}x"))),
            group: None,
            authorities: Some(vec!["lock".to_owned()]),
        }])
        .await
        .expect("update");
    assert_eq!(updated.succeeded, vec![name.clone()], "{updated:?}");

    let deleted = driver
        .delete_users(std::slice::from_ref(&name))
        .await
        .expect("delete");
    assert_eq!(deleted.succeeded, vec![name.clone()], "{deleted:?}");
    let listed = driver.list_users().await.expect("list after delete");
    assert!(!listed.iter().any(|user| user.name == name));

    let commands = ssh.commands.lock().expect("command log").clone();
    assert!(
        commands
            .iter()
            .all(|command| !command.contains(&user_secret) && !command.contains(&password)),
        "a secret reached the command line"
    );
    Ok(())
}
