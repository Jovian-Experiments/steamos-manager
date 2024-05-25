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
use zbus::connection::Connection;
use zbus::ConnectionBuilder;

use crate::daemon::{Daemon, DaemonContext};
use crate::ds_inhibit::Inhibitor;
use crate::manager::root::SteamOSManager;
use crate::path;
use crate::sls::ftrace::Ftrace;

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
pub(crate) struct RootConfig {
    pub services: RootServicesConfig,
}

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
pub(crate) struct RootServicesConfig {}

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
pub(crate) struct RootState {
    pub services: RootServicesState,
}

#[derive(Copy, Clone, Default, Deserialize, Serialize, Debug)]
pub(crate) struct RootServicesState {}

struct RootContext {}

impl DaemonContext for RootContext {
    type State = RootState;
    type Config = RootConfig;

    fn user_config_path(&self) -> Result<PathBuf> {
        Ok(path("/etc/steamos-manager"))
    }

    fn system_config_path(&self) -> Result<PathBuf> {
        Ok(path("/usr/lib/steamos-manager/system.d"))
    }

    fn state(&self) -> RootState {
        RootState::default()
    }

    async fn start(
        &mut self,
        _state: RootState,
        _config: RootConfig,
        _daemon: &mut Daemon<RootContext>,
    ) -> Result<()> {
        // Nothing to do yet
        Ok(())
    }

    async fn reload(
        &mut self,
        _config: RootConfig,
        _daemon: &mut Daemon<RootContext>,
    ) -> Result<()> {
        // Nothing to do yet
        Ok(())
    }
}

async fn create_connection() -> Result<Connection> {
    let connection = ConnectionBuilder::system()?
        .name("com.steampowered.SteamOSManager1")?
        .build()
        .await?;
    let manager = SteamOSManager::new(connection.clone()).await?;
    connection
        .object_server()
        .at("/com/steampowered/SteamOSManager1", manager)
        .await?;
    Ok(connection)
}

pub async fn daemon() -> Result<()> {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things. It implements com.steampowered.SteamOSManager1.RootManager interface

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

    let context = RootContext {};
    let mut daemon = Daemon::new(subscriber, connection.clone()).await?;

    let ftrace = Ftrace::init(connection.clone()).await?;
    daemon.add_service(ftrace);

    let inhibitor = Inhibitor::init().await?;
    daemon.add_service(inhibitor);

    daemon.run(context).await
}
