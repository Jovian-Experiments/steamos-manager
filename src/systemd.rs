/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Unit",
    default_service = "org.freedesktop.systemd1"
)]
trait SystemdUnit {
    #[zbus(property)]
    fn active_state(&self) -> Result<String>;

    async fn restart(&self, mode: &str) -> Result<OwnedObjectPath>;
    async fn start(&self, mode: &str) -> Result<OwnedObjectPath>;
    async fn stop(&self, mode: &str) -> Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
    async fn reload(&self) -> Result<()>;
}

pub struct SystemdUnit<'dbus> {
    proxy: SystemdUnitProxy<'dbus>,
}

pub async fn daemon_reload(connection: &Connection) -> Result<()> {
    let proxy = SystemdManagerProxy::new(&connection).await?;
    proxy.reload().await?;
    Ok(())
}

impl<'dbus> SystemdUnit<'dbus> {
    pub async fn new(connection: Connection, name: &str) -> Result<SystemdUnit<'dbus>> {
        let path = PathBuf::from("/org/freedesktop/systemd1/unit").join(name);
        let path = String::from(path.to_str().ok_or(anyhow!("Unit name {name} invalid"))?);
        Ok(SystemdUnit {
            proxy: SystemdUnitProxy::builder(&connection)
                .cache_properties(zbus::CacheProperties::No)
                .path(path)?
                .build()
                .await?,
        })
    }

    pub async fn restart(&self) -> Result<()> {
        self.proxy.restart("fail").await?;
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.proxy.start("fail").await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.proxy.stop("fail").await?;
        Ok(())
    }

    pub async fn active(&self) -> Result<bool> {
        Ok(self.proxy.active_state().await? == "active")
    }
}
