/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use std::future::Future;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

mod ds_inhibit;
mod error;
mod job;
mod manager;
mod process;
mod sls;
mod systemd;
mod thread;
mod udev;

pub mod cec;
pub mod daemon;
pub mod hardware;
pub mod power;
pub mod proxy;
pub mod wifi;

#[cfg(test)]
mod testing;

const API_VERSION: u32 = 9;

pub trait Service
where
    Self: Sized + Send,
{
    const NAME: &'static str;

    fn run(&mut self) -> impl Future<Output = Result<()>> + Send;

    fn shutdown(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    fn start(mut self, token: CancellationToken) -> impl Future<Output = Result<()>> + Send {
        async move {
            info!("Starting {}", Self::NAME);
            let res = tokio::select! {
                r = self.run() => r,
                _ = token.cancelled() => Ok(()),
            };
            if res.is_err() {
                warn!(
                    "{} encountered an error: {}",
                    Self::NAME,
                    res.as_ref().unwrap_err()
                );
                token.cancel();
            }
            info!("Shutting down {}", Self::NAME);
            self.shutdown().await.and(res)
        }
    }
}

#[cfg(not(test))]
pub fn path<S: AsRef<str>>(path: S) -> PathBuf {
    PathBuf::from(path.as_ref())
}

#[cfg(test)]
pub fn path<S: AsRef<str>>(path: S) -> PathBuf {
    let current_test = crate::testing::current();
    let test_path = current_test.path();
    PathBuf::from(test_path.as_os_str().to_str().unwrap())
        .join(path.as_ref().trim_start_matches('/'))
}

pub(crate) async fn write_synced<P: AsRef<Path>>(path: P, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(path.as_ref()).await?;
    file.write_all(bytes).await?;
    Ok(file.sync_data().await?)
}

pub(crate) fn read_comm(pid: u32) -> Result<String> {
    let comm = std::fs::read_to_string(path(format!("/proc/{}/comm", pid)))?;
    Ok(comm.trim_end().to_string())
}

pub(crate) fn get_appid(pid: u32) -> Result<Option<u64>> {
    let environ = std::fs::read_to_string(path(format!("/proc/{}/environ", pid)))?;
    for env_var in environ.split('\0') {
        let (key, value) = match env_var.split_once('=') {
            Some((k, v)) => (k, v),
            None => continue,
        };
        if key != "SteamGameId" {
            continue;
        }
        match value.parse() {
            Ok(appid) => return Ok(Some(appid)),
            Err(_) => break,
        };
    }

    let stat = std::fs::read_to_string(path(format!("/proc/{}/stat", pid)))?;
    let stat = match stat.rsplit_once(") ") {
        Some((_, v)) => v,
        None => return Ok(None),
    };
    let ppid = match stat.split(' ').nth(1) {
        Some(ppid) => ppid,
        None => return Err(anyhow!("stat data invalid")),
    };
    let ppid: u32 = ppid.parse()?;
    if ppid > 1 {
        get_appid(ppid)
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod test {
    use crate::testing;
    use std::fs;

    #[test]
    fn read_comm() {
        let h = testing::start();
        let path = h.test.path();
        fs::create_dir_all(path.join("proc/123456")).expect("create_dir_all");
        fs::write(path.join("proc/123456/comm"), "test\n").expect("write comm");

        assert_eq!(crate::read_comm(123456).expect("read_comm"), "test");
        assert!(crate::read_comm(123457).is_err());
    }

    #[test]
    fn appid_environ() {
        let h = testing::start();
        let path = h.test.path();
        fs::create_dir_all(path.join("proc/123456")).expect("create_dir_all");
        fs::write(
            path.join("proc/123456/environ"),
            "A=B\0SteamGameId=98765\0C=D",
        )
        .expect("write environ");

        assert_eq!(crate::get_appid(123456).expect("get_appid"), Some(98765));
        assert!(crate::get_appid(123457).is_err());
    }

    #[test]
    fn appid_parent_environ() {
        let h = testing::start();
        let path = h.test.path();
        fs::create_dir_all(path.join("proc/123456")).expect("create_dir_all");
        fs::write(
            path.join("proc/123456/environ"),
            "A=B\0SteamGameId=98765\0C=D",
        )
        .expect("write environ");
        fs::create_dir_all(path.join("proc/123457")).expect("create_dir_all");
        fs::write(path.join("proc/123457/environ"), "A=B\0C=D").expect("write environ");
        fs::write(path.join("proc/123457/stat"), "0 (comm) S 123456 ...").expect("write stat");

        assert_eq!(crate::get_appid(123457).expect("get_appid"), Some(98765));
    }

    #[test]
    fn appid_missing() {
        let h = testing::start();
        let path = h.test.path();
        fs::create_dir_all(path.join("proc/123457")).expect("create_dir_all");
        fs::write(path.join("proc/123457/environ"), "A=B\0C=D").expect("write environ");
        fs::write(path.join("proc/123457/stat"), "0 (comm) S 1 ...").expect("write stat");

        assert_eq!(crate::get_appid(123457).expect("get_appid"), None);
    }
}
