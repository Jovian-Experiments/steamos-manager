/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice shall be included
 * in all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
 * CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
 * TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
 * SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

use anyhow::{bail, Error, Result};
use tokio::signal::unix::{signal, SignalKind};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, Registry};
use zbus::connection::Connection;
use zbus::ConnectionBuilder;

use crate::ds_inhibit::Inhibitor;
use crate::sls::ftrace::Ftrace;
use crate::sls::{LogLayer, LogReceiver};

mod ds_inhibit;
mod manager;
mod sls;

#[cfg(test)]
mod testing;

trait Service
where
    Self: Sized,
{
    const NAME: &'static str;

    async fn run(&mut self) -> Result<()>;

    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    async fn start(mut self, token: CancellationToken) -> Result<()> {
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

#[cfg(not(test))]
pub fn sysbase() -> String {
    String::new()
}

#[cfg(test)]
pub fn sysbase() -> String {
    let current_test = crate::testing::current();
    let path = current_test.path();
    String::from(path.as_os_str().to_str().unwrap())
}

pub fn read_comm(pid: u32) -> Result<String> {
    let comm = std::fs::read_to_string(format!("{}/proc/{}/comm", sysbase(), pid))?;
    Ok(comm.trim_end().to_string())
}

pub fn get_appid(pid: u32) -> Result<Option<u64>> {
    let environ = std::fs::read_to_string(format!("{}/proc/{}/environ", sysbase(), pid))?;
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

    let stat = std::fs::read_to_string(format!("{}/proc/{}/stat", sysbase(), pid))?;
    let stat = match stat.rsplit_once(") ") {
        Some((_, v)) => v,
        None => return Ok(None),
    };
    let ppid = match stat.split(' ').nth(1) {
        Some(ppid) => ppid,
        None => return Err(anyhow::Error::msg("stat data invalid")),
    };
    let ppid: u32 = ppid.parse()?;
    if ppid > 1 {
        get_appid(ppid)
    } else {
        Ok(None)
    }
}

async fn reload() -> Result<()> {
    loop {
        let mut sighup = signal(SignalKind::hangup())?;
        sighup.recv().await.ok_or(Error::msg(""))?;
    }
}

async fn create_connection() -> Result<Connection> {
    let manager = manager::SMManager::new()?;

    ConnectionBuilder::system()?
        .name("com.steampowered.SteamOSManager1")?
        .serve_at("/com/steampowered/SteamOSManager1", manager)?
        .build()
        .await
        .map_err(|e| e.into())
}

#[tokio::main]
async fn main() -> Result<()> {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things. It implements com.steampowered.SteamOSManager1 interface

    let stdout_log = fmt::layer();
    let subscriber = Registry::default().with(stdout_log);

    let connection = match create_connection().await {
        Ok(c) => c,
        Err(e) => {
            let _guard = tracing::subscriber::set_default(subscriber);
            error!("Error connecting to DBus: {}", e);
            bail!(e);
        }
    };

    let mut services = JoinSet::new();
    let token = CancellationToken::new();

    let mut log_receiver = LogReceiver::new(connection.clone()).await?;
    let remote_logger = LogLayer::new(&log_receiver).await;
    let subscriber = subscriber.with(remote_logger);
    let _guard = tracing::subscriber::set_global_default(subscriber)?;

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigquit = signal(SignalKind::quit())?;

    let ftrace = Ftrace::init(connection.clone()).await?;
    services.spawn(ftrace.start(token.clone()));

    let inhibitor = Inhibitor::init().await?;
    services.spawn(inhibitor.start(token.clone()));

    let mut res = tokio::select! {
        e = log_receiver.run() => e,
        e = services.join_next() => match e.unwrap() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(e.into())
        },
        _ = tokio::signal::ctrl_c() => Ok(()),
        e = sigterm.recv() => e.ok_or(Error::msg("SIGTERM machine broke")),
        _ = sigquit.recv() => Err(Error::msg("Got SIGQUIT")),
        e = reload() => e,
    }
    .inspect_err(|e| error!("Encountered error running: {e}"));
    token.cancel();

    info!("Shutting down");

    while let Some(service_res) = services.join_next().await {
        res = match service_res {
            Ok(Err(e)) => Err(e),
            Err(e) => Err(e.into()),
            _ => continue,
        };
    }

    res.inspect_err(|e| error!("Encountered error: {e}"))
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
