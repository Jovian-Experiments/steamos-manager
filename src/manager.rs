/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use std::fmt;
use tokio::fs::File;
use tracing::error;
use zbus::{interface, zvariant::Fd};

use crate::hardware::{check_support, variant, HardwareCurrentlySupported, HardwareVariant};
use crate::power::{
    get_gpu_performance_level, set_gpu_clocks, set_gpu_performance_level, set_tdp_limit,
    GPUPerformanceLevel,
};
use crate::process::{run_script, script_output, SYSTEMCTL_PATH};
use crate::wifi::{set_wifi_debug_mode, WifiDebugMode, WifiPowerManagement};

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum PrepareFactoryReset {
    Unknown = 0,
    RebootRequired = 1,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum FanControl {
    UnsupportedFeature = 0,
    BIOS = 1,
    OS = 2,
}

impl TryFrom<u32> for FanControl {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == FanControl::UnsupportedFeature as u32 => Ok(FanControl::UnsupportedFeature),
            x if x == FanControl::BIOS as u32 => Ok(FanControl::BIOS),
            x if x == FanControl::OS as u32 => Ok(FanControl::BIOS),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for FanControl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FanControl::UnsupportedFeature => write!(f, "Unsupported feature"),
            FanControl::BIOS => write!(f, "BIOS"),
            FanControl::OS => write!(f, "OS"),
        }
    }
}

pub struct SteamOSManager {
    wifi_debug_mode: WifiDebugMode,
    // Whether we should use trace-cmd or not.
    // True on galileo devices, false otherwise
    should_trace: bool,
}

impl SteamOSManager {
    pub async fn new() -> Result<Self> {
        Ok(SteamOSManager {
            wifi_debug_mode: WifiDebugMode::Off,
            should_trace: variant().await? == HardwareVariant::Galileo,
        })
    }
}

const ALS_INTEGRATION_PATH: &str = "/sys/devices/platform/AMDI0010:00/i2c-0/i2c-PRP0001:01/iio:device0/in_illuminance_integration_time";

#[interface(name = "com.steampowered.SteamOSManager1.Manager")]
impl SteamOSManager {
    const API_VERSION: u32 = 7;

    async fn prepare_factory_reset(&self) -> u32 {
        // Run steamos factory reset script and return true on success
        let res = run_script(
            "factory reset",
            "/usr/bin/steamos-factory-reset-config",
            &[""],
        )
        .await
        .unwrap_or(false);
        match res {
            true => PrepareFactoryReset::RebootRequired as u32,
            false => PrepareFactoryReset::Unknown as u32,
        }
    }

    #[zbus(property)]
    fn wifi_power_management_state(&self) -> u32 {
        WifiPowerManagement::UnsupportedFeature as u32 // TODO
    }

    #[zbus(property)]
    async fn set_wifi_power_management_state(&self, state: u32) -> zbus::Result<()> {
        let state = match WifiPowerManagement::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(zbus::fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        let state = match state {
            WifiPowerManagement::Disabled => "off",
            WifiPowerManagement::Enabled => "on",
            WifiPowerManagement::UnsupportedFeature => {
                return Err(zbus::fdo::Error::InvalidArgs(String::from(
                    "Can't set state to unsupported",
                ))
                .into())
            }
        };

        let res = run_script(
            "set wifi power management",
            "/usr/bin/iwconfig",
            &["wlan0", "power", state],
        )
        .await;

        match res {
            Ok(true) => Ok(()),
            Ok(false) => Err(zbus::Error::Failure(String::from(
                "iwconfig returned non-zero",
            ))),
            Err(e) => Err(zbus::Error::Failure(e.to_string())),
        }
    }

    #[zbus(property)]
    fn fan_control_state(&self) -> u32 {
        FanControl::UnsupportedFeature as u32 // TODO
    }

    #[zbus(property)]
    async fn set_fan_control_state(&self, state: u32) -> zbus::Result<()> {
        let state = match FanControl::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(zbus::fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        let state = match state {
            FanControl::OS => "stop",
            FanControl::BIOS => "start",
            FanControl::UnsupportedFeature => {
                return Err(zbus::fdo::Error::InvalidArgs(String::from(
                    "Can't set state to unsupported",
                ))
                .into())
            }
        };

        // Run what steamos-polkit-helpers/jupiter-fan-control does
        let res = run_script(
            "enable fan control",
            SYSTEMCTL_PATH,
            &[state, "jupiter-fan-control-service"],
        )
        .await;

        match res {
            Ok(true) => Ok(()),
            Ok(false) => Err(zbus::Error::Failure(String::from(
                "systemctl returned non-zero",
            ))),
            Err(e) => Err(zbus::Error::Failure(format!("{e}"))),
        }
    }

    #[zbus(property)]
    async fn hardware_currently_supported(&self) -> u32 {
        match check_support().await {
            Ok(res) => res as u32,
            Err(_) => HardwareCurrentlySupported::UnsupportedFeature as u32,
        }
    }

    #[zbus(property)]
    async fn als_calibration_gain(&self) -> f64 {
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

    async fn update_bios(&self) -> Result<(), zbus::fdo::Error> {
        // Update the bios as needed
        let res = run_script(
            "update bios",
            "/usr/bin/steamos-potlkit-helpers/jupiter-biosupdate",
            &["--auto"],
        )
        .await;

        match res {
            Ok(_) => Ok(()),
            Err(e) => Err(zbus::fdo::Error::Failed(e.to_string())),
        }
    }

    async fn update_dock(&self) -> Result<(), zbus::fdo::Error> {
        // Update the dock firmware as needed
        let res = run_script(
            "update dock firmware",
            "/usr/bin/steamos-polkit-helpers/jupiter-dock-updater",
            &[""],
        )
        .await;

        match res {
            Ok(_) => Ok(()),
            Err(e) => Err(zbus::fdo::Error::Failed(e.to_string())),
        }
    }

    async fn trim_devices(&self) -> Result<(), zbus::fdo::Error> {
        // Run steamos-trim-devices script
        let res = run_script(
            "trim devices",
            "/usr/bin/steamos-polkit-helpers/steamos-trim-devices",
            &[""],
        )
        .await;

        match res {
            Ok(_) => Ok(()),
            Err(e) => Err(zbus::fdo::Error::Failed(e.to_string())),
        }
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

    #[zbus(property)]
    async fn gpu_performance_level(&self) -> u32 {
        match get_gpu_performance_level().await {
            Ok(level) => level as u32,
            Err(_) => GPUPerformanceLevel::UnsupportedFeature as u32,
        }
    }

    #[zbus(property)]
    async fn set_gpu_performance_level(&self, level: u32) -> zbus::Result<()> {
        let level = match GPUPerformanceLevel::try_from(level) {
            Ok(level) => level,
            Err(e) => return Err(zbus::Error::Failure(e.to_string())),
        };
        set_gpu_performance_level(level)
            .await
            .map_err(|e| zbus::Error::Failure(e.to_string()))
    }

    async fn set_gpu_clocks(&self, clocks: i32) -> bool {
        set_gpu_clocks(clocks).await.is_ok()
    }

    async fn set_tdp_limit(&self, limit: i32) -> bool {
        set_tdp_limit(limit).await.is_ok()
    }

    #[zbus(property)]
    async fn wifi_debug_mode_state(&self) -> u32 {
        // Get the wifi debug mode
        self.wifi_debug_mode as u32
    }

    async fn set_wifi_debug_mode(
        &mut self,
        mode: u32,
        buffer_size: u32,
    ) -> Result<(), zbus::fdo::Error> {
        // Set the wifi debug mode to mode, using an int for flexibility going forward but only
        // doing things on 0 or 1 for now
        // Return false on error

        let wanted_mode = match WifiDebugMode::try_from(mode) {
            Ok(WifiDebugMode::UnsupportedFeature) => {
                return Err(zbus::fdo::Error::InvalidArgs(String::from("Invalid mode")))
            }
            Ok(mode) => mode,
            Err(e) => return Err(zbus::fdo::Error::InvalidArgs(e.to_string())),
        };
        match set_wifi_debug_mode(wanted_mode, buffer_size, self.should_trace).await {
            Ok(()) => {
                self.wifi_debug_mode = wanted_mode;
                Ok(())
            }
            Err(e) => {
                error!("Setting wifi debug mode failed: {e}");
                Err(zbus::fdo::Error::Failed(e.to_string()))
            }
        }
    }

    /// A version property.
    #[zbus(property)]
    async fn version(&self) -> u32 {
        SteamOSManager::API_VERSION
    }
}
