/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::error;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, Registry};
#[cfg(not(test))]
use xdg::BaseDirectories;
use zbus::connection::Connection;
use zbus::ConnectionBuilder;

use crate::daemon::{channel, Daemon, DaemonContext};
use crate::manager::user::SteamOSManager;
use crate::path;
use crate::udev::UdevMonitor;

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
struct UserConfig {
    pub services: UserServicesConfig,
}

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
pub(crate) struct UserServicesConfig {}

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
struct UserState {
    pub services: UserServicesState,
}

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
pub(crate) struct UserServicesState {}

struct UserContext {
    session: Connection,
}

impl DaemonContext for UserContext {
    type State = UserState;
    type Config = UserConfig;
    type Command = ();

    #[cfg(not(test))]
    fn user_config_path(&self) -> Result<PathBuf> {
        let xdg_base = BaseDirectories::new()?;
        Ok(xdg_base.get_config_file("steamos-manager"))
    }

    #[cfg(test)]
    fn user_config_path(&self) -> Result<PathBuf> {
        Ok(path("steamos-manager"))
    }

    fn system_config_path(&self) -> Result<PathBuf> {
        Ok(path("/usr/share/steamos-manager/user.d"))
    }

    fn state(&self) -> UserState {
        UserState::default()
    }

    async fn start(
        &mut self,
        _state: UserState,
        _config: UserConfig,
        daemon: &mut Daemon<UserContext>,
    ) -> Result<()> {
        let udev = UdevMonitor::init(&self.session).await?;
        daemon.add_service(udev);

        Ok(())
    }

    async fn reload(
        &mut self,
        _config: UserConfig,
        _daemon: &mut Daemon<UserContext>,
    ) -> Result<()> {
        // Nothing to do yet
        Ok(())
    }

    async fn handle_command(
        &mut self,
        _cmd: Self::Command,
        _daemon: &mut Daemon<UserContext>,
    ) -> Result<()> {
        // Nothing to do yet
        Ok(())
    }
}

async fn create_connections() -> Result<(Connection, Connection)> {
    let system = Connection::system().await?;
    let connection = ConnectionBuilder::session()?
        .name("com.steampowered.SteamOSManager1")?
        .build()
        .await?;

    let manager = SteamOSManager::new(connection.clone(), &system).await?;
    connection
        .object_server()
        .at("/com/steampowered/SteamOSManager1", manager)
        .await?;

    Ok((connection, system))
}

pub async fn daemon() -> Result<()> {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things. It implements com.steampowered.SteamOSManager1.Manager interface

    let stdout_log = fmt::layer();
    let subscriber = Registry::default().with(stdout_log);
    let (_tx, rx) = channel::<UserContext>();

    let (session, system) = match create_connections().await {
        Ok(c) => c,
        Err(e) => {
            let _guard = tracing::subscriber::set_default(subscriber);
            error!("Error connecting to DBus: {}", e);
            bail!(e);
        }
    };

    let context = UserContext { session };
    let mut daemon = Daemon::new(subscriber, system, rx).await?;

    daemon.run(context).await
}
