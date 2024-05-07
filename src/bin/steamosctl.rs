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
use std::str::FromStr;
use steamos_manager::{ManagerProxy, WifiBackend};
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::{zvariant, Connection};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Optionally get all properties
    #[arg(short, long)]
    all_properties: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    GetAlsCalibrationGain {},
    GetHardwareCurrentlySupported {},

    SetFanControlState {
        // Set the fan control state.
        // 0 - BIOS, 1 - OS
        #[arg(short, long)]
        value: u32,
    },

    GetFanControlState {},

    SetGPUPerformanceLevel {
        // Set the gpu performance level
        // 0 = Auto, 1 = Low, 2 = High, 3 = Manual, 4 = Profile Peak
        #[arg(short, long)]
        value: u32,
    },

    GetGPUPerformanceLevel {},

    SetManualGPUClock {
        // Set the GPU clock value manually
        // Controls the GPU clock frequency in MHz when GPUPerformanceLevel is set to Manual
        #[arg(short, long)]
        value: u32,
    },

    GetManualGPUClock {},
    GetManualGPUClockMax {},
    GetManualGPUClockMin {},

    SetTDPLimit {
        // Set the TDP limit
        #[arg(short, long)]
        value: u32,
    },

    GetTDPLimit {},
    GetTDPLimitMax {},
    GetTDPLimitMin {},

    GetVersion {},

    SetWifiBackend {
        // Set the wifi backend to given string if possible
        // Supported values are iwd|wpa_supplicant
        #[arg(short, long)]
        backend: String,
    },

    GetWifiBackend {},

    SetWifiDebugMode {
        // Set wifi debug mode to given value
        // 1 for on, 0 for off currently
        #[arg(short, long)]
        mode: u32,
    },

    GetWifiDebugMode {},

    SetWifiPowerManagementState {
        // Set the wifi power management state
        // 0 - disabled, 1 - enabled
        #[arg(short, long)]
        value: u32,
    },

    GetWifiPowerManagementState {},

    UpdateBios {},
    UpdateDock {},
    TrimDevices {},
    FactoryReset {},
}

#[tokio::main]
async fn main() -> Result<()> {
    // This is a command-line utility that calls api using dbus

    // First set up which command line arguments we support
    let args = Args::parse();

    // Then get a connection to the service
    let conn = Connection::session().await?;
    let proxy = ManagerProxy::builder(&conn).build().await?;

    if args.all_properties {
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

    // Then process arguments
    match &args.command {
        Some(Commands::GetAlsCalibrationGain {}) => {
            let gain = proxy.als_calibration_gain().await?;
            println!("ALS calibration gain: {gain}");
        }
        Some(Commands::GetHardwareCurrentlySupported {}) => {
            let supported = proxy.hardware_currently_supported().await?;
            println!("Hardware currently supported: {supported}");
        }
        Some(Commands::GetVersion {}) => {
            let version = proxy.version().await?;
            println!("Version: {version}");
        }
        Some(Commands::SetFanControlState { value }) => {
            proxy.set_fan_control_state(*value).await?;
        }
        Some(Commands::SetGPUPerformanceLevel { value }) => {
            proxy.set_gpu_performance_level(*value).await?;
        }
        Some(Commands::GetGPUPerformanceLevel {}) => {
            let level = proxy.gpu_performance_level().await?;
            println!("GPU performance level: {level}");
        }
        Some(Commands::SetManualGPUClock { value }) => {
            proxy.set_manual_gpu_clock(*value).await?;
        }
        Some(Commands::GetManualGPUClock {}) => {
            let clock = proxy.manual_gpu_clock().await?;
            println!("Manual GPU Clock: {clock}");
        }
        Some(Commands::GetManualGPUClockMax {}) => {
            let value = proxy.manual_gpu_clock_max().await?;
            println!("Manual GPU Clock Max: {value}");
        }
        Some(Commands::GetManualGPUClockMin {}) => {
            let value = proxy.manual_gpu_clock_min().await?;
            println!("Manual GPU Clock Min: {value}");
        }
        Some(Commands::SetTDPLimit { value }) => {
            proxy.set_tdp_limit(*value).await?;
        }
        Some(Commands::GetTDPLimit {}) => {
            let limit = proxy.tdp_limit().await?;
            println!("TDP limit: {limit}");
        }
        Some(Commands::GetFanControlState {}) => {
            let state = proxy.fan_control_state().await?;
            println!("Fan control state: {state}");
        }
        Some(Commands::GetTDPLimitMax {}) => {
            let value = proxy.tdp_limit_max().await?;
            println!("TDP limit max: {value}");
        }
        Some(Commands::GetTDPLimitMin {}) => {
            let value = proxy.tdp_limit_min().await?;
            println!("TDP limit min: {value}");
        }
        Some(Commands::SetWifiBackend { backend }) => match WifiBackend::from_str(backend) {
            Ok(b) => {
                proxy.set_wifi_backend(b as u32).await?;
            }
            Err(_) => {
                println!("Unknown wifi backend {backend}");
            }
        },
        Some(Commands::GetWifiBackend {}) => {
            let backend = proxy.wifi_backend().await?;
            let backend_string = WifiBackend::try_from(backend).unwrap().to_string();
            println!("Wifi backend: {backend_string}");
        }
        Some(Commands::SetWifiDebugMode { mode }) => {
            proxy.set_wifi_debug_mode(*mode, 20000).await?;
        }
        Some(Commands::GetWifiDebugMode {}) => {
            let mode = proxy.wifi_debug_mode_state().await?;
            println!("Wifi debug mode: {mode}");
        }
        Some(Commands::SetWifiPowerManagementState { value }) => {
            proxy.set_wifi_power_management_state(*value).await?;
        }
        Some(Commands::GetWifiPowerManagementState {}) => {
            let state = proxy.wifi_power_management_state().await?;
            println!("Wifi power management state: {state}");
        }
        Some(Commands::UpdateBios {}) => {
            let _ = proxy.update_bios().await?;
        }
        Some(Commands::UpdateDock {}) => {
            let _ = proxy.update_dock().await?;
        }
        Some(Commands::FactoryReset {}) => {
            let _ = proxy.prepare_factory_reset().await?;
        }
        Some(Commands::TrimDevices {}) => {
            let _ = proxy.trim_devices().await?;
        }
        None => {}
    }

    Ok(())
}
