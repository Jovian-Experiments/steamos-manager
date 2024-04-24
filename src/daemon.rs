/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use tokio::signal::unix::{signal, Signal, SignalKind};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use zbus::connection::Connection;

use crate::sls::{LogLayer, LogReceiver};
use crate::{reload, Service};

pub struct Daemon {
    services: JoinSet<Result<()>>,
    token: CancellationToken,
    sigterm: Signal,
    sigquit: Signal,
}

impl Daemon {
    pub async fn new<S: SubscriberExt + Send + Sync + for<'a> LookupSpan<'a>>(
        subscriber: S,
        connection: Connection,
    ) -> Result<Daemon> {
        let services = JoinSet::new();
        let token = CancellationToken::new();

        let log_receiver = LogReceiver::new(connection.clone()).await?;
        let remote_logger = LogLayer::new(&log_receiver).await;
        let subscriber = subscriber.with(remote_logger);
        tracing::subscriber::set_global_default(subscriber)?;

        let sigterm = signal(SignalKind::terminate())?;
        let sigquit = signal(SignalKind::quit())?;

        let mut daemon = Daemon {
            services,
            token,
            sigterm,
            sigquit,
        };
        daemon.add_service(log_receiver);

        Ok(daemon)
    }

    pub fn add_service<S: Service + 'static>(&mut self, service: S) {
        let token = self.token.clone();
        self.services
            .spawn(async move { service.start(token).await });
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut res = tokio::select! {
            e = self.services.join_next() => match e.unwrap() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(e.into())
            },
            _ = tokio::signal::ctrl_c() => Ok(()),
            e = self.sigterm.recv() => e.ok_or(anyhow!("SIGTERM machine broke")),
            _ = self.sigquit.recv() => Err(anyhow!("Got SIGQUIT")),
            e = reload() => e,
        }
        .inspect_err(|e| error!("Encountered error running: {e}"));
        self.token.cancel();

        info!("Shutting down");

        while let Some(service_res) = self.services.join_next().await {
            res = match service_res {
                Ok(Err(e)) => Err(e),
                Err(e) => Err(e.into()),
                _ => continue,
            };
        }

        res.inspect_err(|e| error!("Encountered error: {e}"))
    }
}
