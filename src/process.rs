/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use std::ffi::OsStr;
use tokio::process::Command;
use tracing::warn;

pub async fn script_exit_code(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<bool> {
    // Run given script and return true on success
    let mut child = Command::new(executable).args(args).spawn()?;
    let status = child.wait().await?;
    Ok(status.success())
}

pub async fn run_script(name: &str, executable: &str, args: &[impl AsRef<OsStr>]) -> Result<bool> {
    // Run given script to get exit code and return true on success.
    // Return false on failure, but also print an error if needed
    script_exit_code(executable, args)
        .await
        .inspect_err(|message| warn!("Error running {name} {message}"))
}

pub async fn script_output(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<String> {
    // Run given command and return the output given
    let output = Command::new(executable).args(args).output();

    let output = output.await?;

    let s = std::str::from_utf8(&output.stdout)?;
    Ok(s.to_string())
}
