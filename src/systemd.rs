/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */
#![allow(dead_code)]

use anyhow::{anyhow, bail, Result};
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
    #[zbus(property)]
    fn unit_file_state(&self) -> Result<String>;

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
    #[allow(clippy::type_complexity)]
    async fn enable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
        force: bool,
    ) -> Result<(bool, Vec<(String, String, String)>)>;

    async fn disable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
    ) -> Result<Vec<(String, String, String)>>;

    async fn mask_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
    ) -> Result<Vec<(String, String, String)>>;

    async fn unmask_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
    ) -> Result<Vec<(String, String, String)>>;

    async fn reload(&self) -> Result<()>;
}

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum EnableState {
    Disbled,
    Enabled,
    Masked,
    Static,
}

pub struct SystemdUnit<'dbus> {
    connection: Connection,
    proxy: SystemdUnitProxy<'dbus>,
    name: String,
}

pub async fn daemon_reload(connection: &Connection) -> Result<()> {
    let proxy = SystemdManagerProxy::new(connection).await?;
    proxy.reload().await?;
    Ok(())
}

impl<'dbus> SystemdUnit<'dbus> {
    pub async fn new(connection: Connection, name: &str) -> Result<SystemdUnit<'dbus>> {
        let path = PathBuf::from("/org/freedesktop/systemd1/unit").join(escape(name));
        let path = String::from(path.to_str().ok_or(anyhow!("Unit name {name} invalid"))?);
        Ok(SystemdUnit {
            proxy: SystemdUnitProxy::builder(&connection)
                .cache_properties(zbus::CacheProperties::No)
                .path(path)?
                .build()
                .await?,
            connection,
            name: String::from(name),
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

    pub async fn enable(&self) -> Result<bool> {
        let manager = SystemdManagerProxy::new(&self.connection).await?;
        let (_, res) = manager
            .enable_unit_files(&[self.name.as_str()], false, false)
            .await?;
        Ok(!res.is_empty())
    }

    pub async fn disable(&self) -> Result<bool> {
        let manager = SystemdManagerProxy::new(&self.connection).await?;
        let res = manager
            .disable_unit_files(&[self.name.as_str()], false)
            .await?;
        Ok(!res.is_empty())
    }

    pub async fn mask(&self) -> Result<bool> {
        let manager = SystemdManagerProxy::new(&self.connection).await?;
        let res = manager
            .mask_unit_files(&[self.name.as_str()], false)
            .await?;
        Ok(!res.is_empty())
    }

    pub async fn unmask(&self) -> Result<bool> {
        let manager = SystemdManagerProxy::new(&self.connection).await?;
        let res = manager
            .unmask_unit_files(&[self.name.as_str()], false)
            .await?;
        Ok(!res.is_empty())
    }

    pub async fn active(&self) -> Result<bool> {
        Ok(self.proxy.active_state().await? == "active")
    }

    pub async fn enabled(&self) -> Result<EnableState> {
        Ok(match self.proxy.unit_file_state().await?.as_str() {
            "enabled" => EnableState::Enabled,
            "disabled" => EnableState::Disbled,
            "masked" => EnableState::Masked,
            "static" => EnableState::Static,
            state => bail!("Unknown state {state}"),
        })
    }
}

pub fn escape(name: &str) -> String {
    let mut parts = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            parts.push(c);
        } else {
            let escaped = format!("_{:02x}", u32::from(c));
            parts.push_str(escaped.as_str());
        }
    }
    parts
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_escape() {
        assert_eq!(escape("systemd"), "systemd");
        assert_eq!(escape("system d"), "system_20d");
    }
}
