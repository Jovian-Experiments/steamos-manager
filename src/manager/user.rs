/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 * Copyright © 2024 Igalia S.L.
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::mpsc::{Sender, UnboundedSender};
use tokio::sync::oneshot;
use tracing::error;
use zbus::proxy::Builder;
use zbus::zvariant::{self, Fd};
use zbus::{fdo, interface, CacheProperties, Connection, Proxy, SignalContext};

use crate::cec::{HdmiCecControl, HdmiCecState};
use crate::daemon::user::Command;
use crate::daemon::DaemonCommand;
use crate::error::{to_zbus_error, to_zbus_fdo_error, zbus_to_zbus_fdo};
use crate::hardware::{check_support, HardwareCurrentlySupported};
use crate::job::JobManagerCommand;
use crate::power::{
    get_available_cpu_scaling_governors, get_available_gpu_performance_levels,
    get_available_gpu_power_profiles, get_cpu_scaling_governor, get_gpu_clocks,
    get_gpu_clocks_range, get_gpu_performance_level, get_gpu_power_profile, get_tdp_limit,
};
use crate::wifi::{get_wifi_backend, get_wifi_power_management_state};
use crate::API_VERSION;

const MANAGER_PATH: &str = "/com/steampowered/SteamOSManager1";

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

macro_rules! job_method {
    ($self:expr, $method:expr, $($args:expr),+) => {
        {
            let (tx, rx) = oneshot::channel();
            $self.job_manager.send(JobManagerCommand::MirrorJob {
                connection: $self.proxy.connection().clone(),
                path: method!($self, $method, $($args),+)?,
                reply: tx,
            }).map_err(to_zbus_fdo_error)?;
            rx.await.map_err(to_zbus_fdo_error)?
        }
    };
    ($self:expr, $method:expr) => {
        {
            let (tx, rx) = oneshot::channel();
            $self.job_manager.send(JobManagerCommand::MirrorJob {
                connection: $self.proxy.connection().clone(),
                path: method!($self, $method)?,
                reply: tx,
            }).map_err(to_zbus_fdo_error)?;
            rx.await.map_err(to_zbus_fdo_error)?
        }
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

struct SteamOSManager {
    proxy: Proxy<'static>,
}

struct AmbientLightSensor1 {
    proxy: Proxy<'static>,
}

struct CpuScaling1 {
    proxy: Proxy<'static>,
}

struct FactoryReset1 {
    proxy: Proxy<'static>,
}

struct FanControl1 {
    proxy: Proxy<'static>,
}

struct GpuPerformanceLevel1 {
    proxy: Proxy<'static>,
}

struct GpuPowerProfile1 {
    proxy: Proxy<'static>,
}

struct GpuTdpLimit1 {
    proxy: Proxy<'static>,
}

struct HdmiCec1 {
    hdmi_cec: HdmiCecControl<'static>,
}

struct Manager2 {
    proxy: Proxy<'static>,
    channel: Sender<Command>,
}

struct Storage1 {
    proxy: Proxy<'static>,
    job_manager: UnboundedSender<JobManagerCommand>,
}

struct UpdateBios1 {
    proxy: Proxy<'static>,
    job_manager: UnboundedSender<JobManagerCommand>,
}

struct UpdateDock1 {
    proxy: Proxy<'static>,
    job_manager: UnboundedSender<JobManagerCommand>,
}

struct WifiDebug1 {
    proxy: Proxy<'static>,
}

struct WifiPowerManagement1 {
    proxy: Proxy<'static>,
}

impl SteamOSManager {
    pub async fn new(
        system_conn: Connection,
        proxy: Proxy<'static>,
        job_manager: UnboundedSender<JobManagerCommand>,
    ) -> Result<Self> {
        job_manager.send(JobManagerCommand::MirrorConnection(system_conn))?;
        Ok(SteamOSManager { proxy })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.Manager")]
impl SteamOSManager {
    #[zbus(property(emits_changed_signal = "const"))]
    async fn version(&self) -> u32 {
        API_VERSION
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn tdp_limit_min(&self) -> u32 {
        0
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
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.AmbientLightSensor1")]
impl AmbientLightSensor1 {
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
}

#[interface(name = "com.steampowered.SteamOSManager1.CpuScaling1")]
impl CpuScaling1 {
    #[zbus(property(emits_changed_signal = "false"))]
    async fn available_cpu_scaling_governors(&self) -> fdo::Result<Vec<String>> {
        let governors = get_available_cpu_scaling_governors()
            .await
            .map_err(to_zbus_fdo_error)?;
        let mut result = Vec::new();
        for g in governors {
            result.push(g.to_string());
        }
        Ok(result)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn cpu_scaling_governor(&self) -> fdo::Result<String> {
        let governor = get_cpu_scaling_governor()
            .await
            .map_err(to_zbus_fdo_error)?;
        Ok(governor.to_string())
    }

    #[zbus(property)]
    async fn set_cpu_scaling_governor(&self, governor: String) -> zbus::Result<()> {
        self.proxy
            .call("SetCpuScalingGovernor", &(governor))
            .await
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.FactoryReset1")]
impl FactoryReset1 {
    async fn prepare_factory_reset(&self) -> fdo::Result<u32> {
        method!(self, "PrepareFactoryReset")
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.FanControl1")]
impl FanControl1 {
    #[zbus(property(emits_changed_signal = "false"))]
    async fn fan_control_state(&self) -> fdo::Result<u32> {
        getter!(self, "FanControlState")
    }

    #[zbus(property)]
    async fn set_fan_control_state(&self, state: u32) -> zbus::Result<()> {
        setter!(self, "FanControlState", state)
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.GpuPerformanceLevel1")]
impl GpuPerformanceLevel1 {
    #[zbus(property(emits_changed_signal = "false"))]
    async fn available_gpu_performance_levels(&self) -> fdo::Result<Vec<String>> {
        get_available_gpu_performance_levels()
            .await
            .inspect_err(|message| error!("Error getting GPU performance levels: {message}"))
            .map(|levels| levels.into_iter().map(|level| level.to_string()).collect())
            .map_err(to_zbus_fdo_error)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn gpu_performance_level(&self) -> fdo::Result<String> {
        match get_gpu_performance_level().await {
            Ok(level) => Ok(level.to_string()),
            Err(e) => {
                error!("Error getting GPU performance level: {e}");
                Err(to_zbus_fdo_error(e))
            }
        }
    }

    #[zbus(property)]
    async fn set_gpu_performance_level(&self, level: &str) -> zbus::Result<()> {
        self.proxy
            .call("SetGpuPerformanceLevel", &(level))
            .await
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
        self.proxy
            .call("SetManualGpuClock", &(clocks))
            .await
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn manual_gpu_clock_min(&self) -> fdo::Result<u32> {
        Ok(get_gpu_clocks_range().await.map_err(to_zbus_fdo_error)?.0)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    async fn manual_gpu_clock_max(&self) -> fdo::Result<u32> {
        Ok(get_gpu_clocks_range().await.map_err(to_zbus_fdo_error)?.1)
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.GpuPowerProfile1")]
impl GpuPowerProfile1 {
    #[zbus(property(emits_changed_signal = "false"))]
    async fn available_gpu_power_profiles(&self) -> fdo::Result<Vec<String>> {
        let (_, names): (Vec<u32>, Vec<String>) = get_available_gpu_power_profiles()
            .await
            .map_err(to_zbus_fdo_error)?
            .into_iter()
            .unzip();
        Ok(names)
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn gpu_power_profile(&self) -> fdo::Result<String> {
        match get_gpu_power_profile().await {
            Ok(profile) => Ok(profile.to_string()),
            Err(e) => {
                error!("Error getting GPU power profile: {e}");
                Err(to_zbus_fdo_error(e))
            }
        }
    }

    #[zbus(property)]
    async fn set_gpu_power_profile(&self, profile: &str) -> zbus::Result<()> {
        self.proxy
            .call("SetGpuPowerProfile", &(profile))
            .await
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.GpuTdpLimit1")]
impl GpuTdpLimit1 {
    #[zbus(property(emits_changed_signal = "false"))]
    async fn tdp_limit(&self) -> fdo::Result<u32> {
        get_tdp_limit().await.map_err(to_zbus_fdo_error)
    }

    #[zbus(property)]
    async fn set_tdp_limit(&self, limit: u32) -> zbus::Result<()> {
        self.proxy
            .call("SetTdpLimit", &(limit))
            .await
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
}

impl HdmiCec1 {
    async fn new(connection: &Connection) -> Result<HdmiCec1> {
        let hdmi_cec = HdmiCecControl::new(connection).await?;
        Ok(HdmiCec1 { hdmi_cec })
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.HdmiCec1")]
impl HdmiCec1 {
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
}

#[interface(name = "com.steampowered.SteamOSManager1.Manager2")]
impl Manager2 {
    #[zbus(property(emits_changed_signal = "const"))]
    async fn hardware_currently_supported(&self) -> u32 {
        match check_support().await {
            Ok(res) => res as u32,
            Err(_) => HardwareCurrentlySupported::Unknown as u32,
        }
    }

    async fn reload_config(&self) -> fdo::Result<()> {
        self.channel
            .send(DaemonCommand::ReadConfig)
            .await
            .inspect_err(|message| error!("Error sending ReadConfig command: {message}"))
            .map_err(to_zbus_fdo_error)?;
        method!(self, "ReloadConfig")
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.Storage1")]
impl Storage1 {
    async fn format_device(
        &mut self,
        device: &str,
        label: &str,
        validate: bool,
    ) -> fdo::Result<zvariant::OwnedObjectPath> {
        job_method!(self, "FormatDevice", device, label, validate)
    }

    async fn trim_devices(&mut self) -> fdo::Result<zvariant::OwnedObjectPath> {
        job_method!(self, "TrimDevices")
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.UpdateBios1")]
impl UpdateBios1 {
    async fn update_bios(&mut self) -> fdo::Result<zvariant::OwnedObjectPath> {
        job_method!(self, "UpdateBios")
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.UpdateDock1")]
impl UpdateDock1 {
    async fn update_dock(&mut self) -> fdo::Result<zvariant::OwnedObjectPath> {
        job_method!(self, "UpdateDock")
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.WifiDebug1")]
impl WifiDebug1 {
    #[zbus(property)]
    async fn wifi_debug_mode_state(&self) -> fdo::Result<u32> {
        getter!(self, "WifiDebugModeState")
    }

    async fn set_wifi_debug_mode(
        &self,
        mode: u32,
        options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(signal_context)] ctx: SignalContext<'_>,
    ) -> fdo::Result<()> {
        method!(self, "SetWifiDebugMode", mode, options)?;
        self.wifi_debug_mode_state_changed(&ctx)
            .await
            .map_err(zbus_to_zbus_fdo)?;
        Ok(())
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn wifi_backend(&self) -> fdo::Result<String> {
        match get_wifi_backend().await {
            Ok(backend) => Ok(backend.to_string()),
            Err(e) => Err(to_zbus_fdo_error(e)),
        }
    }

    #[zbus(property)]
    async fn set_wifi_backend(&self, backend: &str) -> zbus::Result<()> {
        self.proxy
            .call("SetWifiBackend", &(backend))
            .await
    }
}

#[interface(name = "com.steampowered.SteamOSManager1.WifiPowerManagement1")]
impl WifiPowerManagement1 {
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
    }
}

pub(crate) async fn create_interfaces(
    session: Connection,
    system: Connection,
    daemon: Sender<Command>,
    job_manager: UnboundedSender<JobManagerCommand>,
) -> Result<()> {
    let proxy = Builder::<Proxy>::new(&system)
        .destination("com.steampowered.SteamOSManager1")?
        .path("/com/steampowered/SteamOSManager1")?
        .interface("com.steampowered.SteamOSManager1.RootManager")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    let manager = SteamOSManager::new(system.clone(), proxy.clone(), job_manager.clone()).await?;

    let als = AmbientLightSensor1 {
        proxy: proxy.clone(),
    };
    let cpu_scaling = CpuScaling1 {
        proxy: proxy.clone(),
    };
    let factory_reset = FactoryReset1 {
        proxy: proxy.clone(),
    };
    let fan_control = FanControl1 {
        proxy: proxy.clone(),
    };
    let gpu_performance_level = GpuPerformanceLevel1 {
        proxy: proxy.clone(),
    };
    let gpu_power_profile = GpuPowerProfile1 {
        proxy: proxy.clone(),
    };
    let gpu_tdp_limit = GpuTdpLimit1 {
        proxy: proxy.clone(),
    };
    let hdmi_cec = HdmiCec1::new(&session).await?;
    let manager2 = Manager2 {
        proxy: proxy.clone(),
        channel: daemon,
    };
    let storage = Storage1 {
        proxy: proxy.clone(),
        job_manager: job_manager.clone(),
    };
    let update_bios = UpdateBios1 {
        proxy: proxy.clone(),
        job_manager: job_manager.clone(),
    };
    let update_dock = UpdateDock1 {
        proxy: proxy.clone(),
        job_manager: job_manager.clone(),
    };
    let wifi_debug = WifiDebug1 {
        proxy: proxy.clone(),
    };
    let wifi_power_management = WifiPowerManagement1 {
        proxy: proxy.clone(),
    };

    let object_server = session.object_server();
    object_server.at(MANAGER_PATH, manager).await?;
    object_server.at(MANAGER_PATH, als).await?;
    object_server.at(MANAGER_PATH, cpu_scaling).await?;
    object_server.at(MANAGER_PATH, factory_reset).await?;
    object_server.at(MANAGER_PATH, fan_control).await?;
    object_server
        .at(MANAGER_PATH, gpu_performance_level)
        .await?;
    object_server.at(MANAGER_PATH, gpu_power_profile).await?;
    object_server.at(MANAGER_PATH, gpu_tdp_limit).await?;
    object_server.at(MANAGER_PATH, hdmi_cec).await?;
    object_server.at(MANAGER_PATH, manager2).await?;
    object_server.at(MANAGER_PATH, storage).await?;
    object_server.at(MANAGER_PATH, update_bios).await?;
    object_server.at(MANAGER_PATH, update_dock).await?;
    object_server.at(MANAGER_PATH, wifi_debug).await?;
    object_server
        .at(MANAGER_PATH, wifi_power_management)
        .await?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::daemon::channel;
    use crate::daemon::user::UserContext;
    use crate::testing;

    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::sleep;
    use zbus::{Connection, Interface};

    struct TestHandle {
        _handle: testing::TestHandle,
        connection: Connection,
    }

    async fn start() -> Result<TestHandle> {
        let mut handle = testing::start();
        let (tx_ctx, _rx_ctx) = channel::<UserContext>();
        let (tx_job, _rx_job) = unbounded_channel::<JobManagerCommand>();
        let connection = handle.new_dbus().await?;
        create_interfaces(connection.clone(), connection.clone(), tx_ctx, tx_job).await?;

        sleep(Duration::from_millis(1)).await;

        Ok(TestHandle {
            _handle: handle,
            connection,
        })
    }

    #[tokio::test]
    async fn interface_matches() {
        let test = start().await.expect("start");

        let remote = testing::InterfaceIntrospection::from_remote::<SteamOSManager, _>(
            &test.connection,
            MANAGER_PATH,
        )
        .await
        .expect("remote");
        let local = testing::InterfaceIntrospection::from_local(
            "com.steampowered.SteamOSManager1.Manager.xml",
            "com.steampowered.SteamOSManager1.Manager",
        )
        .await
        .expect("local");
        assert!(remote.compare(&local));
    }

    async fn test_interface_matches<I: Interface>(connection: &Connection) -> Result<bool> {
        let remote =
            testing::InterfaceIntrospection::from_remote::<I, _>(connection, MANAGER_PATH).await?;
        let local = testing::InterfaceIntrospection::from_local(
            "com.steampowered.SteamOSManager1.xml",
            I::name().to_string(),
        )
        .await?;
        Ok(remote.compare(&local))
    }

    #[tokio::test]
    async fn interface_matches_ambient_light_sensor1() {
        let test = start().await.expect("start");

        assert!(
            test_interface_matches::<AmbientLightSensor1>(&test.connection)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn interface_matches_cpu_scaling1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<CpuScaling1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_factory_reset1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<FactoryReset1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_fan_control1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<FanControl1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_gpu_performance_level1() {
        let test = start().await.expect("start");

        assert!(
            test_interface_matches::<GpuPerformanceLevel1>(&test.connection)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn interface_matches_gpu_power_profile1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<GpuPowerProfile1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_gpu_tdp_limit1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<GpuTdpLimit1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_hdmi_cec1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<HdmiCec1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_manager2() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<Manager2>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_storage1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<Storage1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_update_bios1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<UpdateBios1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_update_dock1() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<UpdateDock1>(&test.connection)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn interface_matches_wifi_power_management1() {
        let test = start().await.expect("start");

        assert!(
            test_interface_matches::<WifiPowerManagement1>(&test.connection)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn interface_matches_wifi_debug() {
        let test = start().await.expect("start");

        assert!(test_interface_matches::<WifiDebug1>(&test.connection)
            .await
            .unwrap());
    }
}
