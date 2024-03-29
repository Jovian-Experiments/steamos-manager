/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use std::ffi::OsStr;
use tokio::process::Command;
use tracing::warn;

pub const SYSTEMCTL_PATH: &str = "/usr/bin/systemctl";

pub async fn script_exit_code(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<i32> {
    // Run given script and return the exit code
    let mut child = Command::new(executable).args(args).spawn()?;
    let status = child.wait().await?;
    status.code().ok_or(anyhow!("Killed by signal"))
}

pub async fn run_script(name: &str, executable: &str, args: &[impl AsRef<OsStr>]) -> Result<()> {
    // Run given script to get exit code and return true on success.
    // Return Err on failure, but also print an error if needed
    match script_exit_code(executable, args).await {
        Ok(0) => Ok(()),
        Ok(code) => {
            warn!("Error running {name}: exited {code}");
            Err(anyhow!("Exited {code}"))
        }
        Err(message) => {
            warn!("Error running {name}: {message}");
            Err(message)
        }
    }
}

pub async fn script_output(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<String> {
    // Run given command and return the output given
    let output = Command::new(executable).args(args).output();

    let output = output.await?;

    let s = std::str::from_utf8(&output.stdout)?;
    Ok(s.to_string())
}
