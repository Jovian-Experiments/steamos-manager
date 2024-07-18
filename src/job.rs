/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, Result};
use libc::pid_t;
use nix::sys::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::ffi::OsStr;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use tokio::process::{Child, Command};
use tracing::error;
use zbus::{fdo, interface, zvariant, Connection};

use crate::error::to_zbus_fdo_error;

const JOB_PREFIX: &str = "/com/steampowered/SteamOSManager1/Job";

pub struct JobManager {
    // This object manages exported jobs. It spawns processes, numbers them, and
    // keeps a handle to the zbus connection to expose the name over the bus.
    connection: Connection,
    next_job: u32,
}

struct Job {
    process: Child,
    paused: bool,
    exit_code: Option<i32>,
}

impl JobManager {
    pub fn new(conn: Connection) -> JobManager {
        JobManager {
            connection: conn,
            next_job: 0,
        }
    }

    pub async fn run_process(
        &mut self,
        executable: &str,
        args: &[impl AsRef<OsStr>],
        operation_name: &str,
    ) -> fdo::Result<zvariant::OwnedObjectPath> {
        // Run the given executable and give back an object path
        let path = format!("{}{}", JOB_PREFIX, self.next_job);
        self.next_job += 1;
        let job = Job::spawn(executable, args)
            .await
            .inspect_err(|message| error!("Error {operation_name}: {message}"))
            .map_err(to_zbus_fdo_error)?;
        self.connection
            .object_server()
            .at(path.as_str(), job)
            .await?;
        zvariant::OwnedObjectPath::try_from(path).map_err(to_zbus_fdo_error)
    }
}

impl Job {
    async fn spawn(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<Job> {
        let child = Command::new(executable).args(args).spawn()?;
        Ok(Job {
            process: child,
            paused: false,
            exit_code: None,
        })
    }

    fn send_signal(&self, signal: nix::sys::signal::Signal) -> Result<()> {
        let pid = match self.process.id() {
            Some(id) => id,
            None => bail!("Unable to get pid from command, it likely finished running"),
        };
        let pid: pid_t = match pid.try_into() {
            Ok(pid) => pid,
            Err(message) => bail!("Unable to get pid_t from command {message}"),
        };
        signal::kill(Pid::from_raw(pid), signal)?;
        Ok(())
    }

    fn update_exit_code(&mut self, status: ExitStatus) -> Result<i32> {
        if let Some(code) = status.code() {
            self.exit_code = Some(code);
            Ok(code)
        } else if let Some(signal) = status.signal() {
            self.exit_code = Some(-signal);
            Ok(-signal)
        } else {
            bail!("Process exited without return code or signal");
        }
    }

    fn try_wait(&mut self) -> Result<Option<i32>> {
        if self.exit_code.is_none() {
            // If we don't already have an exit code, try to wait for the process
            if let Some(status) = self.process.try_wait()? {
                self.update_exit_code(status)?;
            }
        }
        Ok(self.exit_code)
    }

    async fn wait_internal(&mut self) -> Result<i32> {
        if let Some(code) = self.exit_code {
            // Just give the exit_code if we have it already
            Ok(code)
        } else {
            // Otherwise wait for the process
            let status = self.process.wait().await?;
            self.update_exit_code(status)
        }
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.Job")]
impl Job {
    pub async fn pause(&mut self) -> fdo::Result<()> {
        if self.paused {
            return Err(fdo::Error::Failed("Already paused".to_string()));
        }
        // Pause the given process if possible
        // Return true on success, false otherwise
        let result = self.send_signal(Signal::SIGSTOP).map_err(to_zbus_fdo_error);
        self.paused = true;
        result
    }

    pub async fn resume(&mut self) -> fdo::Result<()> {
        // Resume the given process if possible
        if !self.paused {
            return Err(fdo::Error::Failed("Not paused".to_string()));
        }
        let result = self.send_signal(Signal::SIGCONT).map_err(to_zbus_fdo_error);
        self.paused = false;
        result
    }

    pub async fn cancel(&mut self, force: bool) -> fdo::Result<()> {
        if self.try_wait().map_err(to_zbus_fdo_error)?.is_none() {
            self.send_signal(match force {
                true => Signal::SIGKILL,
                false => Signal::SIGTERM,
            })
            .map_err(to_zbus_fdo_error)?;
            if self.paused {
                self.resume().await?;
            }
        }
        Ok(())
    }

    pub async fn wait(&mut self) -> fdo::Result<i32> {
        if self.paused {
            self.resume().await?;
        }

        let code = match self.wait_internal().await.map_err(to_zbus_fdo_error) {
            Ok(v) => v,
            Err(_) => {
                return Err(fdo::Error::Failed("Unable to get exit code".to_string()));
            }
        };
        self.exit_code = Some(code);
        Ok(code)
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use crate::testing;
    use nix::sys::signal::Signal;

    #[tokio::test]
    async fn test_job_manager() {
        let _h = testing::start();

        let mut false_process = Job::spawn("/bin/false", &[] as &[String; 0]).await.unwrap();
        let mut true_process = Job::spawn("/bin/true", &[] as &[String; 0]).await.unwrap();

        let mut pause_process = Job::spawn("/usr/bin/sleep", &["0.2"]).await.unwrap();
        pause_process.pause().await.expect("pause");

        assert_eq!(
            pause_process.pause().await.unwrap_err(),
            fdo::Error::Failed("Already paused".to_string())
        );

        pause_process.resume().await.expect("resume");

        assert_eq!(
            pause_process.resume().await.unwrap_err(),
            fdo::Error::Failed("Not paused".to_string())
        );

        // Sleep gives 0 exit code when done, -1 when we haven't waited for it yet
        assert_eq!(pause_process.wait().await.unwrap(), 0);

        assert_eq!(false_process.wait().await.unwrap(), 1);
        assert_eq!(true_process.wait().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_multikill() {
        let _h = testing::start();

        let mut sleep_process = Job::spawn("/usr/bin/sleep", &["0.1"]).await.unwrap();
        sleep_process.cancel(true).await.expect("kill");

        // Killing a process should be idempotent
        sleep_process.cancel(true).await.expect("kill");

        assert_eq!(
            sleep_process.wait().await.unwrap(),
            -(Signal::SIGKILL as i32)
        );
    }

    #[tokio::test]
    async fn test_terminate_unpause() {
        let _h = testing::start();

        let mut pause_process = Job::spawn("/usr/bin/sleep", &["0.2"]).await.unwrap();
        pause_process.pause().await.expect("pause");
        assert_eq!(pause_process.try_wait().expect("try_wait"), None);

        // Canceling a process should unpause it
        pause_process.cancel(false).await.expect("pause");
        assert_eq!(
            pause_process.wait().await.unwrap(),
            -(Signal::SIGTERM as i32)
        );
    }
}
