/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use tokio::fs::File;
use tracing::error;
use zbus::zvariant::Fd;
use zbus::{fdo, interface, Connection, SignalContext};

use crate::error::{to_zbus_error, to_zbus_fdo_error};
use crate::hardware::{variant, FanControl, FanControlState, HardwareVariant};
use crate::power::{set_gpu_clocks, set_gpu_performance_level, set_tdp_limit, GPUPerformanceLevel};
use crate::process::{run_script, script_output, ProcessManager};
use crate::wifi::{
    set_wifi_backend, set_wifi_debug_mode, set_wifi_power_management_state, WifiBackend,
    WifiDebugMode, WifiPowerManagement,
};
use crate::API_VERSION;

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum PrepareFactoryReset {
    Unknown = 0,
    RebootRequired = 1,
}

pub struct SteamOSManager {
    connection: Connection,
    wifi_debug_mode: WifiDebugMode,
    fan_control: FanControl,
    // Whether we should use trace-cmd or not.
    // True on galileo devices, false otherwise
    should_trace: bool,
    process_manager: ProcessManager,
}

impl SteamOSManager {
    pub async fn new(connection: Connection) -> Result<Self> {
        Ok(SteamOSManager {
            fan_control: FanControl::new(connection.clone()),
            wifi_debug_mode: WifiDebugMode::Off,
            should_trace: variant().await? == HardwareVariant::Galileo,
            process_manager: ProcessManager::new(connection.clone()),
            connection,
        })
    }
}

const ALS_INTEGRATION_PATH: &str = "/sys/devices/platform/AMDI0010:00/i2c-0/i2c-PRP0001:01/iio:device0/in_illuminance_integration_time";

#[interface(name = "com.steampowered.SteamOSManager1.RootManager")]
impl SteamOSManager {
    async fn prepare_factory_reset(&self) -> u32 {
        // Run steamos factory reset script and return true on success
        let res = run_script("/usr/bin/steamos-factory-reset-config", &[""]).await;
        match res {
            Ok(_) => PrepareFactoryReset::RebootRequired as u32,
            Err(_) => PrepareFactoryReset::Unknown as u32,
        }
    }

    async fn set_wifi_power_management_state(&self, state: u32) -> fdo::Result<()> {
        let state = match WifiPowerManagement::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(to_zbus_fdo_error(err)),
        };
        set_wifi_power_management_state(state)
            .await
            .map_err(to_zbus_fdo_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn fan_control_state(&self) -> fdo::Result<u32> {
        Ok(self
            .fan_control
            .get_state()
            .await
            .map_err(to_zbus_fdo_error)? as u32)
    }

    #[zbus(property)]
    async fn set_fan_control_state(&self, state: u32) -> zbus::Result<()> {
        let state = match FanControlState::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        self.fan_control
            .set_state(state)
            .await
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
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

    async fn get_als_integration_time_file_descriptor(&self) -> fdo::Result<Fd> {
        // Get the file descriptor for the als integration time sysfs path
        let result = File::create(ALS_INTEGRATION_PATH).await;
        match result {
            Ok(f) => Ok(Fd::Owned(std::os::fd::OwnedFd::from(f.into_std().await))),
            Err(message) => {
                error!("Error opening sysfs file for giving file descriptor: {message}");
                Err(fdo::Error::IOError(message.to_string()))
            }
        }
    }

    async fn update_bios(&mut self) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        // Update the bios as needed
        self.process_manager
            .get_command_object_path("/usr/bin/jupiter-biosupdate", &["--auto"], "updating BIOS")
            .await
    }

    async fn update_dock(&mut self) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        // Update the dock firmware as needed
        self.process_manager
            .get_command_object_path(
                "/usr/lib/jupiter-dock-updater/jupiter-dock-updater.sh",
                &[] as &[String; 0],
                "updating dock",
            )
            .await
    }

    async fn trim_devices(&mut self) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        // Run steamos-trim-devices script
        self.process_manager
            .get_command_object_path(
                "/usr/lib/hwsupport/trim-devices.sh",
                &[] as &[String; 0],
                "trimming devices",
            )
            .await
    }

    async fn format_device(
        &mut self,
        device: &str,
        label: &str,
        validate: bool,
    ) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        let mut args = vec!["--label", label, "--device", device];
        if !validate {
            args.push("--skip-validation");
        }
        self.process_manager
            .get_command_object_path(
                "/usr/lib/hwsupport/format-device.sh",
                args.as_ref(),
                format!("formatting {device}").as_str(),
            )
            .await
    }

    async fn set_gpu_performance_level(&self, level: u32) -> fdo::Result<()> {
        let level = match GPUPerformanceLevel::try_from(level) {
            Ok(level) => level,
            Err(e) => return Err(to_zbus_fdo_error(e)),
        };
        set_gpu_performance_level(level)
            .await
            .inspect_err(|message| error!("Error setting GPU performance level: {message}"))
            .map_err(to_zbus_fdo_error)
    }

    async fn set_manual_gpu_clock(&self, clocks: u32) -> fdo::Result<()> {
        set_gpu_clocks(clocks)
            .await
            .inspect_err(|message| error!("Error setting manual GPU clock: {message}"))
            .map_err(to_zbus_fdo_error)
    }

    async fn set_tdp_limit(&self, limit: u32) -> fdo::Result<()> {
        set_tdp_limit(limit).await.map_err(to_zbus_fdo_error)
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
        #[zbus(signal_context)] ctx: SignalContext<'_>,
    ) -> fdo::Result<()> {
        // Set the wifi debug mode to mode, using an int for flexibility going forward but only
        // doing things on 0 or 1 for now
        let wanted_mode = match WifiDebugMode::try_from(mode) {
            Ok(mode) => mode,
            Err(e) => return Err(fdo::Error::InvalidArgs(e.to_string())),
        };
        match set_wifi_debug_mode(
            wanted_mode,
            buffer_size,
            self.should_trace,
            self.connection.clone(),
        )
        .await
        {
            Ok(()) => {
                self.wifi_debug_mode = wanted_mode;
                self.wifi_debug_mode_state_changed(&ctx).await?;
                Ok(())
            }
            Err(e) => {
                error!("Error setting wifi debug mode: {e}");
                Err(to_zbus_fdo_error(e))
            }
        }
    }

    async fn set_wifi_backend(&mut self, backend: u32) -> fdo::Result<()> {
        if self.wifi_debug_mode == WifiDebugMode::On {
            return Err(fdo::Error::Failed(String::from(
                "operation not supported when wifi_debug_mode=on",
            )));
        }
        let backend = match WifiBackend::try_from(backend) {
            Ok(backend) => backend,
            Err(e) => return Err(fdo::Error::InvalidArgs(e.to_string())),
        };
        set_wifi_backend(backend)
            .await
            .inspect_err(|message| error!("Error setting wifi backend: {message}"))
            .map_err(to_zbus_fdo_error)
    }

    /// A version property.
    #[zbus(property(emits_changed_signal = "const"))]
    async fn version(&self) -> u32 {
        API_VERSION
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::power::test::{format_clocks, read_clocks};
    use crate::power::{self, get_gpu_performance_level};
    use crate::testing;
    use tokio::fs::{create_dir_all, write};
    use zbus::{Connection, ConnectionBuilder};

    struct TestHandle {
        _handle: testing::TestHandle,
        connection: Connection,
    }

    async fn start(name: &str) -> TestHandle {
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
        write(
            crate::path("/etc/NetworkManager/conf.d/wifi_backend.conf"),
            "wifi.backend=iwd\n",
        )
        .await
        .expect("write");

        let connection = ConnectionBuilder::session()
            .unwrap()
            .name(format!("com.steampowered.SteamOSManager1.Test.{name}"))
            .unwrap()
            .build()
            .await
            .unwrap();
        let manager = SteamOSManager::new(connection.clone()).await.unwrap();
        connection
            .object_server()
            .at("/com/steampowered/SteamOSManager1", manager)
            .await
            .expect("object_server at");

        TestHandle {
            _handle: handle,
            connection,
        }
    }

    #[zbus::proxy(
        interface = "com.steampowered.SteamOSManager1.RootManager",
        default_service = "com.steampowered.SteamOSManager1.Test.GpuPerformanceLevel",
        default_path = "/com/steampowered/SteamOSManager1"
    )]
    trait GpuPerformanceLevel {
        fn set_gpu_performance_level(&self, level: u32) -> zbus::Result<()>;
    }

    #[tokio::test]
    async fn gpu_performance_level() {
        let test = start("GpuPerformanceLevel").await;
        power::test::setup().await;

        let proxy = GpuPerformanceLevelProxy::new(&test.connection)
            .await
            .unwrap();
        proxy
            .set_gpu_performance_level(GPUPerformanceLevel::Low as u32)
            .await
            .expect("proxy_set");
        assert_eq!(
            get_gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::Low
        );
    }

    #[zbus::proxy(
        interface = "com.steampowered.SteamOSManager1.RootManager",
        default_service = "com.steampowered.SteamOSManager1.Test.ManualGpuClock",
        default_path = "/com/steampowered/SteamOSManager1"
    )]
    trait ManualGpuClock {
        fn set_manual_gpu_clock(&self, clocks: u32) -> zbus::Result<()>;
    }

    #[tokio::test]
    async fn manual_gpu_clock() {
        let test = start("ManualGpuClock").await;

        let proxy = ManualGpuClockProxy::new(&test.connection).await.unwrap();

        power::test::setup().await;
        proxy.set_manual_gpu_clock(200).await.expect("proxy_set");
        assert_eq!(read_clocks().await.unwrap(), format_clocks(200));

        assert!(proxy.set_manual_gpu_clock(100).await.is_err());
        assert_eq!(read_clocks().await.unwrap(), format_clocks(200));
    }

    #[zbus::proxy(
        interface = "com.steampowered.SteamOSManager1.RootManager",
        default_service = "com.steampowered.SteamOSManager1.Test.Version",
        default_path = "/com/steampowered/SteamOSManager1"
    )]
    trait Version {
        #[zbus(property)]
        fn version(&self) -> zbus::Result<u32>;
    }

    #[tokio::test]
    async fn version() {
        let test = start("Version").await;
        let proxy = VersionProxy::new(&test.connection).await.unwrap();
        assert_eq!(proxy.version().await, Ok(API_VERSION));
    }
}
