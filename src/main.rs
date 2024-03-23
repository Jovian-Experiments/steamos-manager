/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{Error, Result};
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber;
use zbus::ConnectionBuilder;

mod manager;

#[tokio::main]
async fn main() -> Result<()> {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things. It implements com.steampowered.SteamOSManager1 interface

    tracing_subscriber::fmt::init();

    let mut sigterm = signal(SignalKind::terminate())?;

    let manager = manager::SMManager::new()?;

    let _system_connection = ConnectionBuilder::system()?
        .name("com.steampowered.SteamOSManager1")?
        .serve_at("/com/steampowered/SteamOSManager1", manager)?
        .build()
        .await?;

    tokio::select! {
        e = sigterm.recv() => e.ok_or(Error::msg("SIGTERM pipe broke")),
        e = tokio::signal::ctrl_c() => Ok(e?),
    }
}
