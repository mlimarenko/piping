//! Device-management driver adapters, media transport modules, and helper process spawning.
use crate::contexts::device_management::domain::DriverError;
use std::time::Duration;
use tokio::process::{Child, Command};

pub mod drivers;
pub mod http_client;
pub mod media_budget;
pub mod remux;
pub mod resolver;
pub mod rtsp_clip;
pub mod rtsp_live;
pub mod rtsp_still;
pub mod transport;

/// Spawns an external media helper process (ffmpeg / ffprobe / test wrapper),
/// briefly retrying if Linux reports `ETXTBSY` (os error 26) due to a concurrent
/// fork in another thread inheriting an open write descriptor.
pub(crate) async fn spawn_media_process(cmd: &mut Command) -> Result<Child, DriverError> {
    for _ in 0..10 {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(err) if err.raw_os_error() == Some(26) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(err) => {
                return Err(DriverError::Transport(format!(
                    "ffmpeg spawn failed: {err}"
                )));
            }
        }
    }
    cmd.spawn()
        .map_err(|err| DriverError::Transport(format!("ffmpeg spawn failed: {err}")))
}
