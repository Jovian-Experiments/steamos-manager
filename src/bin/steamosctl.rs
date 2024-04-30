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
    SetWifiBackend {
        // Set the wifi backend to given string if possible
        // Supported values are iwd|wpa_supplicant
        #[arg(short, long)]
        backend: String,
    },

    SetWifiDebugMode {
        // Set wifi debug mode to given value
        // 1 for on, 0 for off currently
        #[arg(short, long)]
        mode: u32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // This is a command-line utility that calls api using dbus

    // First set up which command line arguments we support
    let args = Args::parse();

    // Then get a connection to the service
    let conn = Connection::system().await?;
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
        Some(Commands::SetWifiBackend { backend }) => match WifiBackend::from_str(backend) {
            Ok(b) => {
                proxy.set_wifi_backend(b as u32).await?;
            }
            Err(_) => {
                println!("Unknown wifi backend {backend}");
            }
        },
        Some(Commands::SetWifiDebugMode { mode }) => {
            proxy.set_wifi_debug_mode(*mode, 20000).await?;
        }
        None => {}
    }

    Ok(())
}
