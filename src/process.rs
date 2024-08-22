/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use std::ffi::OsStr;
#[cfg(not(test))]
use std::process::Stdio;
#[cfg(not(test))]
use tokio::process::Command;

#[cfg(not(test))]
pub async fn script_exit_code(
    executable: impl AsRef<OsStr>,
    args: &[impl AsRef<OsStr>],
) -> Result<i32> {
    // Run given script and return the exit code
    let output = Command::new(executable)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await?;
    output.status.code().ok_or(anyhow!("Killed by signal"))
}

#[cfg(test)]
pub async fn script_exit_code(
    executable: impl AsRef<OsStr>,
    args: &[impl AsRef<OsStr>],
) -> Result<i32> {
    let test = crate::testing::current();
    let args: Vec<&OsStr> = args.iter().map(|arg| arg.as_ref()).collect();
    let cb = test.process_cb.get();
    cb(executable.as_ref(), args.as_ref()).map(|(res, _)| res)
}

pub async fn run_script(executable: impl AsRef<OsStr>, args: &[impl AsRef<OsStr>]) -> Result<()> {
    // Run given script to get exit code and return true on success.
    // Return Err on failure, but also print an error if needed
    match script_exit_code(executable, args).await {
        Ok(0) => Ok(()),
        Ok(code) => Err(anyhow!("Exited {code}")),
        Err(message) => Err(message),
    }
}

#[cfg(not(test))]
pub async fn script_output(
    executable: impl AsRef<OsStr>,
    args: &[impl AsRef<OsStr>],
) -> Result<String> {
    // Run given command and return the output given
    let output = Command::new(executable).args(args).output();

    let output = output.await?;

    let s = std::str::from_utf8(&output.stdout)?;
    Ok(s.to_string())
}

#[cfg(test)]
pub async fn script_output(
    executable: impl AsRef<OsStr>,
    args: &[impl AsRef<OsStr>],
) -> Result<String> {
    let test = crate::testing::current();
    let args: Vec<&OsStr> = args.iter().map(|arg| arg.as_ref()).collect();
    let cb = test.process_cb.get();
    cb(executable.as_ref(), args.as_ref()).map(|(_, res)| res)
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use crate::testing;

    pub fn ok(_: &OsStr, _: &[&OsStr]) -> Result<(i32, String)> {
        Ok((0, String::from("ok")))
    }

    pub fn code(_: &OsStr, _: &[&OsStr]) -> Result<(i32, String)> {
        Ok((1, String::from("code")))
    }

    pub fn exit(_: &OsStr, _: &[&OsStr]) -> Result<(i32, String)> {
        Err(anyhow!("oops!"))
    }

    #[tokio::test]
    async fn test_run_script() {
        let h = testing::start();

        h.test.process_cb.set(ok);
        assert!(run_script("", &[] as &[&OsStr]).await.is_ok());

        h.test.process_cb.set(code);
        assert_eq!(
            run_script("", &[] as &[&OsStr])
                .await
                .unwrap_err()
                .to_string(),
            "Exited 1"
        );

        h.test.process_cb.set(exit);
        assert_eq!(
            run_script("", &[] as &[&OsStr])
                .await
                .unwrap_err()
                .to_string(),
            "oops!"
        );
    }
}
