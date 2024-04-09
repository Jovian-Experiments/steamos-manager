/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, Result};
use nix::sys::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::ffi::OsStr;
use tokio::process::{Child, Command};
use tracing::error;
use zbus::interface;

use crate::{to_zbus_fdo_error};

const PROCESS_PREFIX: &str = "/com/steampowered/SteamOSManager1/Process";

pub struct ProcessManager {
    process: Child,
}

impl ProcessManager {
    pub async fn get_command_object_path(
        executable: &str,
        args: &[impl AsRef<OsStr>],
        connection: &mut zbus::Connection,
        next_process: &mut u32,
        operation_name: &str,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        // Run the given executable and give back an object path
        let path = format!("{}{}", PROCESS_PREFIX, next_process);
        *next_process += 1;
        let pm = ProcessManager::run_long_command(executable, args)
            .await
            .inspect_err(|message| error!("Error {operation_name}: {message}"))
            .map_err(to_zbus_fdo_error)?;
        connection.object_server().at(path.as_str(), pm).await?;
        zbus::zvariant::OwnedObjectPath::try_from(path).map_err(to_zbus_fdo_error)
    }

    fn send_signal(&self, signal: nix::sys::signal::Signal) -> Result<()> {
        // if !self.processes.contains_key(&id) {
        // println!("no process found with id {id}");
        // return Err(anyhow!("No process found with id {id}"));
        // }

        let command = &self.process;
        let pid: Result<i32, std::io::Error> = match command.id() {
            Some(id) => match id.try_into() {
                Ok(raw_pid) => Ok(raw_pid),
                Err(message) => {
                    bail!("Unable to get pid_t from command {message}");
                }
            },
            None => {
                bail!("Unable to get pid from command, it likely finished running");
            }
        };
        signal::kill(Pid::from_raw(pid.unwrap()), signal)?;
        Ok(())
    }

    async fn exit_code_internal(&mut self) -> Result<i32> {
        let status = self.process.wait().await?;
        match status.code() {
            Some(code) => Ok(code),
            None => bail!("Process exited without giving a code somehow."),
        }
    }

    pub async fn run_long_command(
        executable: &str,
        args: &[impl AsRef<OsStr>],
    ) -> Result<ProcessManager> {
        // Run the given executable with the given arguments
        // Return an id that can be used later to pause/cancel/resume as needed
        let child = Command::new(executable).args(args).spawn()?;
        Ok(ProcessManager { process: child })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.ProcessManager")]
impl ProcessManager {
    pub async fn pause(&self) -> zbus::fdo::Result<()> {
        // Pause the given process if possible
        // Return true on success, false otherwise
        self.send_signal(Signal::SIGSTOP).map_err(to_zbus_fdo_error)
    }

    pub async fn resume(&self) -> zbus::fdo::Result<()> {
        // Resume the given process if possible
        self.send_signal(Signal::SIGCONT).map_err(to_zbus_fdo_error)
    }

    pub async fn cancel(&self) -> zbus::fdo::Result<()> {
        self.send_signal(Signal::SIGTERM).map_err(to_zbus_fdo_error)
    }

    pub async fn kill(&self) -> zbus::fdo::Result<()> {
        self.send_signal(signal::SIGKILL).map_err(to_zbus_fdo_error)
    }

    pub async fn exit_code(&mut self) -> zbus::fdo::Result<i32> {
        self.exit_code_internal().await.map_err(to_zbus_fdo_error)
    }
}

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
    let cb = test.process_cb.get();
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
    let cb = test.process_cb.get();
    cb(executable, args.as_ref()).map(|(_, res)| res)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;

    fn ok(_: &str, _: &[&OsStr]) -> Result<(i32, String)> {
        Ok((0, String::from("ok")))
    }

    fn code(_: &str, _: &[&OsStr]) -> Result<(i32, String)> {
        Ok((1, String::from("code")))
    }

    fn exit(_: &str, _: &[&OsStr]) -> Result<(i32, String)> {
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
