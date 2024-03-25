/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{ensure, Result};
use std::{ffi::OsStr, fmt, fs};
use tokio::{fs::File, io::AsyncWriteExt, process::Command};
use tracing::{error, warn};
use zbus::{interface, zvariant::Fd};

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum WifiDebugMode {
    Off,
    On,
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

pub struct SMManager {
    wifi_debug_mode: WifiDebugMode,
    // Whether we should use trace-cmd or not.
    // True on galileo devices, false otherwise
    should_trace: bool,
}

impl SMManager {
    pub fn new() -> Result<Self> {
        Ok(SMManager {
            wifi_debug_mode: WifiDebugMode::Off,
            should_trace: is_galileo()?,
        })
    }
}

const OVERRIDE_CONTENTS: &str = "[Service]
ExecStart=
ExecStart=/usr/lib/iwd/iwd -d
";
const OVERRIDE_FOLDER: &str = "/etc/systemd/system/iwd.service.d";
const OVERRIDE_PATH: &str = "/etc/systemd/system/iwd.service.d/override.conf";
// Only use one path for output for now. If needed we can add a timestamp later
// to have multiple files, etc.
const OUTPUT_FILE: &str = "/var/log/wifitrace.dat";
const MIN_BUFFER_SIZE: u32 = 100;

const BOARD_NAME_PATH: &str = "/sys/class/dmi/id/board_name";
const GALILEO_NAME: &str = "Galileo";

const ALS_INTEGRATION_PATH: &str = "/sys/devices/platform/AMDI0010:00/i2c-0/i2c-PRP0001:01/iio:device0/in_illuminance_integration_time";
const POWER1_CAP_PATH: &str = "/sys/class/hwmon/hwmon5/power1_cap";
const POWER2_CAP_PATH: &str = "/sys/class/hwmon/hwmon5/power2_cap";

const GPU_PERFORMANCE_LEVEL_PATH: &str =
    "/sys/class/drm/card0/device/power_dpm_force_performance_level";
const GPU_CLOCKS_PATH: &str = "/sys/class/drm/card0/device/pp_od_clk_voltage";

fn is_galileo() -> Result<bool> {
    let mut board_name = fs::read_to_string(BOARD_NAME_PATH)?;
    board_name = board_name.trim().to_string();

    let matches = board_name == GALILEO_NAME;
    Ok(matches)
}

async fn script_exit_code(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<bool> {
    // Run given script and return true on success
    let mut child = Command::new(executable).args(args).spawn()?;
    let status = child.wait().await?;
    Ok(status.success())
}

async fn run_script(name: &str, executable: &str, args: &[impl AsRef<OsStr>]) -> Result<bool> {
    // Run given script to get exit code and return true on success.
    // Return false on failure, but also print an error if needed
    script_exit_code(executable, args)
        .await
        .inspect_err(|message| warn!("Error running {name} {message}"))
}

async fn script_output(executable: &str, args: &[impl AsRef<OsStr>]) -> Result<String> {
    // Run given command and return the output given
    let output = Command::new(executable).args(args).output();

    let output = output.await?;

    let s = std::str::from_utf8(&output.stdout)?;
    Ok(s.to_string())
}

async fn setup_iwd_config(want_override: bool) -> std::io::Result<()> {
    // Copy override.conf file into place or out of place depending
    // on install value

    if want_override {
        // Copy it in
        // Make sure the folder exists
        tokio::fs::create_dir_all(OVERRIDE_FOLDER).await?;
        // Then write the contents into the file
        tokio::fs::write(OVERRIDE_PATH, OVERRIDE_CONTENTS).await
    } else {
        // Delete it
        tokio::fs::remove_file(OVERRIDE_PATH).await
    }
}

async fn restart_iwd() -> Result<bool> {
    // First reload systemd since we modified the config most likely
    // otherwise we wouldn't be restarting iwd.
    match run_script("reload systemd", "systemctl", &["daemon-reload"]).await {
        Ok(value) => {
            if value {
                // worked, now restart iwd
                run_script("restart iwd", "systemctl", &["restart", "iwd"]).await
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
    run_script("stop tracing", "trace-cmd", &["stop"]).await?;
    // stop tracing worked
    run_script(
        "extract traces",
        "trace-cmd",
        &["extract", "-o", OUTPUT_FILE],
    )
    .await
}

async fn start_tracing(buffer_size: u32) -> Result<bool> {
    // Start tracing
    let size_str = format!("{}", buffer_size);
    run_script(
        "start tracing",
        "trace-cmd",
        &["start", "-e", "ath11k_wmi_diag", "-b", &size_str],
    )
    .await
}

async fn set_gpu_performance_level(level: i32) -> Result<()> {
    // Set given GPU performance level
    // Levels are defined below
    // return true if able to write, false otherwise or if level is out of range, etc.
    let levels = ["auto", "low", "high", "manual", "peak_performance"];
    ensure!(
        level >= 0 && level < levels.len() as i32,
        "Invalid performance level"
    );

    let mut myfile = File::create(GPU_PERFORMANCE_LEVEL_PATH)
        .await
        .inspect_err(|message| error!("Error opening sysfs file for writing: {message}"))?;

    myfile
        .write_all(levels[level as usize].as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    Ok(())
}

async fn set_gpu_clocks(clocks: i32) -> Result<()> {
    // Set GPU clocks to given value valid between 200 - 1600
    // Only used when GPU Performance Level is manual, but write whenever called.
    ensure!((200..=1600).contains(&clocks), "Invalid clocks");

    let mut myfile = File::create(GPU_CLOCKS_PATH)
        .await
        .inspect_err(|message| error!("Error opening sysfs file for writing: {message}"))?;

    let data = format!("s 0 {clocks}\n");
    myfile
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;

    let data = format!("s 1 {clocks}\n");
    myfile
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;

    myfile
        .write("c\n".as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    Ok(())
}

async fn set_tdp_limit(limit: i32) -> Result<()> {
    // Set TDP limit given if within range (3-15)
    // Returns false on error or out of range
    ensure!((3..=15).contains(&limit), "Invalid limit");

    let mut power1file = File::create(POWER1_CAP_PATH).await.inspect_err(|message| {
        error!("Error opening sysfs power1_cap file for writing TDP limits {message}")
    })?;

    let mut power2file = File::create(POWER2_CAP_PATH).await.inspect_err(|message| {
        error!("Error opening sysfs power2_cap file for wtriting TDP limits {message}")
    })?;

    // Now write the value * 1,000,000
    let data = format!("{limit}000000");
    power1file
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to power1_cap file: {message}"))?;
    power2file
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to power2_cap file: {message}"))?;
    Ok(())
}

#[interface(name = "com.steampowered.SteamOSManager1")]
impl SMManager {
    const API_VERSION: u32 = 1;

    async fn say_hello(&self, name: &str) -> String {
        format!("Hello {}!", name)
    }

    async fn factory_reset(&self) -> bool {
        // Run steamos factory reset script and return true on success
        run_script("factory reset", "steamos-factory-reset-config", &[""])
            .await
            .unwrap_or(false)
    }

    async fn disable_wifi_power_management(&self) -> bool {
        // Run polkit helper script and return true on success
        run_script(
            "disable wifi power management",
            "/usr/bin/steamos-polkit-helpers/steamos-disable-wireless-power-management",
            &[""],
        )
        .await
        .unwrap_or(false)
    }

    async fn enable_fan_control(&self, enable: bool) -> bool {
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        if enable {
            run_script(
                "enable fan control",
                "systemcltl",
                &["start", "jupiter-fan-control-service"],
            )
            .await
            .unwrap_or(false)
        } else {
            run_script(
                "disable fan control",
                "systemctl",
                &["stop", "jupiter-fan-control.service"],
            )
            .await
            .unwrap_or(false)
        }
    }

    async fn hardware_check_support(&self) -> bool {
        // Run jupiter-check-support note this script does exit 1 for "Support: No" case
        // so no need to parse output, etc.
        run_script("check hardware support", "jupiter-check-support", &[""])
            .await
            .unwrap_or(false)
    }

    async fn read_als_calibration(&self) -> f32 {
        // Run script to get calibration value
        let result = script_output(
            "/usr/bin/steamos-polkit-helpers/jupiter-get-als-gain",
            &[""],
        )
        .await;
        match result {
            Ok(as_string) => as_string.trim().parse().unwrap_or(-1.0),
            Err(message) => {
                error!("Unable to run als calibration script: {}", message);
                -1.0
            }
        }
    }

    async fn update_bios(&self) -> bool {
        // Update the bios as needed
        // Return true if the script was successful (though that might mean no update was needed), false otherwise
        run_script(
            "update bios",
            "/usr/bin/steamos-potlkit-helpers/jupiter-biosupdate",
            &["--auto"],
        )
        .await
        .unwrap_or(false)
    }

    async fn update_dock(&self) -> bool {
        // Update the dock firmware as needed
        // Retur true if successful, false otherwise
        run_script(
            "update dock firmware",
            "/usr/bin/steamos-polkit-helpers/jupiter-dock-updater",
            &[""],
        )
        .await
        .unwrap_or(false)
    }

    async fn trim_devices(&self) -> bool {
        // Run steamos-trim-devices script
        // return true on success, false otherwise
        run_script(
            "trim devices",
            "/usr/bin/steamos-polkit-helpers/steamos-trim-devices",
            &[""],
        )
        .await
        .unwrap_or(false)
    }

    async fn format_sdcard(&self) -> bool {
        // Run steamos-format-sdcard script
        // return true on success, false otherwise
        run_script(
            "format sdcard",
            "/usr/bin/steamos-polkit-helpers/steamos-format-sdcard",
            &[""],
        )
        .await
        .unwrap_or(false)
    }

    async fn set_gpu_performance_level(&self, level: i32) -> bool {
        set_gpu_performance_level(level).await.is_ok()
    }

    async fn set_gpu_clocks(&self, clocks: i32) -> bool {
        set_gpu_clocks(clocks).await.is_ok()
    }

    async fn set_tdp_limit(&self, limit: i32) -> bool {
        set_tdp_limit(limit).await.is_ok()
    }

    async fn get_als_integration_time_file_descriptor(&self) -> Result<Fd, zbus::fdo::Error> {
        // Get the file descriptor for the als integration time sysfs path
        let result = File::create(ALS_INTEGRATION_PATH).await;
        match result {
            Ok(f) => Ok(Fd::Owned(std::os::fd::OwnedFd::from(f.into_std().await))),
            Err(message) => {
                error!("Error opening sysfs file for giving file descriptor: {message}");
                Err(zbus::fdo::Error::IOError(message.to_string()))
            }
        }
    }

    async fn get_wifi_debug_mode(&mut self) -> u32 {
        // Get the wifi debug mode
        self.wifi_debug_mode as u32
    }

    async fn set_wifi_debug_mode(&mut self, mode: u32, buffer_size: u32) -> bool {
        // Set the wifi debug mode to mode, using an int for flexibility going forward but only
        // doing things on 0 or 1 for now
        // Return false on error

        let wanted_mode = WifiDebugMode::try_from(mode);
        match wanted_mode {
            Ok(WifiDebugMode::Off) => {
                // If mode is 0 disable wifi debug mode
                // Stop any existing trace and flush to disk.
                if self.should_trace {
                    let result = match stop_tracing().await {
                        Ok(result) => result,
                        Err(message) => {
                            error!("stop_tracing command got an error: {message}");
                            return false;
                        }
                    };
                    if !result {
                        error!("stop_tracing command returned non-zero");
                        return false;
                    }
                }
                // Stop_tracing was successful
                if let Err(message) = setup_iwd_config(false).await {
                    error!("setup_iwd_config false got an error: {message}");
                    return false;
                }
                // setup_iwd_config false worked
                let value = match restart_iwd().await {
                    Ok(value) => value,
                    Err(message) => {
                        error!("restart_iwd got an error: {message}");
                        return false;
                    }
                };
                if value {
                    // restart iwd worked
                    self.wifi_debug_mode = WifiDebugMode::Off;
                } else {
                    // restart_iwd failed
                    error!("restart_iwd failed, check log above");
                    return false;
                }
            }
            Ok(WifiDebugMode::On) => {
                // If mode is 1 enable wifi debug mode
                if buffer_size < MIN_BUFFER_SIZE {
                    return false;
                }

                if let Err(message) = setup_iwd_config(true).await {
                    error!("setup_iwd_config true got an error: {message}");
                    return false;
                }
                // setup_iwd_config worked
                let value = match restart_iwd().await {
                    Ok(value) => value,
                    Err(message) => {
                        error!("restart_iwd got an error: {message}");
                        return false;
                    }
                };
                if !value {
                    error!("restart_iwd failed");
                    return false;
                }
                // restart_iwd worked
                if self.should_trace {
                    let value = match start_tracing(buffer_size).await {
                        Ok(value) => value,
                        Err(message) => {
                            error!("start_tracing got an error: {message}");
                            return false;
                        }
                    };
                    if !value {
                        // start_tracing failed
                        error!("start_tracing failed");
                        return false;
                    }
                }
                // start_tracing worked
                self.wifi_debug_mode = WifiDebugMode::On;
            }
            Err(_) => {
                // Invalid mode requested, more coming later, but add this catch-all for now
                warn!("Invalid wifi debug mode {mode} requested");
                return false;
            }
        }

        true
    }

    /// A version property.
    #[zbus(property)]
    async fn version(&self) -> u32 {
        SMManager::API_VERSION
    }
}
