/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use zbus::connection::Connection;

use crate::daemon::config::{read_config, read_state, write_state};
use crate::sls::{LogLayer, LogReceiver};
use crate::Service;

mod config;
pub(crate) mod root;
pub(crate) mod user;

pub use root::daemon as root;
pub use user::daemon as user;

pub(crate) trait DaemonContext: Sized {
    type State: for<'a> Deserialize<'a> + Serialize + Default + Debug;
    type Config: for<'a> Deserialize<'a> + Default + Debug;
    type Command: Send;

    fn state_path(&self) -> Result<PathBuf> {
        let config_path = self.user_config_path()?;
        Ok(config_path.join("state.toml"))
    }

    fn user_config_path(&self) -> Result<PathBuf>;
    fn system_config_path(&self) -> Result<PathBuf>;
    fn state(&self) -> Self::State;

    async fn start(
        &mut self,
        state: Self::State,
        config: Self::Config,
        daemon: &mut Daemon<Self>,
    ) -> Result<()>;

    async fn reload(&mut self, config: Self::Config, daemon: &mut Daemon<Self>) -> Result<()>;

    async fn handle_command(&mut self, cmd: Self::Command, daemon: &mut Daemon<Self>)
        -> Result<()>;
}

pub(crate) struct Daemon<C: DaemonContext> {
    services: JoinSet<Result<()>>,
    token: CancellationToken,
    channel: Receiver<DaemonCommand<C::Command>>,
}

#[derive(Debug)]
pub(crate) enum DaemonCommand<T> {
    ContextCommand(T),
    ReadConfig,
    WriteState,
}

impl<C: DaemonContext> Daemon<C> {
    pub(crate) async fn new<S: SubscriberExt + Send + Sync + for<'a> LookupSpan<'a>>(
        subscriber: S,
        connection: Connection,
        channel: Receiver<DaemonCommand<C::Command>>,
    ) -> Result<Daemon<C>> {
        let services = JoinSet::new();
        let token = CancellationToken::new();

        let log_receiver = LogReceiver::new(connection.clone()).await?;
        let remote_logger = LogLayer::new(&log_receiver).await;
        let subscriber = subscriber.with(remote_logger);
        tracing::subscriber::set_global_default(subscriber)?;

        let mut daemon = Daemon {
            services,
            token,
            channel,
        };
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

    pub(crate) async fn run(&mut self, mut context: C) -> Result<()> {
        ensure!(
            !self.services.is_empty(),
            "Can't run a daemon with no services attached."
        );

        let state = read_state(&context).await?;
        let config = read_config(&context).await?;
        debug!("Starting daemon with state: {state:#?}, config: {config:#?}");
        context.start(state, config, self).await?;

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
                        match read_config(&context).await {
                            Ok(config) =>
                                context.reload(config, self).await,
                            Err(error) => {
                                error!("Failed to load configuration: {error}");
                                Ok(())
                            }
                        }
                    }
                    None => Err(anyhow!("SIGHUP machine broke")),
                },
                msg = self.channel.recv() => match msg {
                    Some(msg) => {
                        self.handle_message(msg, &mut context).await
                    }
                    None => Err(anyhow!("All senders have been closed")),
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

    async fn handle_message(
        &mut self,
        cmd: DaemonCommand<C::Command>,
        context: &mut C,
    ) -> Result<()> {
        match cmd {
            DaemonCommand::ContextCommand(cmd) => context.handle_command(cmd, self).await,
            DaemonCommand::ReadConfig => match read_config(context).await {
                Ok(config) => context.reload(config, self).await,
                Err(error) => {
                    error!("Failed to load configuration: {error}");
                    Ok(())
                }
            },
            DaemonCommand::WriteState => write_state(context).await,
        }
    }
}

// Rust doesn't support a good way to simplify this type yet
// See <https://github.com/rust-lang/rust/issues/8995>
#[allow(clippy::type_complexity)]
pub(crate) fn channel<C: DaemonContext>() -> (
    Sender<DaemonCommand<C::Command>>,
    Receiver<DaemonCommand<C::Command>>,
) {
    mpsc::channel(10)
}
