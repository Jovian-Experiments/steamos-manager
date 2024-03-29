/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use std::fmt;
use tokio::fs::File;
use tracing::error;
use zbus::{interface, zvariant::Fd};

use crate::hardware::{check_support, variant, HardwareVariant};
use crate::power::{
    get_gpu_performance_level, set_gpu_clocks, set_gpu_performance_level, set_tdp_limit,
    GPUPerformanceLevel,
};
use crate::process::{run_script, script_output, SYSTEMCTL_PATH};
use crate::wifi::{
    get_wifi_backend_from_conf, get_wifi_backend_from_script, set_wifi_backend,
    set_wifi_debug_mode, WifiBackend, WifiDebugMode, WifiPowerManagement,
};
use crate::{anyhow_to_zbus, anyhow_to_zbus_fdo};

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum PrepareFactoryReset {
    Unknown = 0,
    RebootRequired = 1,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum FanControl {
    BIOS = 1,
    OS = 2,
}

impl TryFrom<u32> for FanControl {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == FanControl::BIOS as u32 => Ok(FanControl::BIOS),
            x if x == FanControl::OS as u32 => Ok(FanControl::BIOS),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for FanControl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FanControl::BIOS => write!(f, "BIOS"),
            FanControl::OS => write!(f, "OS"),
        }
    }
}

pub struct SteamOSManager {
    wifi_backend: WifiBackend,
    wifi_debug_mode: WifiDebugMode,
    // Whether we should use trace-cmd or not.
    // True on galileo devices, false otherwise
    should_trace: bool,
}

impl SteamOSManager {
    pub async fn new() -> Result<Self> {
        Ok(SteamOSManager {
            wifi_backend: get_wifi_backend_from_conf().await?,
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
        .await;
        match res {
            Ok(_) => PrepareFactoryReset::RebootRequired as u32,
            Err(_) => PrepareFactoryReset::Unknown as u32,
        }
    }

    #[zbus(property)]
    fn wifi_power_management_state(&self) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::UnknownProperty(String::from("This property can't currently be read")))
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
        };

        run_script(
            "set wifi power management",
            "/usr/bin/iwconfig",
            &["wlan0", "power", state],
        )
        .await
        .map_err(anyhow_to_zbus)
    }

    #[zbus(property)]
    fn fan_control_state(&self) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::UnknownProperty(String::from("This property can't currently be read")))
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
        };

        // Run what steamos-polkit-helpers/jupiter-fan-control does
        run_script(
            "enable fan control",
            SYSTEMCTL_PATH,
            &[state, "jupiter-fan-control-service"],
        )
        .await
        .map_err(anyhow_to_zbus)
    }

    #[zbus(property)]
    async fn hardware_currently_supported(&self) -> zbus::fdo::Result<u32> {
        match check_support().await {
            Ok(res) => Ok(res as u32),
            Err(e) => Err(anyhow_to_zbus_fdo(e)),
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
        run_script(
            "update bios",
            "/usr/bin/steamos-potlkit-helpers/jupiter-biosupdate",
            &["--auto"],
        )
        .await
        .map_err(anyhow_to_zbus_fdo)
    }

    async fn update_dock(&self) -> Result<(), zbus::fdo::Error> {
        // Update the dock firmware as needed
        run_script(
            "update dock firmware",
            "/usr/bin/steamos-polkit-helpers/jupiter-dock-updater",
            &[""],
        )
        .await
        .map_err(anyhow_to_zbus_fdo)
    }

    async fn trim_devices(&self) -> Result<(), zbus::fdo::Error> {
        // Run steamos-trim-devices script
        run_script(
            "trim devices",
            "/usr/bin/steamos-polkit-helpers/steamos-trim-devices",
            &[""],
        )
        .await
        .map_err(anyhow_to_zbus_fdo)
    }

    async fn format_device(
        &self,
        device: &str,
        label: &str,
        validate: bool,
    ) -> Result<(), zbus::fdo::Error> {
        let mut args = vec!["--label", label, "--device", device];
        if !validate {
            args.push("--skip-validation");
        }
        run_script(
            "format device",
            "/usr/lib/hwsupport/format-device.sh",
            args.as_ref(),
        )
        .await
        .map_err(anyhow_to_zbus_fdo)
    }

    #[zbus(property)]
    async fn gpu_performance_level(&self) -> zbus::fdo::Result<u32> {
        match get_gpu_performance_level().await {
            Ok(level) => Ok(level as u32),
            Err(e) => Err(anyhow_to_zbus_fdo(e)),
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
            .map_err(anyhow_to_zbus)
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

    /// WifiBackend property.
    #[zbus(property)]
    async fn wifi_backend(&self) -> u32 {
        self.wifi_backend as u32
    }

    #[zbus(property)]
    async fn set_wifi_backend(&mut self, backend: u32) -> zbus::fdo::Result<()> {
        if self.wifi_debug_mode == WifiDebugMode::On {
            return Err(zbus::fdo::Error::Failed(String::from(
                "operation not supported when wifi_debug_mode=on",
            )));
        }
        let backend = match WifiBackend::try_from(backend) {
            Ok(backend) => backend,
            Err(e) => return Err(zbus::fdo::Error::InvalidArgs(e.to_string())),
        };
        match set_wifi_backend(backend).await {
            Ok(()) => {
                self.wifi_backend = backend;
                Ok(())
            }
            Err(e) => {
                error!("Setting wifi backend failed: {e}");
                Err(anyhow_to_zbus_fdo(e))
            }
        }
    }

    async fn set_wifi_debug_mode(
        &mut self,
        mode: u32,
        buffer_size: u32,
    ) -> Result<(), zbus::fdo::Error> {
        // Set the wifi debug mode to mode, using an int for flexibility going forward but only
        // doing things on 0 or 1 for now
        // Return false on error
        match get_wifi_backend_from_script().await {
            Ok(WifiBackend::IWD) => (),
            Ok(backend) => {
                return Err(zbus::fdo::Error::Failed(format!(
                    "Setting wifi debug mode not supported when backend is {backend}",
                )));
            }
            Err(e) => return Err(anyhow_to_zbus_fdo(e)),
        }

        let wanted_mode = match WifiDebugMode::try_from(mode) {
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
                Err(anyhow_to_zbus_fdo(e))
            }
        }
    }

    /// A version property.
    #[zbus(property)]
    async fn version(&self) -> u32 {
        SteamOSManager::API_VERSION
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use tokio::fs::{create_dir_all, write};
    use zbus::connection::Connection;
    use zbus::ConnectionBuilder;

    struct TestHandle {
        handle: testing::TestHandle,
        connection: Connection,
    }

    async fn start() -> TestHandle {
        let handle = testing::start();
        create_dir_all(crate::path("/sys/class/dmi/id"))
            .await
            .expect("create_dir_all");
        write(crate::path("/sys/class/dmi/id/board_vendor"), "Valve\n")
            .await
            .expect("write");
        write(crate::path("/sys/class/dmi/id/board_name"), "Jupiter\n")
            .await
            .expect("write");
        create_dir_all(crate::path("/etc/NetworkManager/conf.d"))
            .await
            .expect("create_dir_all");
        write(crate::path("/etc/NetworkManager/conf.d/wifi_backend.conf"), "wifi.backend=iwd\n")
            .await
            .expect("write");

        let manager = SteamOSManager::new().await.unwrap();
        let connection = ConnectionBuilder::session()
            .unwrap()
            .name("com.steampowered.SteamOSManager1.Test")
            .unwrap()
            .serve_at("/com/steampowered/SteamOSManager1", manager)
            .unwrap()
            .build()
            .await
            .unwrap();

        TestHandle { handle, connection }
    }

    #[zbus::proxy(
        interface = "com.steampowered.SteamOSManager1.Manager",
        default_service = "com.steampowered.SteamOSManager1.Test",
        default_path = "/com/steampowered/SteamOSManager1"
    )]
    trait Version {
        #[zbus(property)]
        fn version(&self) -> zbus::Result<u32>;
    }

    #[tokio::test]
    async fn version() {
        let test = start().await;
        let proxy = VersionProxy::new(&test.connection).await.unwrap();
        assert_eq!(proxy.version().await, Ok(SteamOSManager::API_VERSION));
    }
}
