/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, ensure, Result};
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::unistd::pipe;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use tokio::net::unix::pipe::Sender;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::debug;
use udev::{Event, EventType, MonitorBuilder};
use zbus::{self, interface, Connection, InterfaceRef, SignalContext};

use crate::thread::spawn;
use crate::Service;

const PATH: &str = "/com/steampowered/SteamOSManager1/UdevEvents";

pub(crate) struct UdevMonitor
where
    Self: 'static + Send,
{
    shutdown_sender: Sender,
    shutdown_receiver: Option<OwnedFd>,
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
        let mut handle = spawn(move || run_udev(ev_sender, shutdown_receiver));

        loop {
            let handle = &mut handle;
            let ev = tokio::select! {
                r = handle => break r,
                r = ev_receiver.recv() => r.ok_or(anyhow!("udev event pipe broke"))?,
            };
            match ev {
                UdevEvent::OverCurrent {
                    devpath,
                    port,
                    count,
                } => {
                    UdevDbusObject::over_current(
                        self.udev_object.signal_context(),
                        devpath.as_str(),
                        port.as_str(),
                        count,
                    )
                    .await?
                }
            }
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.shutdown_sender.try_write(&[0u8])?;
        Ok(())
    }
}

impl UdevMonitor {
    pub async fn init(connection: &Connection) -> Result<UdevMonitor> {
        let object_server = connection.object_server();
        ensure!(
            object_server.at(PATH, UdevDbusObject {}).await?,
            "Could not register UdevEvents"
        );
        let udev_object: InterfaceRef<UdevDbusObject> = object_server.interface(PATH).await?;
        let (shutdown_receiver, shutdown_sender) = pipe()?;
        Ok(UdevMonitor {
            udev_object,
            shutdown_sender: Sender::from_owned_fd(shutdown_sender)?,
            shutdown_receiver: Some(shutdown_receiver),
        })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.UdevEvents")]
impl UdevDbusObject {
    #[zbus(signal)]
    async fn over_current(
        signal_ctxt: &SignalContext<'_>,
        devpath: &str,
        port: &str,
        count: u64,
    ) -> zbus::Result<()>;
}

fn run_udev(tx: UnboundedSender<UdevEvent>, rx: OwnedFd) -> Result<()> {
    let usb_monitor = MonitorBuilder::new()?
        .match_subsystem_devtype("usb", "usb_interface")?
        .listen()?;
    let fd = usb_monitor.as_fd();
    let mut iter = usb_monitor.iter();
    let ev_poller = PollFd::new(fd, PollFlags::POLLIN);
    let shutdown_poller = PollFd::new(rx.as_fd(), PollFlags::POLLIN);
    debug!(
        "Listening on event poller {} and shutdown poller {}",
        ev_poller.as_fd().as_raw_fd(),
        shutdown_poller.as_fd().as_raw_fd()
    );
    loop {
        let fds = &mut [ev_poller, shutdown_poller];
        let ret = poll(fds, PollTimeout::NONE)?;
        if ret < 0 {
            return Err(std::io::Error::from_raw_os_error(-ret).into());
        }
        let [ev_poller, shutdown_poller] = fds;
        match ev_poller.any() {
            None => bail!("Event poller encountered unknown flags"),
            Some(true) => {
                let ev = iter
                    .next()
                    .ok_or(anyhow!("Poller said event was present, but it was not"))?;
                process_usb_event(ev, &tx)?;
            }
            Some(false) => (),
        }
        match shutdown_poller.any() {
            None => bail!("Shutdown poller encountered unknown flags"),
            Some(true) => break Ok(()),
            Some(false) => (),
        }
    }
}

fn process_usb_event(ev: Event, tx: &UnboundedSender<UdevEvent>) -> Result<()> {
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
