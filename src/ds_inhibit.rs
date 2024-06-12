/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use inotify::{Event, EventMask, EventStream, Inotify, WatchDescriptor, WatchMask};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::{self, read_dir, read_link};
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{debug, error, info, warn};

use crate::{path, write_synced, Service};

struct HidNode {
    id: u32,
}

pub struct Inhibitor {
    inotify: EventStream<[u8; 512]>,
    dev_watch: WatchDescriptor,
    watches: HashMap<WatchDescriptor, HidNode>,
}

impl HidNode {
    fn new(id: u32) -> HidNode {
        HidNode { id }
    }

    fn sys_base(&self) -> PathBuf {
        path(format!("/sys/class/hidraw/hidraw{}/device", self.id))
    }

    fn hidraw(&self) -> PathBuf {
        path(format!("/dev/hidraw{}", self.id))
    }

    async fn get_nodes(&self) -> Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        let mut dir = read_dir(self.sys_base().join("input")).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            let mut dir = read_dir(&path).await?;
            while let Some(entry) = dir.next_entry().await? {
                if entry
                    .path()
                    .file_name()
                    .map(|e| e.to_string_lossy())
                    .is_some_and(|e| e.starts_with("mouse"))
                {
                    debug!("Found {}", path.display());
                    entries.push(path.join("inhibited"));
                }
            }
        }
        Ok(entries)
    }

    async fn can_inhibit(&self) -> bool {
        debug!("Checking if hidraw{} can be inhibited", self.id);
        let driver = match read_link(self.sys_base().join("driver")).await {
            Ok(driver) => driver,
            Err(e) => {
                warn!(
                    "Failed to find associated driver for hidraw{}: {}",
                    self.id, e
                );
                return false;
            }
        };

        if !matches!(
            driver.file_name().and_then(|d| d.to_str()),
            Some("sony") | Some("playstation")
        ) {
            debug!("Not a PlayStation controller");
            return false;
        }
        let nodes = match self.get_nodes().await {
            Ok(nodes) => nodes,
            Err(e) => {
                warn!("Failed to list inputs for hidraw{}: {e}", self.id);
                return false;
            }
        };
        if nodes.is_empty() {
            debug!("No nodes to inhibit");
            return false;
        }
        true
    }

    async fn check(&self) -> Result<()> {
        let hidraw = self.hidraw();
        let mut dir = read_dir(path("/proc")).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            let proc = match path.file_name().map(|p| p.to_str()) {
                Some(Some(p)) => p,
                _ => continue,
            };
            let _: u32 = match proc.parse() {
                Ok(i) => i,
                _ => continue,
            };
            let mut fds = match read_dir(path.join("fd")).await {
                Ok(fds) => fds,
                Err(e) => {
                    debug!("Process {proc} disappeared while scanning: {e}");
                    continue;
                }
            };
            while let Ok(Some(f)) = fds.next_entry().await {
                let path = match read_link(f.path()).await {
                    Ok(p) => p,
                    Err(e) => {
                        debug!("Process {proc} disappeared while scanning: {e}");
                        continue;
                    }
                };
                if path == hidraw {
                    let comm = match fs::read(crate::path(format!("/proc/{proc}/comm"))).await {
                        Ok(c) => c,
                        Err(e) => {
                            debug!("Process {proc} disappeared while scanning: {e}");
                            continue;
                        }
                    };
                    if String::from_utf8_lossy(comm.as_ref()) == "steam\n" {
                        info!("Inhibiting hidraw{}", self.id);
                        self.inhibit().await?;
                        return Ok(());
                    }
                }
            }
        }
        info!("Uninhibiting hidraw{}", self.id);
        self.uninhibit().await?;
        Ok(())
    }

    async fn inhibit(&self) -> Result<()> {
        let mut res = Ok(());
        for node in self.get_nodes().await?.into_iter() {
            if let Err(err) = write_synced(node, b"1\n").await {
                error!("Encountered error inhibiting: {err}");
                res = Err(err);
            }
        }
        res
    }

    async fn uninhibit(&self) -> Result<()> {
        let mut res = Ok(());
        for node in self.get_nodes().await?.into_iter() {
            if let Err(err) = write_synced(node, b"0\n").await {
                error!("Encountered error inhibiting: {err}");
                res = Err(err);
            }
        }
        res
    }
}

impl Inhibitor {
    pub fn new() -> Result<Inhibitor> {
        let inotify = Inotify::init()?.into_event_stream([0; 512])?;
        let dev_watch = inotify.watches().add(path("/dev"), WatchMask::CREATE)?;

        Ok(Inhibitor {
            inotify,
            dev_watch,
            watches: HashMap::new(),
        })
    }

    pub async fn init() -> Result<Inhibitor> {
        let mut inhibitor = match Inhibitor::new() {
            Ok(i) => i,
            Err(e) => {
                error!("Could not create inotify watches: {e}");
                return Err(e);
            }
        };

        let mut dir = read_dir(path("/dev")).await?;
        while let Some(entry) = dir.next_entry().await? {
            if let Err(e) = inhibitor.watch(entry.path().as_path()).await {
                error!("Encountered error attempting to watch: {e}");
            }
        }
        Ok(inhibitor)
    }

    async fn watch(&mut self, path: &Path) -> Result<bool> {
        let metadata = path.metadata()?;
        if metadata.is_dir() {
            return Ok(false);
        }

        let id = match path
            .file_name()
            .and_then(|f| f.to_str())
            .and_then(|s| s.strip_prefix("hidraw"))
            .and_then(|s| s.parse().ok())
        {
            Some(id) => id,
            None => return Ok(false),
        };

        let node = HidNode::new(id);
        if !node.can_inhibit().await {
            return Ok(false);
        }
        info!("Adding {} to watchlist", path.display());
        let watch = self.inotify.watches().add(
            &node.hidraw(),
            WatchMask::DELETE_SELF
                | WatchMask::OPEN
                | WatchMask::CLOSE_NOWRITE
                | WatchMask::CLOSE_WRITE,
        )?;
        if let Err(e) = node.check().await {
            error!(
                "Encountered error attempting to check if hidraw{} can be inhibited: {e}",
                node.id
            );
        }
        self.watches.insert(watch, node);
        Ok(true)
    }

    async fn process_event(&mut self, event: Event<OsString>) -> Result<()> {
        const QSEC: Duration = Duration::from_millis(250);
        debug!("Got event: {:08x}", event.mask);
        if event.wd == self.dev_watch {
            let path = match event.name {
                Some(fname) => PathBuf::from(fname),
                None => {
                    error!("Got an event without an associated filename!");
                    return Err(anyhow!("Got an event without an associated filename"));
                }
            };
            debug!("New device {} found", path.display());
            let path = crate::path("/dev").join(path);
            sleep(QSEC).await; // Wait a quarter second for nodes to enumerate
            if let Err(e) = self.watch(path.as_path()).await {
                error!("Encountered error attempting to watch: {e}");
                return Err(e);
            }
        } else if event.mask == EventMask::DELETE_SELF {
            debug!("Device removed");
            self.watches.remove(&event.wd);
            let _ = self.inotify.watches().remove(event.wd);
        } else if let Some(node) = self.watches.get(&event.wd) {
            node.check().await?;
        } else if event.mask != EventMask::IGNORED {
            error!("Unhandled event: {:08x}", event.mask);
        }
        Ok(())
    }
}

impl Service for Inhibitor {
    const NAME: &'static str = "ds-inhibitor";

    async fn run(&mut self) -> Result<()> {
        loop {
            let res = match self.inotify.next().await {
                Some(Ok(event)) => self.process_event(event).await,
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            };
            if let Err(e) = res {
                warn!("Got error processing event: {e}");
            }
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        let mut res = Ok(());
        for (wd, node) in self.watches.drain() {
            if let Err(e) = self.inotify.watches().remove(wd) {
                warn!("Error removing watch while shutting down: {e}");
                res = Err(e.into());
            }
            if let Err(e) = node.uninhibit().await {
                warn!("Error uninhibiting {} while shutting down: {e}", node.id);
                res = Err(e);
            }
        }
        res
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use std::fs::{create_dir_all, read_to_string, remove_file, write, File};
    use std::os::unix::fs::symlink;

    async fn nyield(time: u64) {
        sleep(Duration::from_millis(time)).await;
    }

    #[tokio::test]
    async fn hid_nodes() {
        let _h = testing::start();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(sys_base.join("input/input0/foo0")).expect("foo0");
        create_dir_all(sys_base.join("input/input1/bar0")).expect("bar0");
        create_dir_all(sys_base.join("input/input2/mouse0")).expect("mouse0");

        assert_eq!(
            hid.get_nodes().await.expect("get_nodes"),
            &[sys_base.join("input/input2/inhibited")]
        );
    }

    #[tokio::test]
    async fn hid_can_inhibit() {
        let _h = testing::start();

        let hids = [
            HidNode::new(0),
            HidNode::new(1),
            HidNode::new(2),
            HidNode::new(3),
            HidNode::new(4),
            HidNode::new(5),
            HidNode::new(6),
        ];

        create_dir_all(hids[0].sys_base().join("input/input0/foo0")).expect("foo0");
        symlink("foo", hids[0].sys_base().join("driver")).expect("hidraw0");
        create_dir_all(hids[1].sys_base().join("input/input1/mouse0")).expect("mouse0");
        symlink("foo", hids[1].sys_base().join("driver")).expect("hidraw1");
        create_dir_all(hids[2].sys_base().join("input/input2/foo1")).expect("foo1");
        symlink("sony", hids[2].sys_base().join("driver")).expect("hidraw2");
        create_dir_all(hids[3].sys_base().join("input/input3/mouse1")).expect("mouse1");
        symlink("sony", hids[3].sys_base().join("driver")).expect("hidraw3");
        create_dir_all(hids[4].sys_base().join("input/input4/foo2")).expect("foo2");
        symlink("playstation", hids[4].sys_base().join("driver")).expect("hidraw4");
        create_dir_all(hids[5].sys_base().join("input/input5/mouse2")).expect("mouse2");
        symlink("playstation", hids[5].sys_base().join("driver")).expect("hidraw5");
        create_dir_all(hids[6].sys_base().join("input/input6/mouse3")).expect("mouse3");

        assert!(!hids[0].can_inhibit().await);
        assert!(!hids[1].can_inhibit().await);
        assert!(!hids[2].can_inhibit().await);
        assert!(hids[3].can_inhibit().await);
        assert!(!hids[4].can_inhibit().await);
        assert!(hids[5].can_inhibit().await);
        assert!(!hids[6].can_inhibit().await);
    }

    #[tokio::test]
    async fn hid_inhibit() {
        let _h = testing::start();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        symlink("sony", sys_base.join("driver")).expect("hidraw0");

        assert!(hid.can_inhibit().await);

        hid.inhibit().await.expect("inhibit");
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "1\n"
        );
        hid.uninhibit().await.expect("uninhibit");
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "0\n"
        );
    }

    #[tokio::test]
    async fn hid_inhibit_error_continue() {
        let _h = testing::start();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        create_dir_all(sys_base.join("input/input0/inhibited")).expect("inhibited");
        create_dir_all(sys_base.join("input/input1/mouse1")).expect("mouse0");
        symlink("sony", sys_base.join("driver")).expect("hidraw0");

        assert!(hid.can_inhibit().await);

        assert!(hid.inhibit().await.is_err());
        assert_eq!(
            read_to_string(sys_base.join("input/input1/inhibited")).expect("inhibited"),
            "1\n"
        );
        assert!(hid.uninhibit().await.is_err());
        assert_eq!(
            read_to_string(sys_base.join("input/input1/inhibited")).expect("inhibited"),
            "0\n"
        );
    }

    #[tokio::test]
    async fn hid_check() {
        let h = testing::start();
        let path = h.test.path();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        symlink("sony", sys_base.join("driver")).expect("hidraw0");
        create_dir_all(path.join("proc/1/fd")).expect("fd");

        symlink(hid.hidraw(), path.join("proc/1/fd/3")).expect("symlink");
        write(path.join("proc/1/comm"), "steam\n").expect("comm");

        hid.check().await.expect("check");
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "1\n"
        );

        write(path.join("proc/1/comm"), "epic\n").expect("comm");
        hid.check().await.expect("check");
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "0\n"
        );

        remove_file(path.join("proc/1/fd/3")).expect("rm");
        write(path.join("proc/1/comm"), "steam\n").expect("comm");
        hid.check().await.expect("check");
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "0\n"
        );
    }

    #[tokio::test]
    async fn inhibitor_start() {
        let h = testing::start();
        let path = h.test.path();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(path.join("dev")).expect("dev");
        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        write(hid.hidraw(), "").expect("hidraw");
        symlink("sony", sys_base.join("driver")).expect("driver");
        create_dir_all(path.join("proc/1/fd")).expect("fd");
        symlink(hid.hidraw(), path.join("proc/1/fd/3")).expect("symlink");
        write(path.join("proc/1/comm"), "steam\n").expect("comm");

        let mut inhibitor = Inhibitor::init().await.expect("init");

        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "1\n"
        );

        inhibitor.shutdown().await.expect("stop");

        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "0\n"
        );
    }

    #[tokio::test]
    async fn inhibitor_open_close() {
        let h = testing::start();
        let path = h.test.path();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(path.join("dev")).expect("dev");
        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        File::create(hid.hidraw()).expect("hidraw");
        symlink("sony", sys_base.join("driver")).expect("driver");
        create_dir_all(path.join("proc/1/fd")).expect("fd");
        write(path.join("proc/1/comm"), "steam\n").expect("comm");

        let mut inhibitor = Inhibitor::init().await.expect("init");
        let task = tokio::spawn(async move {
            inhibitor.run().await.expect("run");
        });

        nyield(5).await;
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "0\n"
        );

        symlink(hid.hidraw(), path.join("proc/1/fd/3")).expect("symlink");
        let f = File::open(hid.hidraw()).expect("hidraw");
        nyield(15).await;
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "1\n"
        );

        drop(f);
        remove_file(path.join("proc/1/fd/3")).expect("rm");
        nyield(5).await;
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "0\n"
        );

        task.abort();
    }

    #[tokio::test]
    async fn inhibitor_fast_create() {
        let h = testing::start();
        let path = h.test.path();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(path.join("dev")).expect("dev");
        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        symlink("sony", sys_base.join("driver")).expect("driver");
        create_dir_all(path.join("proc/1/fd")).expect("fd");
        write(path.join("proc/1/comm"), "steam\n").expect("comm");

        let mut inhibitor = Inhibitor::init().await.expect("init");
        let task = tokio::spawn(async move {
            inhibitor.run().await.expect("run");
        });

        nyield(5).await;
        assert!(read_to_string(sys_base.join("input/input0/inhibited")).is_err());

        File::create(hid.hidraw()).expect("hidraw");
        symlink(hid.hidraw(), path.join("proc/1/fd/3")).expect("symlink");
        let _f = File::open(hid.hidraw()).expect("hidraw");
        nyield(300).await;
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "1\n"
        );

        task.abort();
    }

    #[tokio::test]
    async fn inhibitor_create() {
        let _h = testing::start();

        let hid = HidNode::new(0);
        let sys_base = hid.sys_base();

        create_dir_all(path("/dev")).expect("dev");
        create_dir_all(sys_base.join("input/input0/mouse0")).expect("mouse0");
        symlink("sony", sys_base.join("driver")).expect("driver");
        create_dir_all(path("/proc/1/fd")).expect("fd");
        write(path("/proc/1/comm"), "steam\n").expect("comm");

        let mut inhibitor = Inhibitor::init().await.expect("init");
        let task = tokio::spawn(async move {
            inhibitor.run().await.expect("run");
        });

        nyield(5).await;
        assert!(read_to_string(sys_base.join("input/input0/inhibited")).is_err());

        File::create(hid.hidraw()).expect("hidraw");
        nyield(50).await;
        symlink(hid.hidraw(), path("/proc/1/fd/3")).expect("symlink");
        let _f = File::open(hid.hidraw()).expect("hidraw");
        nyield(250).await;
        assert_eq!(
            read_to_string(sys_base.join("input/input0/inhibited")).expect("inhibited"),
            "1\n"
        );

        task.abort();
    }
}
