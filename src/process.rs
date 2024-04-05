/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use std::ffi::OsStr;
use tokio::process::Command;

#[cfg(not(test))]
pub async fn script_exit_code(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<i32> {
    // Run given script and return the exit code
    let mut child = Command::new(executable).args(args).spawn()?;
    let status = child.wait().await?;
    status.code().ok_or(anyhow!("Killed by signal"))
}

#[cfg(test)]
pub async fn script_exit_code(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<i32> {
    let test = crate::testing::current();
    let args: Vec<&OsStr> = args.iter().map(|arg| arg.as_ref()).collect();
    let cb = &test.process_cb;
    cb(executable, args.as_ref()).map(|(res, _)| res)
}

pub async fn run_script(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<()> {
    // Run given script to get exit code and return true on success.
    // Return Err on failure, but also print an error if needed
    match script_exit_code(executable, args).await {
        Ok(0) => Ok(()),
        Ok(code) => Err(anyhow!("Exited {code}")),
        Err(message) => Err(message),
    }
}

#[cfg(not(test))]
pub async fn script_output(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<String> {
    // Run given command and return the output given
    let output = Command::new(executable).args(args).output();

    let output = output.await?;

    let s = std::str::from_utf8(&output.stdout)?;
    Ok(s.to_string())
}

#[cfg(test)]
pub async fn script_output(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<String> {
    let test = crate::testing::current();
    let args: Vec<&OsStr> = args.iter().map(|arg| arg.as_ref()).collect();
    let cb = &test.process_cb;
    cb(executable, args.as_ref()).map(|(_, res)| res)
}
