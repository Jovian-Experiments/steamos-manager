/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, ensure, Result};
use tokio::signal::unix::{signal, SignalKind};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use zbus::connection::Connection;

use crate::daemon::config::read_config;
use crate::sls::{LogLayer, LogReceiver};
use crate::Service;

mod config;
mod root;
mod user;

pub use root::daemon as root;
pub use user::daemon as user;

pub(crate) struct Daemon {
    services: JoinSet<Result<()>>,
    token: CancellationToken,
}

impl Daemon {
    pub(crate) async fn new<S: SubscriberExt + Send + Sync + for<'a> LookupSpan<'a>>(
        subscriber: S,
        connection: Connection,
    ) -> Result<Daemon> {
        let services = JoinSet::new();
        let token = CancellationToken::new();

        let log_receiver = LogReceiver::new(connection.clone()).await?;
        let remote_logger = LogLayer::new(&log_receiver).await;
        let subscriber = subscriber.with(remote_logger);
        tracing::subscriber::set_global_default(subscriber)?;

        let mut daemon = Daemon { services, token };
        daemon.add_service(log_receiver);

        Ok(daemon)
    }

    pub(crate) fn add_service<S: Service + 'static>(&mut self, service: S) -> CancellationToken {
        let token = self.token.child_token();
        let moved_token = token.clone();
        self.services
            .spawn(async move { service.start(moved_token).await });
        token
    }

    pub(crate) async fn run(&mut self) -> Result<()> {
        ensure!(
            !self.services.is_empty(),
            "Can't run a daemon with no services attached."
        );

        let mut res = loop {
            let mut sigterm = signal(SignalKind::terminate())?;
            let mut sigquit = signal(SignalKind::quit())?;
            let mut sighup = signal(SignalKind::hangup())?;

            let res = tokio::select! {
                e = self.services.join_next() => match e.unwrap() {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.into())
                },
                _ = tokio::signal::ctrl_c() => break Ok(()),
                e = sigterm.recv() => match e {
                    Some(_) => Ok(()),
                    None => Err(anyhow!("SIGTERM machine broke")),
                },
                e = sighup.recv() => match e {
                    Some(_) => {
                        if let Err(error) = read_config().await {
                            error!("Failed to reload configuration: {error}");
                        }
                        Ok(())
                    }
                    None => Err(anyhow!("SIGHUP machine broke")),
                },
                _ = sigquit.recv() => Err(anyhow!("Got SIGQUIT")),
            }
            .inspect_err(|e| error!("Encountered error running: {e}"));
            match res {
                Ok(()) => continue,
                r => break r,
            }
        };
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
