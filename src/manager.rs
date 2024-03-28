/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use tokio::fs::File;
use tracing::{error, warn};
use zbus::{interface, zvariant::Fd};

use crate::hardware::{variant, HardwareVariant};
use crate::power::{set_gpu_clocks, set_gpu_performance_level, set_tdp_limit};
use crate::process::{run_script, script_output, SYSTEMCTL_PATH};
use crate::wifi::{restart_iwd, setup_iwd_config, start_tracing, stop_tracing, WifiDebugMode};

pub struct SMManager {
    wifi_debug_mode: WifiDebugMode,
    // Whether we should use trace-cmd or not.
    // True on galileo devices, false otherwise
    should_trace: bool,
}

impl SMManager {
    pub async fn new() -> Result<Self> {
        Ok(SMManager {
            wifi_debug_mode: WifiDebugMode::Off,
            should_trace: variant().await? == HardwareVariant::Galileo,
        })
    }
}

const MIN_BUFFER_SIZE: u32 = 100;

const ALS_INTEGRATION_PATH: &str = "/sys/devices/platform/AMDI0010:00/i2c-0/i2c-PRP0001:01/iio:device0/in_illuminance_integration_time";

#[interface(name = "com.steampowered.SteamOSManager1")]
impl SMManager {
    const API_VERSION: u32 = 1;

    async fn say_hello(&self, name: &str) -> String {
        format!("Hello {}!", name)
    }

    async fn factory_reset(&self) -> bool {
        // Run steamos factory reset script and return true on success
        run_script(
            "factory reset",
            "/usr/bin/steamos-factory-reset-config",
            &[""],
        )
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
                SYSTEMCTL_PATH,
                &["start", "jupiter-fan-control-service"],
            )
            .await
            .unwrap_or(false)
        } else {
            run_script(
                "disable fan control",
                SYSTEMCTL_PATH,
                &["stop", "jupiter-fan-control.service"],
            )
            .await
            .unwrap_or(false)
        }
    }

    async fn hardware_check_support(&self) -> bool {
        // Run jupiter-check-support note this script does exit 1 for "Support: No" case
        // so no need to parse output, etc.
        run_script(
            "check hardware support",
            "/usr/bin/jupiter-check-support",
            &[""],
        )
        .await
        .unwrap_or(false)
    }

    async fn read_als_calibration(&self) -> f32 {
        // Run script to get calibration value
        let result = script_output(
            "/usr/bin/steamos-polkit-helpers/jupiter-get-als-gain",
            &[] as &[String; 0],
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
