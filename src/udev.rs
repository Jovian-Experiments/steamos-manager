/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, ensure, Result};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;
use tokio::select;
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, Sender, UnboundedSender};
use tokio::task::{spawn, JoinHandle};
use tokio::time::sleep;
use tracing::debug;
use udev::{Event, EventType, MonitorBuilder};
use zbus::object_server::{InterfaceRef, SignalEmitter};
use zbus::{self, interface, Connection};

use crate::Service;

const PATH: &str = "/com/steampowered/SteamOSManager1";

pub(crate) struct UdevMonitor
where
    Self: 'static + Send,
{
    shutdown_sender: Sender<()>,
    shutdown_receiver: Option<Receiver<()>>,
    udev_object: InterfaceRef<UdevDbusObject>,
}

struct UdevDbusObject
where
    Self: 'static + Send, {}

#[derive(Debug)]
enum UdevEvent {
    OverCurrent {
        devpath: String,
        port: String,
        count: u64,
    },
}

impl Service for UdevMonitor {
    const NAME: &'static str = "udev-monitor";

    async fn run(&mut self) -> Result<()> {
        let (ev_sender, mut ev_receiver) = unbounded_channel();
        let shutdown_receiver = self
            .shutdown_receiver
            .take()
            .ok_or(anyhow!("UdevMonitor cannot be run twice"))?;
        let mut handle = spawn(run_udev(ev_sender, shutdown_receiver));

        loop {
            let handle = &mut handle;
            let ev = tokio::select! {
                r = handle => break r?,
                r = ev_receiver.recv() => r.ok_or(anyhow!("udev event pipe broke"))?,
            };
            match ev {
                UdevEvent::OverCurrent {
                    devpath,
                    port,
                    count,
                } => {
                    UdevDbusObject::usb_over_current(
                        self.udev_object.signal_emitter(),
                        devpath.as_str(),
                        port.as_str(),
                        count,
                    )
                    .await?;
                }
            }
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.shutdown_sender.send(()).await;
        Ok(())
    }
}

impl UdevMonitor {
    pub async fn init(connection: &Connection) -> Result<UdevMonitor> {
        let object_server = connection.object_server();
        ensure!(
            object_server.at(PATH, UdevDbusObject {}).await?,
            "Could not register UdevEvents1"
        );
        let udev_object: InterfaceRef<UdevDbusObject> = object_server.interface(PATH).await?;
        let (shutdown_sender, shutdown_receiver) = channel(1);
        Ok(UdevMonitor {
            udev_object,
            shutdown_sender,
            shutdown_receiver: Some(shutdown_receiver),
        })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.UdevEvents1")]
impl UdevDbusObject {
    #[zbus(signal)]
    async fn usb_over_current(
        signal_ctxt: &SignalEmitter<'_>,
        devpath: &str,
        port: &str,
        count: u64,
    ) -> zbus::Result<()>;
}

async fn run_udev(tx: UnboundedSender<UdevEvent>, mut shutdown_rx: Receiver<()>) -> Result<()> {
    let usb_monitor = MonitorBuilder::new()?
        .match_subsystem_devtype("usb", "usb_interface")?
        .listen()?;
    let fd = AsyncFd::new(usb_monitor.as_fd())?;
    let mut iter = usb_monitor.iter();
    loop {
        select! {
            guard = fd.ready(Interest::READABLE) => {
                let mut guard = guard?;
                for ev in iter.by_ref() {
                    process_usb_event(&ev, &tx)?;
                };
                guard.clear_ready();
            },
            _ = shutdown_rx.recv() => break Ok(()),
            _ = fd.ready(Interest::ERROR) => bail!("Event poller encountered unknown flags"),
        }
    }
}

pub(crate) fn single_poll<F>(
    subsystem: &str,
    callback: F,
    timeout: Duration,
) -> Result<JoinHandle<Result<PathBuf>>>
where
    F: Fn(&Event) -> bool + Send + 'static,
{
    let monitor = MonitorBuilder::new()?
        .match_subsystem(subsystem)?
        .listen()?;
    let handle = spawn(async move {
        let fd = AsyncFd::new(monitor.as_fd())?;
        let mut iter = monitor.iter();
        loop {
            select! {
                _ = sleep(timeout) => bail!("Udev poller timed out"),
                guard = fd.ready(Interest::READABLE) => {
                    let mut guard = guard?;
                    for ev in iter.by_ref() {
                        if callback(&ev) {
                            return Ok(ev.syspath().to_path_buf());
                        }
                    };
                    guard.clear_ready();
                },
                _ = fd.ready(Interest::ERROR) => bail!("Udev poller encountered unknown flags"),
            };
        }
    });
    Ok(handle)
}

fn process_usb_event(ev: &Event, tx: &UnboundedSender<UdevEvent>) -> Result<()> {
    debug!("Got USB event {ev:?}");
    if ev.event_type() != EventType::Change {
        return Ok(());
    }
    let port = match ev.property_value("OVER_CURRENT_PORT") {
        None => return Ok(()),
        Some(port) => port.to_string_lossy().to_string(),
    };
    let count: u64 = match ev.property_value("OVER_CURRENT_COUNT") {
        None => return Ok(()),
        Some(count) => count.to_string_lossy().parse()?,
    };
    let devpath = ev.devpath().to_string_lossy().to_string();
    tx.send(UdevEvent::OverCurrent {
        devpath,
        port,
        count,
    })?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;
    use zbus::object_server::Interface;

    use crate::testing;

    #[tokio::test]
    async fn test_interface_matches() {
        let mut handle = testing::start();
        let connection = handle.new_dbus().await.expect("new_dbus");
        sleep(Duration::from_millis(1)).await;
        let object_server = connection.object_server();
        object_server.at(PATH, UdevDbusObject {}).await.expect("at");

        let remote =
            testing::InterfaceIntrospection::from_remote::<UdevDbusObject, _>(&connection, PATH)
                .await
                .expect("remove");
        let local = testing::InterfaceIntrospection::from_local(
            "com.steampowered.SteamOSManager1.xml",
            UdevDbusObject::name().to_string(),
        )
        .await
        .expect("local");
        assert!(remote.compare(&local));
    }
}
