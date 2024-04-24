/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, Result};
use tokio::signal::unix::{signal, SignalKind};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, Registry};
use zbus::connection::Connection;
use zbus::ConnectionBuilder;

use crate::ds_inhibit::Inhibitor;
use crate::{manager, reload, Service};
use crate::sls::ftrace::Ftrace;
use crate::sls::{LogLayer, LogReceiver};

async fn create_connection() -> Result<Connection> {
    let connection = ConnectionBuilder::system()?
        .name("com.steampowered.SteamOSManager1")?
        .build()
        .await?;
    let manager = manager::SteamOSManager::new(connection.clone()).await?;
    connection
        .object_server()
        .at("/com/steampowered/SteamOSManager1", manager)
        .await?;
    Ok(connection)
}

pub async fn daemon() -> Result<()> {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things. It implements com.steampowered.SteamOSManager1.Manager interface

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
    tracing::subscriber::set_global_default(subscriber)?;

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
        e = sigterm.recv() => e.ok_or(anyhow!("SIGTERM machine broke")),
        _ = sigquit.recv() => Err(anyhow!("Got SIGQUIT")),
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
