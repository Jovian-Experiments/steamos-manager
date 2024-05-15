/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{bail, Error, Result};
use std::fmt;
use std::str::FromStr;
use tokio::fs;
use zbus::Connection;

use crate::path;
use crate::process::script_exit_code;
use crate::systemd::SystemdUnit;

const BOARD_VENDOR_PATH: &str = "/sys/class/dmi/id/board_vendor";
const BOARD_NAME_PATH: &str = "/sys/class/dmi/id/board_name";

#[derive(PartialEq, Debug, Copy, Clone)]
pub(crate) enum HardwareVariant {
    Unknown,
    Jupiter,
    Galileo,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub(crate) enum HardwareCurrentlySupported {
    Unsupported = 0,
    Supported = 1,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum FanControlState {
    Bios = 0,
    Os = 1,
}

impl FromStr for HardwareVariant {
    type Err = Error;
    fn from_str(input: &str) -> Result<HardwareVariant, Self::Err> {
        Ok(match input {
            "Jupiter" => HardwareVariant::Jupiter,
            "Galileo" => HardwareVariant::Galileo,
            _ => HardwareVariant::Unknown,
        })
    }
}

impl TryFrom<u32> for HardwareCurrentlySupported {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == HardwareCurrentlySupported::Unsupported as u32 => {
                Ok(HardwareCurrentlySupported::Unsupported)
            }
            x if x == HardwareCurrentlySupported::Supported as u32 => {
                Ok(HardwareCurrentlySupported::Supported)
            }
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl fmt::Display for HardwareCurrentlySupported {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HardwareCurrentlySupported::Unsupported => write!(f, "Unsupported"),
            HardwareCurrentlySupported::Supported => write!(f, "Supported"),
        }
    }
}

impl TryFrom<u32> for FanControlState {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == FanControlState::Bios as u32 => Ok(FanControlState::Bios),
            x if x == FanControlState::Os as u32 => Ok(FanControlState::Os),
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl FromStr for FanControlState {
    type Err = Error;
    fn from_str(input: &str) -> Result<FanControlState, Self::Err> {
        Ok(match input.to_lowercase().as_str() {
            "bios" => FanControlState::Bios,
            "os" => FanControlState::Os,
            v => bail!("No enum match for value {v}"),
        })
    }
}

impl fmt::Display for FanControlState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FanControlState::Bios => write!(f, "BIOS"),
            FanControlState::Os => write!(f, "OS"),
        }
    }
}

pub(crate) async fn variant() -> Result<HardwareVariant> {
    let board_vendor = fs::read_to_string(path(BOARD_VENDOR_PATH)).await?;
    if board_vendor.trim_end() != "Valve" {
        return Ok(HardwareVariant::Unknown);
    }

    let board_name = fs::read_to_string(path(BOARD_NAME_PATH)).await?;
    HardwareVariant::from_str(board_name.trim_end())
}

pub(crate) async fn check_support() -> Result<HardwareCurrentlySupported> {
    // Run jupiter-check-support note this script does exit 1 for "Support: No" case
    // so no need to parse output, etc.
    let res = script_exit_code("/usr/bin/jupiter-check-support", &[] as &[String; 0]).await?;

    Ok(match res {
        0 => HardwareCurrentlySupported::Supported,
        _ => HardwareCurrentlySupported::Unsupported,
    })
}

pub(crate) struct FanControl {
    connection: Connection,
}

impl FanControl {
    pub fn new(connection: Connection) -> FanControl {
        FanControl { connection }
    }

    pub async fn get_state(&self) -> Result<FanControlState> {
        let jupiter_fan_control =
            SystemdUnit::new(self.connection.clone(), "jupiter-fan-control.service").await?;
        let active = jupiter_fan_control.active().await?;
        Ok(match active {
            true => FanControlState::Os,
            false => FanControlState::Bios,
        })
    }

    pub async fn set_state(&self, state: FanControlState) -> Result<()> {
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        let jupiter_fan_control =
            SystemdUnit::new(self.connection.clone(), "jupiter-fan-control.service").await?;
        match state {
            FanControlState::Os => jupiter_fan_control.start().await,
            FanControlState::Bios => jupiter_fan_control.stop().await,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use tokio::fs::{create_dir_all, write};

    #[tokio::test]
    async fn board_lookup() {
        let _h = testing::start();

        create_dir_all(crate::path("/sys/class/dmi/id"))
            .await
            .expect("create_dir_all");
        assert!(variant().await.is_err());

        write(crate::path(BOARD_VENDOR_PATH), "LENOVO\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Unknown);

        write(crate::path(BOARD_VENDOR_PATH), "Valve\n")
            .await
            .expect("write");
        write(crate::path(BOARD_NAME_PATH), "Jupiter\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Jupiter);

        write(crate::path(BOARD_NAME_PATH), "Galileo\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Galileo);

        write(crate::path(BOARD_NAME_PATH), "Neptune\n")
            .await
            .expect("write");
        assert_eq!(variant().await.unwrap(), HardwareVariant::Unknown);
    }
}
