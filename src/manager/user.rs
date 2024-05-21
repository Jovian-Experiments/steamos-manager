/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use tracing::error;
use zbus::proxy::Builder;
use zbus::zvariant::Fd;
use zbus::{fdo, interface, Connection, Proxy, SignalContext};

use crate::cec::{HdmiCecControl, HdmiCecState};
use crate::error::{to_zbus_error, to_zbus_fdo_error, zbus_to_zbus_fdo};
use crate::hardware::check_support;
use crate::power::{get_gpu_clocks, get_gpu_performance_level, get_tdp_limit};
use crate::wifi::{get_wifi_backend, get_wifi_power_management_state};
use crate::API_VERSION;

macro_rules! method {
    ($self:expr, $method:expr, $($args:expr),+) => {
        $self.proxy
            .call($method, &($($args,)*))
            .await
            .map_err(zbus_to_zbus_fdo)
    };
    ($self:expr, $method:expr) => {
        $self.proxy
            .call($method, &())
            .await
            .map_err(zbus_to_zbus_fdo)
    };
}

macro_rules! getter {
    ($self:expr, $prop:expr) => {
        $self
            .proxy
            .get_property($prop)
            .await
            .map_err(zbus_to_zbus_fdo)
    };
}

macro_rules! setter {
    ($self:expr, $prop:expr, $value:expr) => {
        $self
            .proxy
            .set_property($prop, $value)
            .await
            .map_err(|e| zbus::Error::FDO(Box::new(e)))
    };
}

pub struct SteamOSManager {
    proxy: Proxy<'static>,
    hdmi_cec: HdmiCecControl<'static>,
}

impl SteamOSManager {
    pub async fn new(connection: Connection, system_conn: &Connection) -> Result<Self> {
        Ok(SteamOSManager {
            hdmi_cec: HdmiCecControl::new(&connection).await?,
            proxy: Builder::new(system_conn)
                .destination("com.steampowered.SteamOSManager1")?
                .path("/com/steampowered/SteamOSManager1")?
                .interface("com.steampowered.SteamOSManager1.RootManager")?
                .cache_properties(zbus::CacheProperties::No)
                .build()
                .await?,
        })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.Manager")]
impl SteamOSManager {
    #[zbus(property(emits_changed_signal = "const"))]
    async fn version(&self) -> u32 {
        API_VERSION
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn hdmi_cec_state(&self) -> fdo::Result<u32> {
        match self.hdmi_cec.get_enabled_state().await {
            Ok(state) => Ok(state as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
        }
    }

    #[zbus(property)]
    async fn set_hdmi_cec_state(&self, state: u32) -> zbus::Result<()> {
        let state = match HdmiCecState::try_from(state) {
            Ok(state) => state,
            Err(err) => return Err(fdo::Error::InvalidArgs(err.to_string()).into()),
        };
        self.hdmi_cec
            .set_enabled_state(state)
            .await
            .inspect_err(|message| error!("Error setting CEC state: {message}"))
            .map_err(to_zbus_error)
    }

    async fn prepare_factory_reset(&self) -> fdo::Result<u32> {
        method!(self, "PrepareFactoryReset")
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn wifi_power_management_state(&self) -> fdo::Result<u32> {
        match get_wifi_power_management_state().await {
            Ok(state) => Ok(state as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
        }
    }

    #[zbus(property)]
    async fn set_wifi_power_management_state(&self, state: u32) -> zbus::Result<()> {
        self.proxy
            .call("SetWifiPowerManagementState", &(state))
            .await
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn fan_control_state(&self) -> fdo::Result<u32> {
        getter!(self, "FanControlState")
    }

    #[zbus(property)]
    async fn set_fan_control_state(&self, state: u32) -> zbus::Result<()> {
        setter!(self, "FanControlState", state)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn hardware_currently_supported(&self) -> fdo::Result<u32> {
        match check_support().await {
            Ok(res) => Ok(res as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
        }
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn als_calibration_gain(&self) -> fdo::Result<f64> {
        getter!(self, "AlsCalibrationGain")
    }

    async fn get_als_integration_time_file_descriptor(&self) -> fdo::Result<Fd> {
        let m = self
            .proxy
            .call_method::<&str, ()>("GetAlsIntegrationTimeFileDescriptor", &())
            .await
            .map_err(zbus_to_zbus_fdo)?;
        match m.body().deserialize::<Fd>() {
            Ok(fd) => fd.try_to_owned().map_err(to_zbus_fdo_error),
            Err(e) => Err(zbus_to_zbus_fdo(e)),
        }
    }

    async fn update_bios(&self) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        method!(self, "UpdateBios")
    }

    async fn update_dock(&self) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        method!(self, "UpdateDock")
    }

    async fn trim_devices(&self) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        method!(self, "TrimDevices")
    }

    async fn format_device(
        &self,
        device: &str,
        label: &str,
        validate: bool,
    ) -> fdo::Result<zbus::zvariant::OwnedObjectPath> {
        method!(self, "FormatDevice", device, label, validate)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn gpu_performance_level(&self) -> fdo::Result<u32> {
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
        self.proxy
            .call("SetGpuPerformanceLevel", &(level))
            .await
            .map_err(to_zbus_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn manual_gpu_clock(&self) -> fdo::Result<u32> {
        get_gpu_clocks()
            .await
            .inspect_err(|message| error!("Error getting manual GPU clock: {message}"))
            .map_err(to_zbus_fdo_error)
    }

    #[zbus(property)]
    async fn set_manual_gpu_clock(&self, clocks: u32) -> zbus::Result<()> {
        setter!(self, "SetManualGpuClock", clocks)
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
    async fn tdp_limit(&self) -> fdo::Result<u32> {
        get_tdp_limit().await.map_err(to_zbus_fdo_error)
    }

    #[zbus(property)]
    async fn set_tdp_limit(&self, limit: u32) -> zbus::Result<()> {
        self.proxy
            .call("SetTdpLimit", &(limit))
            .await
            .map_err(to_zbus_error)
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
    async fn wifi_debug_mode_state(&self) -> fdo::Result<u32> {
        getter!(self, "WifiDebugModeState")
    }

    async fn set_wifi_debug_mode(
        &self,
        mode: u32,
        buffer_size: u32,
        #[zbus(signal_context)] ctx: SignalContext<'_>,
    ) -> fdo::Result<()> {
        method!(self, "SetWifiDebugMode", mode, buffer_size)?;
        self.wifi_debug_mode_state_changed(&ctx)
            .await
            .map_err(zbus_to_zbus_fdo)?;
        Ok(())
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn wifi_backend(&self) -> fdo::Result<u32> {
        match get_wifi_backend().await {
            Ok(backend) => Ok(backend as u32),
            Err(e) => Err(to_zbus_fdo_error(e)),
        }
    }

    #[zbus(property)]
    async fn set_wifi_backend(&self, backend: u32) -> zbus::Result<()> {
        self.proxy
            .call("SetWifiBackend", &(backend))
            .await
            .map_err(to_zbus_error)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use std::collections::{HashMap, HashSet};
    use std::iter::zip;
    use tokio::fs::read;
    use zbus::{Connection, ConnectionBuilder, Interface};
    use zbus_xml::{Method, Node, Property};

    struct TestHandle {
        _handle: testing::TestHandle,
        connection: Connection,
    }

    async fn start(name: &str) -> TestHandle {
        let handle = testing::start();
        let connection = ConnectionBuilder::session()
            .unwrap()
            .name(format!("com.steampowered.SteamOSManager1.UserTest.{name}"))
            .unwrap()
            .build()
            .await
            .unwrap();
        let manager = SteamOSManager::new(connection.clone(), &connection)
            .await
            .unwrap();
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
        let remote_method_names: HashSet<&String> = remote_methods.keys().collect();
        let remote_properties = collect_properties(remote_interface.properties());
        let remote_property_names: HashSet<&String> = remote_properties.keys().collect();

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
        let local_method_names: HashSet<&String> = local_methods.keys().collect();
        let local_properties = collect_properties(local_interface.properties());
        let local_property_names: HashSet<&String> = local_properties.keys().collect();

        for key in local_method_names.union(&remote_method_names) {
            let local_method = local_methods.get(*key).expect(key);
            let remote_method = remote_methods.get(*key).expect(key);

            assert_eq!(local_method.name(), remote_method.name());
            assert_eq!(
                local_method.args().len(),
                remote_method.args().len(),
                "Testing {:?} against {:?}",
                local_method,
                remote_method
            );
            for (local_arg, remote_arg) in
                zip(local_method.args().iter(), remote_method.args().iter())
            {
                assert_eq!(local_arg.direction(), remote_arg.direction());
                assert_eq!(local_arg.ty(), remote_arg.ty());
            }
        }

        for key in local_property_names.union(&remote_property_names) {
            let local_property = local_properties.get(*key).expect(key);
            let remote_property = remote_properties.get(*key).expect(key);

            assert_eq!(local_property.name(), remote_property.name());
            assert_eq!(local_property.ty(), remote_property.ty());
            assert_eq!(local_property.access(), remote_property.access());
        }
    }
}
