/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, Result};
use tracing::error;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, Registry};
use zbus::connection::Connection;
use zbus::ConnectionBuilder;

use crate::daemon::Daemon;

async fn create_connection() -> Result<Connection> {
    let connection = ConnectionBuilder::session()?
        .name("com.steampowered.SteamOSManager1")?
        .build()
        .await?;
    Ok(connection)
}

pub async fn daemon() -> Result<()> {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things. It implements com.steampowered.SteamOSManager1.Manager interface

    let stdout_log = fmt::layer();
    let subscriber = Registry::default().with(stdout_log);

    let system = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            let _guard = tracing::subscriber::set_default(subscriber);
            error!("Error connecting to DBus: {}", e);
            bail!(e);
        }
    };
    let session = match create_connection().await {
        Ok(c) => c,
        Err(e) => {
            let _guard = tracing::subscriber::set_default(subscriber);
            error!("Error connecting to DBus: {}", e);
            bail!(e);
        }
    };

    let mut daemon = Daemon::new(subscriber, system.clone()).await?;

    daemon.run().await
}
