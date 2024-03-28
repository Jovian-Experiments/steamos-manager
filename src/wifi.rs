/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, ensure, Result};
use std::fmt;
use tokio::fs;
use tracing::error;

use crate::process::{run_script, SYSTEMCTL_PATH};

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

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum WifiDebugMode {
    UnsupportedFeature = 0,
    Off = 1,
    On = 2,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum WifiPowerManagement {
    UnsupportedFeature = 0,
    Disabled = 1,
    Enabled = 2,
}

impl TryFrom<u32> for WifiDebugMode {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == WifiDebugMode::UnsupportedFeature as u32 => {
                Ok(WifiDebugMode::UnsupportedFeature)
            }
            x if x == WifiDebugMode::Off as u32 => Ok(WifiDebugMode::Off),
            x if x == WifiDebugMode::On as u32 => Ok(WifiDebugMode::On),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for WifiDebugMode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WifiDebugMode::UnsupportedFeature => write!(f, "Unsupported feature"),
            WifiDebugMode::Off => write!(f, "Off"),
            WifiDebugMode::On => write!(f, "On"),
        }
    }
}

impl TryFrom<u32> for WifiPowerManagement {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == WifiPowerManagement::UnsupportedFeature as u32 => {
                Ok(WifiPowerManagement::UnsupportedFeature)
            }
            x if x == WifiPowerManagement::Disabled as u32 => Ok(WifiPowerManagement::Disabled),
            x if x == WifiPowerManagement::Enabled as u32 => Ok(WifiPowerManagement::Enabled),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for WifiPowerManagement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WifiPowerManagement::UnsupportedFeature => write!(f, "Unsupported feature"),
            WifiPowerManagement::Disabled => write!(f, "Disabled"),
            WifiPowerManagement::Enabled => write!(f, "Enabled"),
        }
    }
}

pub async fn setup_iwd_config(want_override: bool) -> std::io::Result<()> {
    // Copy override.conf file into place or out of place depending
    // on install value

    if want_override {
        // Copy it in
        // Make sure the folder exists
        fs::create_dir_all(OVERRIDE_FOLDER).await?;
        // Then write the contents into the file
        fs::write(OVERRIDE_PATH, OVERRIDE_CONTENTS).await
    } else {
        // Delete it
        fs::remove_file(OVERRIDE_PATH).await
    }
}

async fn restart_iwd() -> Result<bool> {
    // First reload systemd since we modified the config most likely
    // otherwise we wouldn't be restarting iwd.
    match run_script("reload systemd", SYSTEMCTL_PATH, &["daemon-reload"]).await {
        Ok(value) => {
            if value {
                // worked, now restart iwd
                run_script("restart iwd", SYSTEMCTL_PATH, &["restart", "iwd"]).await
            } else {
                // reload failed
                error!("restart_iwd: reload systemd failed with non-zero exit code");
                Ok(false)
            }
        }
        Err(message) => {
            error!("restart_iwd: reload systemd got an error: {message}");
            Err(message)
        }
    }
}

async fn stop_tracing() -> Result<bool> {
    // Stop tracing and extract ring buffer to disk for capture
    run_script("stop tracing", TRACE_CMD_PATH, &["stop"]).await?;
    // stop tracing worked
    run_script(
        "extract traces",
        TRACE_CMD_PATH,
        &["extract", "-o", OUTPUT_FILE],
    )
    .await
}

async fn start_tracing(buffer_size: u32) -> Result<bool> {
    // Start tracing
    let size_str = format!("{}", buffer_size);
    run_script(
        "start tracing",
        TRACE_CMD_PATH,
        &["start", "-e", "ath11k_wmi_diag", "-b", &size_str],
    )
    .await
}

pub async fn set_wifi_debug_mode(
    mode: WifiDebugMode,
    buffer_size: u32,
    should_trace: bool,
) -> Result<()> {
    // Set the wifi debug mode to mode, using an int for flexibility going forward but only
    // doing things on 0 or 1 for now
    // Return false on error

    match mode {
        WifiDebugMode::Off => {
            // If mode is 0 disable wifi debug mode
            // Stop any existing trace and flush to disk.
            if should_trace {
                let result = match stop_tracing().await {
                    Ok(result) => result,
                    Err(message) => bail!("stop_tracing command got an error: {message}"),
                };
                ensure!(result, "stop_tracing command returned non-zero");
            }
            // Stop_tracing was successful
            if let Err(message) = setup_iwd_config(false).await {
                bail!("setup_iwd_config false got an error: {message}");
            }
            // setup_iwd_config false worked
            let value = match restart_iwd().await {
                Ok(value) => value,
                Err(message) => {
                    bail!("restart_iwd got an error: {message}");
                }
            };
            // restart_iwd failed
            ensure!(value, "restart_iwd failed, check log above");
        }
        WifiDebugMode::On => {
            ensure!(buffer_size > MIN_BUFFER_SIZE, "Buffer size too small");

            if let Err(message) = setup_iwd_config(true).await {
                bail!("setup_iwd_config true got an error: {message}");
            }
            // setup_iwd_config worked
            let value = match restart_iwd().await {
                Ok(value) => value,
                Err(message) => bail!("restart_iwd got an error: {message}"),
            };
            ensure!(value, "restart_iwd failed");
            // restart_iwd worked
            if should_trace {
                let value = match start_tracing(buffer_size).await {
                    Ok(value) => value,
                    Err(message) => bail!("start_tracing got an error: {message}"),
                };
                ensure!(value, "start_tracing failed");
            }
        }
        mode => {
            // Invalid mode requested, more coming later, but add this catch-all for now
            bail!("Invalid wifi debug mode {mode} requested");
        }
    }
    Ok(())
}
