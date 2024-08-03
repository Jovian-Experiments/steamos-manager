/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use clap::{Parser, Subcommand};
use itertools::Itertools;
use std::collections::HashMap;
use std::ops::Deref;
use steamos_manager::cec::HdmiCecState;
use steamos_manager::hardware::FanControlState;
use steamos_manager::power::{CPUScalingGovernor, GPUPerformanceLevel, GPUPowerProfile};
use steamos_manager::proxy::{
    AmbientLightSensor1Proxy, CpuScaling1Proxy, FactoryReset1Proxy, FanControl1Proxy,
    GpuPerformanceLevel1Proxy, GpuPowerProfile1Proxy, GpuTdpLimit1Proxy, HdmiCec1Proxy,
    Manager2Proxy, Storage1Proxy, UpdateBios1Proxy, UpdateDock1Proxy, WifiDebug1Proxy,
    WifiPowerManagement1Proxy,
};
use steamos_manager::wifi::{WifiBackend, WifiDebugMode, WifiPowerManagement};
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::{zvariant, Connection};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get all properties
    GetAllProperties,

    /// Get luminance sensor calibration gain
    GetAlsCalibrationGain,

    /// Get if the hardware is currently supported
    GetHardwareCurrentlySupported,

    /// Set the fan control state
    SetFanControlState {
        /// Valid options are bios, os
        state: FanControlState,
    },

    /// Get the fan control state
    GetFanControlState,

    /// Get the available CPU scaling governors supported on this device
    GetAvailableCpuScalingGovernors,

    /// Get the current CPU governor
    GetCpuScalingGovernor,

    /// Set the current CPU Scaling governor
    SetCpuScalingGovernor {
        /// Valid governors are get-cpu-governors.
        governor: CPUScalingGovernor,
    },

    /// Get the GPU power profiles supported on this device
    GetAvailableGPUPowerProfiles,

    /// Get the current GPU power profile
    GetGPUPowerProfile,

    /// Set the GPU Power profile
    SetGPUPowerProfile {
        /// Valid profiles are get-gpu-power-profiles.
        profile: GPUPowerProfile,
    },

    /// Set the GPU performance level
    SetGPUPerformanceLevel {
        /// Valid levels are auto, low, high, manual, profile_peak
        level: GPUPerformanceLevel,
    },

    /// Get the GPU performance level
    GetGPUPerformanceLevel,

    /// Set the GPU clock value manually. Only works when performance level is set to Manual
    SetManualGPUClock {
        /// GPU clock frequency in MHz
        freq: u32,
    },

    /// Get the GPU clock frequency, in MHz. Only works when performance level is set to Manual
    GetManualGPUClock,

    /// Get the maximum allowed GPU clock frequency for the Manual performance level
    GetManualGPUClockMax,

    /// Get the minimum allowed GPU clock frequency for the Manual performance level
    GetManualGPUClockMin,

    /// Set the TDP limit
    SetTDPLimit {
        /// TDP limit, in W
        limit: u32,
    },

    /// Get the TDP limit
    GetTDPLimit,

    /// Get the maximum allowed TDP limit
    GetTDPLimitMax,

    /// Get the minimum allowed TDP limit
    GetTDPLimitMin,

    /// Set the wifi backend if possible
    SetWifiBackend {
        /// Supported backends are iwd, wpa_supplicant
        backend: WifiBackend,
    },

    /// Get the wifi backend
    GetWifiBackend,

    /// Set wifi debug mode
    SetWifiDebugMode {
        /// Valid modes are on, off
        mode: WifiDebugMode,
        /// The size of the debug buffer, in bytes
        buffer: Option<u32>,
    },

    /// Get wifi debug mode
    GetWifiDebugMode,

    /// Set the wifi power management state
    SetWifiPowerManagementState {
        /// Valid modes are enabled, disabled
        state: WifiPowerManagement,
    },

    /// Get the wifi power management state
    GetWifiPowerManagementState,

    /// Get the state of HDMI-CEC support
    GetHdmiCecState,

    /// Set the state of HDMI-CEC support
    SetHdmiCecState {
        /// Valid modes are disabled, control-only, control-and-wake
        state: HdmiCecState,
    },

    /// Update the BIOS, if possible
    UpdateBios,

    /// Update the dock, if possible
    UpdateDock,

    /// Trim applicable drives
    TrimDevices,

    /// Factory reset the device
    FactoryReset,
}

#[tokio::main]
async fn main() -> Result<()> {
    // This is a command-line utility that calls api using dbus

    // First set up which command line arguments we support
    let args = Args::parse();

    // Then get a connection to the service
    let conn = Connection::session().await?;

    // Then process arguments
    match &args.command {
        Commands::GetAllProperties => {
            let properties_proxy = PropertiesProxy::new(
                &conn,
                "com.steampowered.SteamOSManager1",
                "/com/steampowered/SteamOSManager1",
            )
            .await?;
            let name = InterfaceName::try_from("com.steampowered.SteamOSManager1.Manager")?;
            let properties = properties_proxy
                .get_all(zvariant::Optional::from(Some(name)))
                .await?;
            for key in properties.keys().sorted() {
                let value = &properties[key];
                let val = value.deref();
                println!("{key}: {val}");
            }
        }
        Commands::GetAlsCalibrationGain => {
            let proxy = AmbientLightSensor1Proxy::new(&conn).await?;
            let gain = proxy.als_calibration_gain().await?;
            println!("ALS calibration gain: {gain}");
        }
        Commands::GetHardwareCurrentlySupported => {
            let proxy = Manager2Proxy::new(&conn).await?;
            let supported = proxy.hardware_currently_supported().await?;
            println!("Hardware currently supported: {supported}");
        }
        Commands::SetFanControlState { state } => {
            let proxy = FanControl1Proxy::new(&conn).await?;
            proxy.set_fan_control_state(*state as u32).await?;
        }
        Commands::GetFanControlState => {
            let proxy = FanControl1Proxy::new(&conn).await?;
            let state = proxy.fan_control_state().await?;
            match FanControlState::try_from(state) {
                Ok(s) => println!("Fan control state: {}", s),
                Err(_) => println!("Got unknown value {state} from backend"),
            }
        }
        Commands::GetAvailableCpuScalingGovernors => {
            let proxy = CpuScaling1Proxy::new(&conn).await?;
            let governors = proxy.available_cpu_scaling_governors().await?;
            println!("Governors:\n");
            for name in governors {
                println!("{name}");
            }
        }
        Commands::GetCpuScalingGovernor => {
            let proxy = CpuScaling1Proxy::new(&conn).await?;
            let governor = proxy.cpu_scaling_governor().await?;
            let governor_type = CPUScalingGovernor::try_from(governor.as_str());
            match governor_type {
                Ok(_) => {
                    println!("CPU Governor: {governor}");
                }
                Err(_) => {
                    println!("Unknown CPU governor or unable to get type from {governor}");
                }
            }
        }
        Commands::SetCpuScalingGovernor { governor } => {
            let proxy = CpuScaling1Proxy::new(&conn).await?;
            proxy
                .set_cpu_scaling_governor(governor.to_string().as_str())
                .await?;
        }
        Commands::GetAvailableGPUPowerProfiles => {
            let proxy = GpuPowerProfile1Proxy::new(&conn).await?;
            let profiles = proxy.available_gpu_power_profiles().await?;
            println!("Profiles:\n");
            for name in profiles.into_iter().sorted() {
                println!("- {name}");
            }
        }
        Commands::GetGPUPowerProfile => {
            let proxy = GpuPowerProfile1Proxy::new(&conn).await?;
            let profile = proxy.gpu_power_profile().await?;
            let profile_type = GPUPowerProfile::try_from(profile.as_str());
            match profile_type {
                Ok(t) => {
                    let name = t.to_string();
                    println!("GPU Power Profile: {profile} {name}");
                }
                Err(_) => {
                    println!("Unknown GPU power profile or unable to get type from {profile}")
                }
            }
        }
        Commands::SetGPUPowerProfile { profile } => {
            let proxy = GpuPowerProfile1Proxy::new(&conn).await?;
            proxy
                .set_gpu_power_profile(profile.to_string().as_str())
                .await?;
        }
        Commands::SetGPUPerformanceLevel { level } => {
            let proxy = GpuPerformanceLevel1Proxy::new(&conn).await?;
            proxy
                .set_gpu_performance_level(level.to_string().as_str())
                .await?;
        }
        Commands::GetGPUPerformanceLevel => {
            let proxy = GpuPerformanceLevel1Proxy::new(&conn).await?;
            let level = proxy.gpu_performance_level().await?;
            match GPUPerformanceLevel::try_from(level.as_str()) {
                Ok(l) => println!("GPU performance level: {}", l),
                Err(_) => println!("Got unknown value {level} from backend"),
            }
        }
        Commands::SetManualGPUClock { freq } => {
            let proxy = GpuPerformanceLevel1Proxy::new(&conn).await?;
            proxy.set_manual_gpu_clock(*freq).await?;
        }
        Commands::GetManualGPUClock => {
            let proxy = GpuPerformanceLevel1Proxy::new(&conn).await?;
            let clock = proxy.manual_gpu_clock().await?;
            println!("Manual GPU Clock: {clock}");
        }
        Commands::GetManualGPUClockMax => {
            let proxy = GpuPerformanceLevel1Proxy::new(&conn).await?;
            let value = proxy.manual_gpu_clock_max().await?;
            println!("Manual GPU Clock Max: {value}");
        }
        Commands::GetManualGPUClockMin => {
            let proxy = GpuPerformanceLevel1Proxy::new(&conn).await?;
            let value = proxy.manual_gpu_clock_min().await?;
            println!("Manual GPU Clock Min: {value}");
        }
        Commands::SetTDPLimit { limit } => {
            let proxy = GpuTdpLimit1Proxy::new(&conn).await?;
            proxy.set_tdp_limit(*limit).await?;
        }
        Commands::GetTDPLimit => {
            let proxy = GpuTdpLimit1Proxy::new(&conn).await?;
            let limit = proxy.tdp_limit().await?;
            println!("TDP limit: {limit}");
        }
        Commands::GetTDPLimitMax => {
            let proxy = GpuTdpLimit1Proxy::new(&conn).await?;
            let value = proxy.tdp_limit_max().await?;
            println!("TDP limit max: {value}");
        }
        Commands::GetTDPLimitMin => {
            let proxy = GpuTdpLimit1Proxy::new(&conn).await?;
            let value = proxy.tdp_limit_min().await?;
            println!("TDP limit min: {value}");
        }
        Commands::SetWifiBackend { backend } => {
            let proxy = WifiDebug1Proxy::new(&conn).await?;
            proxy.set_wifi_backend(backend.to_string().as_str()).await?;
        }
        Commands::GetWifiBackend => {
            let proxy = WifiDebug1Proxy::new(&conn).await?;
            let backend = proxy.wifi_backend().await?;
            match WifiBackend::try_from(backend.as_str()) {
                Ok(be) => println!("Wifi backend: {}", be),
                Err(_) => println!("Got unknown value {backend} from backend"),
            }
        }
        Commands::SetWifiDebugMode { mode, buffer } => {
            let proxy = WifiDebug1Proxy::new(&conn).await?;
            let mut options = HashMap::<&str, &zvariant::Value<'_>>::new();
            let buffer_size;
            if let Some(size) = buffer {
                buffer_size = Some(zvariant::Value::U32(*size));
                options.insert("buffer_size", buffer_size.as_ref().unwrap());
            }
            proxy.set_wifi_debug_mode(*mode as u32, options).await?;
        }
        Commands::GetWifiDebugMode => {
            let proxy = WifiDebug1Proxy::new(&conn).await?;
            let mode = proxy.wifi_debug_mode_state().await?;
            match WifiDebugMode::try_from(mode) {
                Ok(m) => println!("Wifi debug mode: {}", m),
                Err(_) => println!("Got unknown value {mode} from backend"),
            }
        }
        Commands::SetWifiPowerManagementState { state } => {
            let proxy = WifiPowerManagement1Proxy::new(&conn).await?;
            proxy.set_wifi_power_management_state(*state as u32).await?;
        }
        Commands::GetWifiPowerManagementState => {
            let proxy = WifiPowerManagement1Proxy::new(&conn).await?;
            let state = proxy.wifi_power_management_state().await?;
            match WifiPowerManagement::try_from(state) {
                Ok(s) => println!("Wifi power management state: {}", s),
                Err(_) => println!("Got unknown value {state} from backend"),
            }
        }
        Commands::SetHdmiCecState { state } => {
            let proxy = HdmiCec1Proxy::new(&conn).await?;
            proxy.set_hdmi_cec_state(*state as u32).await?;
        }
        Commands::GetHdmiCecState => {
            let proxy = HdmiCec1Proxy::new(&conn).await?;
            let state = proxy.hdmi_cec_state().await?;
            match HdmiCecState::try_from(state) {
                Ok(s) => println!("HDMI-CEC state: {}", s.to_human_readable()),
                Err(_) => println!("Got unknown value {state} from backend"),
            }
        }
        Commands::UpdateBios => {
            let proxy = UpdateBios1Proxy::new(&conn).await?;
            let _ = proxy.update_bios().await?;
        }
        Commands::UpdateDock => {
            let proxy = UpdateDock1Proxy::new(&conn).await?;
            let _ = proxy.update_dock().await?;
        }
        Commands::FactoryReset => {
            let proxy = FactoryReset1Proxy::new(&conn).await?;
            let _ = proxy.prepare_factory_reset().await?;
        }
        Commands::TrimDevices => {
            let proxy = Storage1Proxy::new(&conn).await?;
            let _ = proxy.trim_devices().await?;
        }
    }

    Ok(())
}
