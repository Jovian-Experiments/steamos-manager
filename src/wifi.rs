/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, ensure, Error, Result};
use std::fmt;
use std::str::FromStr;
use tokio::fs;
use tracing::error;
use zbus::Connection;

use crate::path;
use crate::process::{run_script, script_output};
use crate::systemd::{daemon_reload, SystemdUnit};

const OVERRIDE_CONTENTS: &str = "[Service]
ExecStart=
ExecStart=/usr/lib/iwd/iwd -d
";
const OVERRIDE_FOLDER: &str = "/etc/systemd/system/iwd.service.d";
const OVERRIDE_PATH: &str = "/etc/systemd/system/iwd.service.d/override.conf";

// Only use one path for output for now. If needed we can add a timestamp later
// to have multiple files, etc.
const OUTPUT_FILE: &str = "/var/log/wifitrace.dat";
const TRACE_CMD_PATH: &str = "/usr/bin/trace-cmd";

const MIN_BUFFER_SIZE: u32 = 100;

const WIFI_BACKEND_PATH: &str = "/etc/NetworkManager/conf.d/wifi_backend.conf";

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum WifiDebugMode {
    Off = 0,
    On = 1,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum WifiPowerManagement {
    Disabled = 0,
    Enabled = 1,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum WifiBackend {
    Iwd = 0,
    WPASupplicant = 1,
}

impl TryFrom<u32> for WifiDebugMode {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == WifiDebugMode::Off as u32 => Ok(WifiDebugMode::Off),
            x if x == WifiDebugMode::On as u32 => Ok(WifiDebugMode::On),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for WifiDebugMode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WifiDebugMode::Off => write!(f, "Off"),
            WifiDebugMode::On => write!(f, "On"),
        }
    }
}

impl TryFrom<u32> for WifiPowerManagement {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == WifiPowerManagement::Disabled as u32 => Ok(WifiPowerManagement::Disabled),
            x if x == WifiPowerManagement::Enabled as u32 => Ok(WifiPowerManagement::Enabled),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for WifiPowerManagement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WifiPowerManagement::Disabled => write!(f, "Disabled"),
            WifiPowerManagement::Enabled => write!(f, "Enabled"),
        }
    }
}

impl TryFrom<u32> for WifiBackend {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == WifiBackend::Iwd as u32 => Ok(WifiBackend::Iwd),
            x if x == WifiBackend::WPASupplicant as u32 => Ok(WifiBackend::WPASupplicant),
            _ => Err("No enum match for WifiBackend value {v}"),
        }
    }
}

impl FromStr for WifiBackend {
    type Err = Error;
    fn from_str(input: &str) -> Result<WifiBackend, Self::Err> {
        Ok(match input {
            "iwd" => WifiBackend::Iwd,
            "wpa_supplicant" => WifiBackend::WPASupplicant,
            _ => bail!("Unknown backend"),
        })
    }
}

impl fmt::Display for WifiBackend {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WifiBackend::Iwd => write!(f, "iwd"),
            WifiBackend::WPASupplicant => write!(f, "wpa_supplicant"),
        }
    }
}

pub async fn setup_iwd_config(want_override: bool) -> std::io::Result<()> {
    // Copy override.conf file into place or out of place depending
    // on install value

    if want_override {
        // Copy it in
        // Make sure the folder exists
        fs::create_dir_all(path(OVERRIDE_FOLDER)).await?;
        // Then write the contents into the file
        fs::write(path(OVERRIDE_PATH), OVERRIDE_CONTENTS).await
    } else {
        // Delete it
        match fs::remove_file(path(OVERRIDE_PATH)).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            res => res,
        }
    }
}

async fn restart_iwd(connection: Connection) -> Result<()> {
    // First reload systemd since we modified the config most likely
    // otherwise we wouldn't be restarting iwd.
    daemon_reload(&connection)
        .await
        .inspect_err(|message| error!("restart_iwd: reload systemd got an error: {message}"))?;

    // worked, now restart iwd
    let unit = SystemdUnit::new(connection, "iwd_2eservice").await?;
    unit.restart()
        .await
        .inspect_err(|message| error!("restart_iwd: restart unit got an error: {message}"))
}

async fn stop_tracing() -> Result<()> {
    // Stop tracing and extract ring buffer to disk for capture
    run_script(TRACE_CMD_PATH, &["stop"]).await?;
    // stop tracing worked
    run_script(TRACE_CMD_PATH, &["extract", "-o", OUTPUT_FILE]).await
}

async fn start_tracing(buffer_size: u32) -> Result<()> {
    // Start tracing
    let size_str = buffer_size.to_string();
    run_script(
        TRACE_CMD_PATH,
        &["start", "-e", "ath11k_wmi_diag", "-b", &size_str],
    )
    .await
}

pub async fn set_wifi_debug_mode(
    mode: WifiDebugMode,
    buffer_size: u32,
    should_trace: bool,
    connection: Connection,
) -> Result<()> {
    match get_wifi_backend().await {
        Ok(WifiBackend::Iwd) => (),
        Ok(backend) => bail!("Setting wifi debug mode not supported when backend is {backend}"),
        Err(e) => return Err(e),
    }

    match mode {
        WifiDebugMode::Off => {
            // If mode is 0 disable wifi debug mode
            // Stop any existing trace and flush to disk.
            if should_trace {
                if let Err(message) = stop_tracing().await {
                    bail!("stop_tracing command got an error: {message}");
                };
            }
            // Stop_tracing was successful
            if let Err(message) = setup_iwd_config(false).await {
                bail!("setup_iwd_config false got an error: {message}");
            };
            // setup_iwd_config false worked
            if let Err(message) = restart_iwd(connection).await {
                bail!("restart_iwd got an error: {message}");
            };
        }
        WifiDebugMode::On => {
            ensure!(buffer_size > MIN_BUFFER_SIZE, "Buffer size too small");

            if let Err(message) = setup_iwd_config(true).await {
                bail!("setup_iwd_config true got an error: {message}");
            }
            // setup_iwd_config worked
            if let Err(message) = restart_iwd(connection).await {
                bail!("restart_iwd got an error: {message}");
            };
            // restart_iwd worked
            if should_trace {
                if let Err(message) = start_tracing(buffer_size).await {
                    bail!("start_tracing got an error: {message}");
                };
            }
        }
    }
    Ok(())
}

pub async fn get_wifi_backend() -> Result<WifiBackend> {
    let wifi_backend_contents = fs::read_to_string(path(WIFI_BACKEND_PATH))
        .await?
        .trim()
        .to_string();
    for line in wifi_backend_contents.lines() {
        if line.starts_with("wifi.backend=") {
            let backend = line.trim_start_matches("wifi.backend=").trim();
            return WifiBackend::from_str(backend);
        }
    }

    bail!("WiFi backend not found in config");
}

pub async fn set_wifi_backend(backend: WifiBackend) -> Result<()> {
    run_script("/usr/bin/steamos-wifi-set-backend", &[backend.to_string()]).await
}

pub async fn get_wifi_power_management_state() -> Result<WifiPowerManagement> {
    let output = script_output("/usr/bin/iwconfig", &["wlan0"]).await?;
    for line in output.lines() {
        return Ok(match line.trim() {
            "Power Management:on" => WifiPowerManagement::Enabled,
            "Power Management:off" => WifiPowerManagement::Disabled,
            _ => continue,
        });
    }
    bail!("Failed to query power management state")
}

pub async fn set_wifi_power_management_state(state: WifiPowerManagement) -> Result<()> {
    let state = match state {
        WifiPowerManagement::Disabled => "off",
        WifiPowerManagement::Enabled => "on",
    };

    run_script("/usr/bin/iwconfig", &["wlan0", "power", state])
        .await
        .inspect_err(|message| error!("Error setting wifi power management state: {message}"))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use tokio::fs::{create_dir_all, read_to_string, remove_dir, try_exists, write};

    fn test_wifi_backend_to_string() {
        assert_eq!(WifiBackend::Iwd.to_string(), "iwd");
        assert_eq!(WifiBackend::WPASupplicant.to_string(), "wpa_supplicant");
    }

    #[tokio::test]
    async fn test_setup_iwd_config() {
        let _h = testing::start();

        // Remove with no dir
        assert!(setup_iwd_config(false).await.is_ok());

        create_dir_all(path(OVERRIDE_FOLDER))
            .await
            .expect("create_dir_all");

        // Remove with dir but no file
        assert!(setup_iwd_config(false).await.is_ok());

        // Remove with dir and file
        write(path(OVERRIDE_PATH), "").await.expect("write");
        assert!(try_exists(path(OVERRIDE_PATH)).await.unwrap());

        assert!(setup_iwd_config(false).await.is_ok());
        assert!(!try_exists(path(OVERRIDE_PATH)).await.unwrap());

        // Double remove
        assert!(setup_iwd_config(false).await.is_ok());

        // Create with no dir
        remove_dir(path(OVERRIDE_FOLDER)).await.expect("remove_dir");

        assert!(setup_iwd_config(true).await.is_ok());
        assert_eq!(
            read_to_string(path(OVERRIDE_PATH)).await.unwrap(),
            OVERRIDE_CONTENTS
        );

        // Create with dir
        assert!(setup_iwd_config(false).await.is_ok());
        assert!(setup_iwd_config(true).await.is_ok());
        assert_eq!(
            read_to_string(path(OVERRIDE_PATH)).await.unwrap(),
            OVERRIDE_CONTENTS
        );
    }

    #[tokio::test]
    async fn test_get_wifi_backend() {
        let _h = testing::start();

        create_dir_all(path(WIFI_BACKEND_PATH).parent().unwrap())
            .await
            .expect("create_dir_all");

        assert!(get_wifi_backend().await.is_err());

        write(path(WIFI_BACKEND_PATH), "[device]")
            .await
            .expect("write");
        assert!(get_wifi_backend().await.is_err());

        write(path(WIFI_BACKEND_PATH), "[device]\nwifi.backend=fake\n")
            .await
            .expect("write");
        assert!(get_wifi_backend().await.is_err());

        write(path(WIFI_BACKEND_PATH), "[device]\nwifi.backend=iwd\n")
            .await
            .expect("write");
        assert_eq!(get_wifi_backend().await.unwrap(), WifiBackend::Iwd);

        write(
            path(WIFI_BACKEND_PATH),
            "[device]\nwifi.backend=wpa_supplicant\n",
        )
        .await
        .expect("write");
        assert_eq!(
            get_wifi_backend().await.unwrap(),
            WifiBackend::WPASupplicant
        );
    }
}
