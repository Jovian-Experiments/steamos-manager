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
use crate::process::{run_script, script_output, ProcessManager};
use crate::wifi::{
    get_wifi_backend, get_wifi_power_management_state, set_wifi_backend, set_wifi_debug_mode,
    set_wifi_power_management_state, WifiBackend, WifiDebugMode, WifiPowerManagement,
};
use crate::{to_zbus_error, to_zbus_fdo_error, API_VERSION};

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

#[interface(name = "com.steampowered.SteamOSManager1.Manager")]
impl SteamOSManager {
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
            Err(e) => Err(to_zbus_fdo_error(e)),
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
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn fan_control_state(&self) -> zbus::fdo::Result<u32> {
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
            Err(err) => return Err(zbus::fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        self.fan_control
            .set_state(state)
            .await
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn hardware_currently_supported(&self) -> zbus::fdo::Result<u32> {
        match check_support().await {
            Ok(res) => Ok(res as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
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

    async fn update_bios(&mut self) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        // Update the bios as needed
        self.process_manager
            .get_command_object_path("/usr/bin/jupiter-biosupdate", &["--auto"], "updating BIOS")
            .await
    }

    async fn update_dock(&mut self) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        // Update the dock firmware as needed
        self.process_manager
            .get_command_object_path(
                "/usr/lib/jupiter-dock-updater/jupiter-dock-updater.sh",
                &[] as &[String; 0],
                "updating dock",
            )
            .await
    }

    async fn trim_devices(&mut self) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
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
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
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

    #[zbus(property(emits_changed_signal = "false"))]
    async fn gpu_performance_level(&self) -> zbus::fdo::Result<u32> {
        match get_gpu_performance_level().await {
            Ok(level) => Ok(level as u32),
            Err(e) => {
                error!("Error getting GPU performance level: {e}");
                Err(to_zbus_fdo_error(e))
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
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn manual_gpu_clock(&self) -> zbus::fdo::Result<u32> {
        get_gpu_clocks()
            .await
            .inspect_err(|message| error!("Error getting manual GPU clock: {message}"))
            .map_err(to_zbus_fdo_error)
    }

    #[zbus(property)]
    async fn set_manual_gpu_clock(&self, clocks: u32) -> zbus::Result<()> {
        set_gpu_clocks(clocks)
            .await
            .inspect_err(|message| error!("Error setting manual GPU clock: {message}"))
            .map_err(to_zbus_error)
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
        get_tdp_limit().await.map_err(to_zbus_fdo_error)
    }

    #[zbus(property)]
    async fn set_tdp_limit(&self, limit: u32) -> zbus::Result<()> {
        set_tdp_limit(limit).await.map_err(to_zbus_error)
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
                Err(to_zbus_fdo_error(e))
            }
        }
    }

    /// WifiBackend property.
    #[zbus(property(emits_changed_signal = "false"))]
    async fn wifi_backend(&self) -> zbus::fdo::Result<u32> {
        match get_wifi_backend().await {
            Ok(backend) => Ok(backend as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
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
    use crate::{power, testing};
    use std::collections::HashMap;
    use std::iter::zip;
    use tokio::fs::{create_dir_all, read, write};
    use zbus::{Connection, ConnectionBuilder, Interface};
    use zbus_xml::{Method, Node, Property};

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

        power::test::setup().await;
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
        assert_eq!(proxy.version().await, Ok(API_VERSION));
    }

    fn collect_methods<'a>(methods: &'a [Method<'a>]) -> HashMap<String, &'a Method<'a>> {
        let mut map = HashMap::new();
        for method in methods.iter() {
            map.insert(method.name().to_string(), method);
        }
        map
    }

    fn collect_properties<'a>(props: &'a [Property<'a>]) -> HashMap<String, &'a Property<'a>> {
        let mut map = HashMap::new();
        for prop in props.iter() {
            map.insert(prop.name().to_string(), prop);
        }
        map
    }

    #[tokio::test]
    async fn interface_matches() {
        let test = start("Interface").await;

        let manager_ref = test
            .connection
            .object_server()
            .interface::<_, SteamOSManager>("/com/steampowered/SteamOSManager1")
            .await
            .expect("interface");
        let manager = manager_ref.get().await;
        let mut remote_interface_string = String::from(
            "<node name=\"/\" xmlns:doc=\"http://www.freedesktop.org/dbus/1.0/doc.dtd\">",
        );
        manager.introspect_to_writer(&mut remote_interface_string, 0);
        remote_interface_string.push_str("</node>");
        let remote_interfaces =
            Node::from_reader::<&[u8]>(remote_interface_string.as_bytes()).expect("from_reader");
        let remote_interface: Vec<_> = remote_interfaces
            .interfaces()
            .iter()
            .filter(|iface| iface.name() == "com.steampowered.SteamOSManager1.Manager")
            .collect();
        assert_eq!(remote_interface.len(), 1);
        let remote_interface = remote_interface[0];
        let remote_methods = collect_methods(remote_interface.methods());
        let remote_properties = collect_properties(remote_interface.properties());

        let local_interface_string = read("com.steampowered.SteamOSManager1.xml")
            .await
            .expect("read");
        let local_interfaces =
            Node::from_reader::<&[u8]>(local_interface_string.as_ref()).expect("from_reader");
        let local_interface: Vec<_> = local_interfaces
            .interfaces()
            .iter()
            .filter(|iface| iface.name() == "com.steampowered.SteamOSManager1.Manager")
            .collect();
        assert_eq!(local_interface.len(), 1);
        let local_interface = local_interface[0];
        let local_methods = collect_methods(local_interface.methods());
        let local_properties = collect_properties(local_interface.properties());

        for key in remote_methods.keys() {
            let local_method = local_methods.get(key).expect(key);
            let remote_method = remote_methods.get(key).expect(key);

            assert_eq!(local_method.name(), remote_method.name());
            assert_eq!(local_method.args().len(), remote_method.args().len());
            for (local_arg, remote_arg) in
                zip(local_method.args().iter(), remote_method.args().iter())
            {
                assert_eq!(local_arg.direction(), remote_arg.direction());
                assert_eq!(local_arg.ty(), remote_arg.ty());
            }
        }

        for key in remote_properties.keys() {
            let local_property = local_properties.get(key).expect(key);
            let remote_property = remote_properties.get(key).expect(key);

            assert_eq!(local_property.name(), remote_property.name());
            assert_eq!(local_property.ty(), remote_property.ty());
            assert_eq!(local_property.access(), remote_property.access());
        }
    }
}
