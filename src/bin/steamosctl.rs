/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use clap::{Parser, Subcommand};
use itertools::Itertools;
use std::ops::Deref;
use steamos_manager::power::GPUPerformanceLevel;
use steamos_manager::proxy::ManagerProxy;
use steamos_manager::wifi::WifiBackend;
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
        /// 0 - BIOS, 1 - OS
        state: u32,
    },

    /// Get the fan control state
    GetFanControlState,

    /// Set the GPU performance level
    SetGPUPerformanceLevel {
        /// Allowed levels are auto, low, high, manual, peak_performance
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

    /// Get the current API version
    GetVersion,

    /// Set the wifi backend if possible
    SetWifiBackend {
        /// Supported backends are iwd, wpa_supplicant
        backend: WifiBackend,
    },

    /// Get the wifi backend
    GetWifiBackend,

    /// Set wifi debug mode
    SetWifiDebugMode {
        /// 1 for on, 0 for off
        mode: u32,
        /// The size of the debug buffer, in bytes
        #[arg(default_value_t = 20000)]
        buffer: u32,
    },

    /// Get wifi debug mode
    GetWifiDebugMode,

    /// Set the wifi power management state
    SetWifiPowerManagementState {
        /// 0 - disabled, 1 - enabled
        state: u32,
    },

    /// Get the wifi power management state
    GetWifiPowerManagementState,

    /// Get the state of HDMI-CEC support
    GetHdmiCecState,

    /// Set the state of HDMI-CEC support
    SetHdmiCecState {
        /// 0 - disabled, 1 - only controls, 2 - Controls and TV waking
        state: u32,
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
    let proxy = ManagerProxy::builder(&conn).build().await?;

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
            let gain = proxy.als_calibration_gain().await?;
            println!("ALS calibration gain: {gain}");
        }
        Commands::GetHardwareCurrentlySupported => {
            let supported = proxy.hardware_currently_supported().await?;
            println!("Hardware currently supported: {supported}");
        }
        Commands::GetVersion => {
            let version = proxy.version().await?;
            println!("Version: {version}");
        }
        Commands::SetFanControlState { state } => {
            proxy.set_fan_control_state(*state).await?;
        }
        Commands::SetGPUPerformanceLevel { level } => {
            proxy.set_gpu_performance_level(*level as u32).await?;
        }
        Commands::GetGPUPerformanceLevel => {
            let level = proxy.gpu_performance_level().await?;
            match GPUPerformanceLevel::try_from(level) {
                Ok(l) => println!("GPU performance level: {}", l.to_string()),
                Err(_) => println!("Got unknown value {level} from backend"),
            }
        }
        Commands::SetManualGPUClock { freq } => {
            proxy.set_manual_gpu_clock(*freq).await?;
        }
        Commands::GetManualGPUClock => {
            let clock = proxy.manual_gpu_clock().await?;
            println!("Manual GPU Clock: {clock}");
        }
        Commands::GetManualGPUClockMax => {
            let value = proxy.manual_gpu_clock_max().await?;
            println!("Manual GPU Clock Max: {value}");
        }
        Commands::GetManualGPUClockMin => {
            let value = proxy.manual_gpu_clock_min().await?;
            println!("Manual GPU Clock Min: {value}");
        }
        Commands::SetTDPLimit { limit } => {
            proxy.set_tdp_limit(*limit).await?;
        }
        Commands::GetTDPLimit => {
            let limit = proxy.tdp_limit().await?;
            println!("TDP limit: {limit}");
        }
        Commands::GetFanControlState => {
            let state = proxy.fan_control_state().await?;
            println!("Fan control state: {state}");
        }
        Commands::GetTDPLimitMax => {
            let value = proxy.tdp_limit_max().await?;
            println!("TDP limit max: {value}");
        }
        Commands::GetTDPLimitMin => {
            let value = proxy.tdp_limit_min().await?;
            println!("TDP limit min: {value}");
        }
        Commands::SetWifiBackend { backend } => {
            proxy.set_wifi_backend(*backend as u32).await?;
        },
        Commands::GetWifiBackend => {
            let backend = proxy.wifi_backend().await?;
            let backend_string = WifiBackend::try_from(backend).unwrap().to_string();
            println!("Wifi backend: {backend_string}");
        }
        Commands::SetWifiDebugMode { mode, buffer } => {
            proxy.set_wifi_debug_mode(*mode, *buffer).await?;
        }
        Commands::GetWifiDebugMode => {
            let mode = proxy.wifi_debug_mode_state().await?;
            println!("Wifi debug mode: {mode}");
        }
        Commands::SetWifiPowerManagementState { state } => {
            proxy.set_wifi_power_management_state(*state).await?;
        }
        Commands::GetWifiPowerManagementState => {
            let state = proxy.wifi_power_management_state().await?;
            println!("Wifi power management state: {state}");
        }
        Commands::SetHdmiCecState { state } => {
            proxy.set_hdmi_cec_state(*state).await?;
        }
        Commands::GetHdmiCecState => {
            let state = proxy.hdmi_cec_state().await?;
            println!("HDMI-CEC state: {state}");
        }
        Commands::UpdateBios => {
            let _ = proxy.update_bios().await?;
        }
        Commands::UpdateDock => {
            let _ = proxy.update_dock().await?;
        }
        Commands::FactoryReset => {
            let _ = proxy.prepare_factory_reset().await?;
        }
        Commands::TrimDevices => {
            let _ = proxy.trim_devices().await?;
        }
    }

    Ok(())
}
