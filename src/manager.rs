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
use zbus::{interface, Connection, SignalContext};

use crate::hardware::{check_support, variant, FanControl, FanControlState, HardwareVariant};
use crate::power::{
    get_gpu_clocks, get_gpu_performance_level, get_tdp_limit, set_gpu_clocks,
    set_gpu_performance_level, set_tdp_limit, GPUPerformanceLevel,
};
use crate::process::{run_script, script_output};
use crate::wifi::{
    get_wifi_backend, get_wifi_power_management_state, set_wifi_backend, set_wifi_debug_mode,
    set_wifi_power_management_state, WifiBackend, WifiDebugMode, WifiPowerManagement,
};
use crate::{anyhow_to_zbus, anyhow_to_zbus_fdo};

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
}

impl SteamOSManager {
    pub async fn new(connection: Connection) -> Result<Self> {
        Ok(SteamOSManager {
            fan_control: FanControl::new(connection.clone()),
            connection,
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
        let res = run_script("/usr/bin/steamos-factory-reset-config", &[""]).await;
        match res {
            Ok(_) => PrepareFactoryReset::RebootRequired as u32,
            Err(_) => PrepareFactoryReset::Unknown as u32,
        }
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn wifi_power_management_state(&self) -> zbus::fdo::Result<u32> {
        match get_wifi_power_management_state().await {
            Ok(state) => Ok(state as u32),
            Err(e) => Err(anyhow_to_zbus_fdo(e)),
        }
    }

    #[zbus(property)]
    async fn set_wifi_power_management_state(&self, state: u32) -> zbus::Result<()> {
        let state = match WifiPowerManagement::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(zbus::fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        set_wifi_power_management_state(state)
            .await
            .map_err(anyhow_to_zbus)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn fan_control_state(&self) -> zbus::fdo::Result<u32> {
        Ok(self
            .fan_control
            .get_state()
            .await
            .map_err(anyhow_to_zbus_fdo)? as u32)
    }

    #[zbus(property)]
    async fn set_fan_control_state(&self, state: u32) -> zbus::Result<()> {
        let state = match FanControlState::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(zbus::fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        self.fan_control
            .set_state(state)
            .await
            .map_err(anyhow_to_zbus)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn hardware_currently_supported(&self) -> zbus::fdo::Result<u32> {
        match check_support().await {
            Ok(res) => Ok(res as u32),
            Err(e) => Err(anyhow_to_zbus_fdo(e)),
        }
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

    async fn get_als_integration_time_file_descriptor(&self) -> zbus::fdo::Result<Fd> {
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

    async fn update_bios(&self) -> zbus::fdo::Result<()> {
        // Update the bios as needed
        run_script("/usr/bin/jupiter-biosupdate", &["--auto"])
            .await
            .inspect_err(|message| error!("Error updating BIOS: {message}"))
            .map_err(anyhow_to_zbus_fdo)
    }

    async fn update_dock(&self) -> zbus::fdo::Result<()> {
        // Update the dock firmware as needed
        run_script(
            "/usr/lib/jupiter-dock-updater/jupiter-dock-updater.sh",
            &[] as &[String; 0],
        )
        .await
        .inspect_err(|message| error!("Error updating dock: {message}"))
        .map_err(anyhow_to_zbus_fdo)
    }

    async fn trim_devices(&self) -> zbus::fdo::Result<()> {
        // Run steamos-trim-devices script
        run_script("/usr/lib/hwsupport/trim-devices.sh", &[] as &[String; 0])
            .await
            .inspect_err(|message| error!("Error updating trimming devices: {message}"))
            .map_err(anyhow_to_zbus_fdo)
    }

    async fn format_device(
        &self,
        device: &str,
        label: &str,
        validate: bool,
    ) -> zbus::fdo::Result<()> {
        let mut args = vec!["--label", label, "--device", device];
        if !validate {
            args.push("--skip-validation");
        }
        run_script("/usr/lib/hwsupport/format-device.sh", args.as_ref())
            .await
            .inspect_err(|message| error!("Error formatting {device}: {message}"))
            .map_err(anyhow_to_zbus_fdo)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn gpu_performance_level(&self) -> zbus::fdo::Result<u32> {
        match get_gpu_performance_level().await {
            Ok(level) => Ok(level as u32),
            Err(e) => {
                error!("Error getting GPU performance level: {e}");
                Err(anyhow_to_zbus_fdo(e))
            }
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
            .inspect_err(|message| error!("Error setting GPU performance level: {message}"))
            .map_err(anyhow_to_zbus)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn manual_gpu_clock(&self) -> zbus::fdo::Result<u32> {
        get_gpu_clocks()
            .await
            .inspect_err(|message| error!("Error getting manual GPU clock: {message}"))
            .map_err(anyhow_to_zbus_fdo)
    }

    #[zbus(property)]
    async fn set_manual_gpu_clock(&self, clocks: u32) -> zbus::Result<()> {
        set_gpu_clocks(clocks)
            .await
            .inspect_err(|message| error!("Error setting manual GPU clock: {message}"))
            .map_err(anyhow_to_zbus)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn manual_gpu_clock_min(&self) -> u32 {
        // TODO: Can this be queried from somewhere?
        200
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn manual_gpu_clock_max(&self) -> u32 {
        // TODO: Can this be queried from somewhere?
        1600
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn tdp_limit(&self) -> zbus::fdo::Result<u32> {
        get_tdp_limit().await.map_err(anyhow_to_zbus_fdo)
    }

    #[zbus(property)]
    async fn set_tdp_limit(&self, limit: u32) -> zbus::Result<()> {
        set_tdp_limit(limit).await.map_err(anyhow_to_zbus)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn tdp_limit_min(&self) -> u32 {
        // TODO: Can this be queried from somewhere?
        3
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn tdp_limit_max(&self) -> u32 {
        // TODO: Can this be queried from somewhere?
        15
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
    ) -> zbus::fdo::Result<()> {
        // Set the wifi debug mode to mode, using an int for flexibility going forward but only
        // doing things on 0 or 1 for now
        let wanted_mode = match WifiDebugMode::try_from(mode) {
            Ok(mode) => mode,
            Err(e) => return Err(zbus::fdo::Error::InvalidArgs(e.to_string())),
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
                Err(anyhow_to_zbus_fdo(e))
            }
        }
    }

    /// WifiBackend property.
    #[zbus(property(emits_changed_signal = "false"))]
    async fn wifi_backend(&self) -> zbus::fdo::Result<u32> {
        match get_wifi_backend().await {
            Ok(backend) => Ok(backend as u32),
            Err(e) => Err(anyhow_to_zbus_fdo(e)),
        }
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
        set_wifi_backend(backend)
            .await
            .inspect_err(|message| error!("Error setting wifi backend: {message}"))
            .map_err(anyhow_to_zbus_fdo)
    }

    /// A version property.
    #[zbus(property(emits_changed_signal = "const"))]
    async fn version(&self) -> u32 {
        SteamOSManager::API_VERSION
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{power, testing};
    use tokio::fs::{create_dir_all, write};
    use zbus::connection::Connection;
    use zbus::ConnectionBuilder;

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
        interface = "com.steampowered.SteamOSManager1.Manager",
        default_service = "com.steampowered.SteamOSManager1.Test.GpuPerformanceLevel",
        default_path = "/com/steampowered/SteamOSManager1"
    )]
    trait GpuPerformanceLevel {
        #[zbus(property)]
        fn gpu_performance_level(&self) -> zbus::Result<u32>;

        #[zbus(property)]
        fn set_gpu_performance_level(&self, level: u32) -> zbus::Result<()>;
    }

    #[tokio::test]
    async fn gpu_performance_level() {
        let test = start("GpuPerformanceLevel").await;
        power::test::setup().await;

        let proxy = GpuPerformanceLevelProxy::new(&test.connection)
            .await
            .unwrap();
        set_gpu_performance_level(GPUPerformanceLevel::Auto)
            .await
            .expect("set");
        assert_eq!(
            proxy.gpu_performance_level().await.unwrap(),
            GPUPerformanceLevel::Auto as u32
        );

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
        interface = "com.steampowered.SteamOSManager1.Manager",
        default_service = "com.steampowered.SteamOSManager1.Test.ManualGpuClock",
        default_path = "/com/steampowered/SteamOSManager1"
    )]
    trait ManualGpuClock {
        #[zbus(property)]
        fn manual_gpu_clock(&self) -> zbus::Result<u32>;

        #[zbus(property)]
        fn set_manual_gpu_clock(&self, clocks: u32) -> zbus::Result<()>;
    }

    #[tokio::test]
    async fn manual_gpu_clock() {
        let test = start("ManualGpuClock").await;

        let proxy = ManualGpuClockProxy::new(&test.connection).await.unwrap();

        assert!(proxy.manual_gpu_clock().await.is_err());

        power::test::write_clocks(1600).await;
        assert_eq!(proxy.manual_gpu_clock().await.unwrap(), 1600);

        proxy.set_manual_gpu_clock(200).await.expect("proxy_set");
        power::test::expect_clocks(200).await;

        assert!(proxy.set_manual_gpu_clock(100).await.is_err());
        power::test::expect_clocks(200).await;
    }

    #[zbus::proxy(
        interface = "com.steampowered.SteamOSManager1.Manager",
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
        assert_eq!(proxy.version().await, Ok(SteamOSManager::API_VERSION));
    }
}
