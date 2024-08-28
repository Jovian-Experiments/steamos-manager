/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, Result};
use libc::pid_t;
use nix::sys::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Cursor;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use tokio::process::{Child, Command};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tracing::error;
use zbus::fdo::{self, IntrospectableProxy};
use zbus::{interface, zvariant, Connection, Interface, InterfaceRef, SignalContext};
use zbus_xml::Node;

use crate::error::{to_zbus_fdo_error, zbus_to_zbus_fdo};
use crate::proxy::{JobManager1Proxy, Job1Proxy};
use crate::Service;

const JOB_PREFIX: &str = "/com/steampowered/SteamOSManager1/Jobs";

pub struct JobManager {
    // This object manages exported jobs. It spawns processes, numbers them, and
    // keeps a handle to the zbus connection to expose the name over the bus.
    connection: Connection,
    jm_iface: InterfaceRef<JobManagerInterface>,
    mirrored_jobs: HashMap<String, zvariant::OwnedObjectPath>,
    next_job: u32,
}

struct Job {
    process: Child,
    paused: bool,
    exit_code: Option<i32>,
}

struct JobManagerInterface {}

pub struct JobManagerService {
    job_manager: JobManager,
    channel: UnboundedReceiver<JobManagerCommand>,
    connection: Connection,
}

struct MirroredJob {
    job: Job1Proxy<'static>,
}

pub enum JobManagerCommand {
    MirrorConnection(Connection),
    MirrorJob {
        connection: Connection,
        path: zvariant::OwnedObjectPath,
        reply: oneshot::Sender<fdo::Result<zvariant::OwnedObjectPath>>,
    },
    #[allow(unused)]
    RunProcess {
        executable: String,
        args: Vec<OsString>,
        operation_name: String,
        reply: oneshot::Sender<fdo::Result<zvariant::OwnedObjectPath>>,
    },
}

impl JobManager {
    pub async fn new(connection: Connection) -> Result<JobManager> {
        let jm_iface = JobManagerInterface {};
        let jm_iface: InterfaceRef<JobManagerInterface> = {
            // This object needs to be dropped to appease the borrow checker
            let object_server = connection.object_server();
            object_server.at(JOB_PREFIX, jm_iface).await?;

            object_server.interface(JOB_PREFIX).await?
        };
        Ok(JobManager {
            connection,
            jm_iface,
            mirrored_jobs: HashMap::new(),
            next_job: 0,
        })
    }

    async fn add_job<J: Interface>(&mut self, job: J) -> fdo::Result<zvariant::OwnedObjectPath> {
        let path = format!("{}/{}", JOB_PREFIX, self.next_job);
        self.next_job += 1;
        self.connection
            .object_server()
            .at(path.as_str(), job)
            .await?;

        let object_path = zvariant::OwnedObjectPath::try_from(path).map_err(to_zbus_fdo_error)?;
        JobManagerInterface::job_started(self.jm_iface.signal_context(), object_path.as_ref())
            .await?;
        Ok(object_path)
    }

    pub async fn run_process(
        &mut self,
        executable: impl AsRef<OsStr>,
        args: &[impl AsRef<OsStr>],
        operation_name: &str,
    ) -> fdo::Result<zvariant::OwnedObjectPath> {
        // Run the given executable and give back an object path
        let job = Job::spawn(executable, args)
            .await
            .inspect_err(|message| error!("Error {operation_name}: {message}"))
            .map_err(to_zbus_fdo_error)?;

        self.add_job(job).await
    }

    pub async fn mirror_job<'a, P>(
        &mut self,
        connection: &Connection,
        path: P,
    ) -> fdo::Result<zvariant::OwnedObjectPath>
    where
        P: TryInto<zvariant::ObjectPath<'a>>,
        P::Error: Into<zbus::Error>,
    {
        let path = path.try_into().map_err(Into::into)?.into_owned();
        let name = format!("{}:{}", connection.server_guid(), path.as_str());
        if let Some(object_path) = self.mirrored_jobs.get(&name) {
            return Ok(object_path.clone());
        }

        let proxy = Job1Proxy::builder(connection)
            .destination("com.steampowered.SteamOSManager1")?
            .path(path)?
            .build()
            .await?;
        let job = MirroredJob { job: proxy };

        let object_path = self.add_job(job).await?;
        self.mirrored_jobs.insert(name, object_path.to_owned());
        Ok(object_path)
    }

    pub async fn mirror_connection(&mut self, connection: &Connection) -> fdo::Result<()> {
        let proxy = IntrospectableProxy::builder(connection)
            .destination("com.steampowered.SteamOSManager1")?
            .path(JOB_PREFIX)?
            .build()
            .await?;
        let introspection = proxy.introspect().await?;
        let introspection =
            Node::from_reader(Cursor::new(introspection)).map_err(to_zbus_fdo_error)?;
        for node in introspection.nodes() {
            if let Some(name) = node.name() {
                self.mirror_job(connection, format!("{JOB_PREFIX}/{name}"))
                    .await?;
            }
        }
        Ok(())
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.JobManager1")]
impl JobManagerInterface {
    #[zbus(signal)]
    async fn job_started(
        signal_ctxt: &SignalContext<'_>,
        job: zvariant::ObjectPath<'_>,
    ) -> zbus::Result<()>;
}

impl Job {
    async fn spawn(executable: impl AsRef<OsStr>, args: &[impl AsRef<OsStr>]) -> Result<Job> {
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

#[interface(name = "com.steampowered.SteamOSManager1.Job1")]
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

#[interface(name = "com.steampowered.SteamOSManager1.Job1")]
impl MirroredJob {
    pub async fn pause(&mut self) -> fdo::Result<()> {
        self.job.pause().await.map_err(zbus_to_zbus_fdo)
    }

    pub async fn resume(&mut self) -> fdo::Result<()> {
        self.job.resume().await.map_err(zbus_to_zbus_fdo)
    }

    pub async fn cancel(&mut self, force: bool) -> fdo::Result<()> {
        self.job.cancel(force).await.map_err(zbus_to_zbus_fdo)
    }

    pub async fn wait(&mut self) -> fdo::Result<i32> {
        self.job.wait().await.map_err(zbus_to_zbus_fdo)
    }
}

impl JobManagerService {
    pub fn new(
        job_manager: JobManager,
        channel: UnboundedReceiver<JobManagerCommand>,
        connection: Connection,
    ) -> JobManagerService {
        JobManagerService {
            job_manager,
            channel,
            connection,
        }
    }

    async fn handle_command(&mut self, command: JobManagerCommand) -> Result<()> {
        match command {
            JobManagerCommand::MirrorConnection(connection) => {
                self.job_manager.mirror_connection(&connection).await?
            }
            JobManagerCommand::MirrorJob {
                connection,
                path,
                reply,
            } => {
                let path = self.job_manager.mirror_job(&connection, path).await;
                reply
                    .send(path)
                    .map_err(|e| anyhow!("Failed to send reply {e:?}"))?;
            }
            JobManagerCommand::RunProcess {
                executable,
                args,
                operation_name,
                reply,
            } => {
                let path = self
                    .job_manager
                    .run_process(&executable, &args, &operation_name)
                    .await;
                reply
                    .send(path)
                    .map_err(|e| anyhow!("Failed to send reply {e:?}"))?;
            }
        }
        Ok(())
    }
}

impl Service for JobManagerService {
    const NAME: &'static str = "job-manager";

    async fn run(&mut self) -> Result<()> {
        let jm = JobManager1Proxy::new(&self.connection).await?;
        let mut stream = jm.receive_job_started().await?;

        loop {
            tokio::select! {
                Some(job) = stream.next() => {
                    let path = job.args()?.job;
                    self.job_manager
                        .mirror_job(&self.connection, path)
                        .await?;
                },
                message = self.channel.recv() => {
                    let message = match message {
                        None => bail!("Job manager service channel broke"),
                        Some(message) => message,
                    };
                    self.handle_command(message).await.inspect_err(|e| error!("Failed to handle command: {e}"))?;
                },
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use crate::testing;

    use anyhow::anyhow;
    use nix::sys::signal::Signal;
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};
    use tokio::task::JoinHandle;
    use tokio::time::sleep;
    use zbus::names::BusName;
    use zbus::ConnectionBuilder;

    #[tokio::test]
    async fn test_job_emitted() {
        let _h = testing::start();

        let connection = ConnectionBuilder::session()
            .expect("session")
            .build()
            .await
            .expect("connection");
        let sender = connection.unique_name().unwrap().to_owned();
        let mut pm = JobManager::new(connection).await.expect("pm");

        let (tx, rx) = oneshot::channel::<()>();

        let job = tokio::spawn(async move {
            let connection = ConnectionBuilder::session()?.build().await?;
            let jm = JobManager1Proxy::builder(&connection)
                .destination(sender)?
                .build()
                .await?;
            let mut spawned = jm.receive_job_started().await?;
            let next = spawned.next();
            let _ = tx.send(());

            next.await.ok_or(anyhow!("nothing"))
        });

        rx.await.expect("rx");

        let object = pm
            .run_process("/usr/bin/true", &[] as &[&OsStr], "")
            .await
            .expect("path");
        assert_eq!(object.as_ref(), "/com/steampowered/SteamOSManager1/Jobs/0");

        let job = job.await.expect("job");
        let job = job.expect("job2");
        let path = job.args().expect("args").job;

        assert_eq!(object.as_ref(), path);
    }

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

    struct MockJob {}

    #[zbus::interface(name = "com.steampowered.SteamOSManager1.Job1")]
    impl MockJob {
        pub async fn pause(&mut self) -> fdo::Result<()> {
            Err(fdo::Error::Failed(String::from("pause")))
        }

        pub async fn resume(&mut self) -> fdo::Result<()> {
            Err(fdo::Error::Failed(String::from("resume")))
        }

        pub async fn cancel(&mut self, _force: bool) -> fdo::Result<()> {
            Err(fdo::Error::Failed(String::from("cancel")))
        }

        pub async fn wait(&mut self) -> fdo::Result<i32> {
            Ok(-1)
        }
    }

    #[tokio::test]
    async fn test_job_mirror_relay() {
        let mut handle = testing::start();

        let connection = handle.new_dbus().await.expect("connection");
        let address = handle.dbus_address().await.unwrap();
        connection
            .request_name("com.steampowered.SteamOSManager1")
            .await
            .expect("reserve");

        connection
            .object_server()
            .at(format!("{JOB_PREFIX}/0"), MockJob {})
            .await
            .expect("at");

        let (tx, mut rx) = mpsc::channel(3);
        let (fin_tx, fin_rx) = oneshot::channel();

        let job: JoinHandle<Result<()>> = tokio::spawn(async move {
            let connection = ConnectionBuilder::address(address)
                .expect("address")
                .build()
                .await
                .expect("build");
            let mut jm = JobManager::new(connection.clone()).await.expect("jm");

            sleep(Duration::from_millis(10)).await;

            let path = jm
                .mirror_job(&connection, format!("{JOB_PREFIX}/0"))
                .await
                .expect("mirror_job");
            let name = connection.unique_name().unwrap().clone();
            let proxy = Job1Proxy::builder(&connection)
                .destination(BusName::Unique(name.into()))
                .expect("destination")
                .path(path)
                .expect("path")
                .build()
                .await
                .expect("build");

            match proxy.pause().await.unwrap_err() {
                zbus::Error::MethodError(_, Some(text), _) => tx.send(text).await?,
                _ => bail!("pause"),
            };
            match proxy.resume().await.unwrap_err() {
                zbus::Error::MethodError(_, Some(text), _) => tx.send(text).await?,
                _ => bail!("resume"),
            };
            match proxy.cancel(false).await.unwrap_err() {
                zbus::Error::MethodError(_, Some(text), _) => tx.send(text).await?,
                _ => bail!("cancel"),
            };

            Ok(fin_rx.await?)
        });

        assert_eq!(
            rx.recv().await.expect("rx"),
            "org.freedesktop.DBus.Error.Failed: pause"
        );
        assert_eq!(
            rx.recv().await.expect("rx"),
            "org.freedesktop.DBus.Error.Failed: resume"
        );
        assert_eq!(
            rx.recv().await.expect("rx"),
            "org.freedesktop.DBus.Error.Failed: cancel"
        );

        fin_tx.send(()).expect("fin");
        job.await.expect("job").expect("job2");
    }
}
