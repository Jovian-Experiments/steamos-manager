/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use std::io::ErrorKind;
use tokio::fs::{create_dir_all, read_to_string, write};
use tracing::{error, info};

use crate::daemon::DaemonContext;

pub(in crate::daemon) async fn read_state<C: DaemonContext>(context: &C) -> Result<C::State> {
    let path = context.state_path()?;
    let state = match read_to_string(path).await {
        Ok(state) => state,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                info!("No state file found, reloading default state");
                return Ok(C::State::default());
            }
            error!("Error loading state: {e}");
            return Err(e.into());
        }
    };
    Ok(toml::from_str(state.as_str())?)
}

pub(in crate::daemon) async fn write_state<C: DaemonContext>(context: &C) -> Result<()> {
    let path = context.state_path()?;
    create_dir_all(path.parent().ok_or(anyhow!(
        "Context path {} has no parent dir",
        path.to_string_lossy()
    ))?)
    .await?;
    let state = toml::to_string_pretty(&context.state())?;
    Ok(write(path, state.as_bytes()).await?)
}

pub(in crate::daemon) async fn read_config<C: DaemonContext>(_context: &C) -> Result<C::Config> {
    todo!();
}
