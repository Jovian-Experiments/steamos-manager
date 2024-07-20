use anyhow::{anyhow, bail, Result};
use libc::pid_t;
use nix::sys::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;
use tempfile::{tempdir, TempDir};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use zbus::connection::{Builder, Connection};

thread_local! {
    static TEST: RefCell<Option<Rc<Test>>> = RefCell::new(None);
}

#[macro_export]
macro_rules! enum_roundtrip {
    ($enum:ident => $value:literal : str = $variant:ident) => {
        assert_eq!($enum::$variant.to_string(), $value);
        assert_eq!($enum::from_str($value).unwrap(), $enum::$variant);
    };
    ($enum:ident => $value:literal : $ty:ty = $variant:ident) => {
        assert_eq!($enum::$variant as $ty, $value);
        assert_eq!($enum::try_from($value), Ok($enum::$variant));
    };

    ($enum:ident { $($value:literal : $ty:ident = $variant:ident,)+ }) => {
        $(enum_roundtrip!($enum => $value : $ty = $variant);)+
    };
}

#[macro_export]
macro_rules! enum_on_off {
    ($enum:ident => ($on:ident, $off:ident)) => {
        assert_eq!($enum::from_str("on").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("On").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("enable").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("enabled").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("1").unwrap(), $enum::$on);
        assert_eq!($enum::from_str("off").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("Off").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("disable").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("disabled").unwrap(), $enum::$off);
        assert_eq!($enum::from_str("0").unwrap(), $enum::$off);
    };
}

pub fn start() -> TestHandle {
    TEST.with(|lock| {
        assert!(lock.borrow().as_ref().is_none());
        let test: Rc<Test> = Rc::new(Test {
            base: tempdir().expect("Couldn't create test directory"),
            process_cb: Cell::new(|_, _| Err(anyhow!("No current process_cb"))),
            mock_dbus: Cell::new(None),
        });
        *lock.borrow_mut() = Some(test.clone());
        TestHandle { test }
    })
}

pub fn stop() {
    TEST.with(|lock| {
        let test = (*lock.borrow_mut()).take();
        if let Some(test) = test {
            if let Some(mock_dbus) = test.mock_dbus.take() {
                let _ = mock_dbus.shutdown();
            }
        }
    });
}

pub fn current() -> Rc<Test> {
    TEST.with(|lock| lock.borrow().as_ref().unwrap().clone())
}

pub struct MockDBus {
    pub connection: Connection,
    process: Child,
}

pub struct Test {
    base: TempDir,
    pub process_cb: Cell<fn(&str, &[&OsStr]) -> Result<(i32, String)>>,
    pub mock_dbus: Cell<Option<MockDBus>>,
}

pub struct TestHandle {
    pub test: Rc<Test>,
}

impl MockDBus {
    pub async fn new() -> Result<MockDBus> {
        let mut process = Command::new("/usr/bin/dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()?;

        let stdout = BufReader::new(
            process
                .stdout
                .take()
                .ok_or(anyhow!("Couldn't capture stdout"))?,
        );

        let address = stdout
            .lines()
            .next_line()
            .await?
            .ok_or(anyhow!("Failed to read address"))?;

        let connection = Builder::address(address.trim_end())?.build().await?;

        Ok(MockDBus {
            connection,
            process,
        })
    }

    pub fn shutdown(mut self) -> Result<()> {
        let pid = match self.process.id() {
            Some(id) => id,
            None => return Ok(()),
        };
        let pid: pid_t = match pid.try_into() {
            Ok(pid) => pid,
            Err(message) => bail!("Unable to get pid_t from command {message}"),
        };
        signal::kill(Pid::from_raw(pid), Signal::SIGINT)?;
        for _ in [0..10] {
            // Wait for the process to exit synchronously, but not for too long
            if self.process.try_wait()?.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_micros(100));
        }
        Ok(())
    }
}

impl Test {
    pub fn path(&self) -> &Path {
        self.base.path()
    }
}

impl TestHandle {
    pub async fn new_dbus(&mut self) -> Result<Connection> {
        let dbus = MockDBus::new().await?;
        let connection = dbus.connection.clone();
        self.test.mock_dbus.set(Some(dbus));
        Ok(connection)
    }
}

impl Drop for TestHandle {
    fn drop(&mut self) {
        stop();
    }
}
