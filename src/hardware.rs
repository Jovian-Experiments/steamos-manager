/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{Error, Result};
use std::fmt;
use std::str::FromStr;
use tokio::fs;

use crate::path;
use crate::process::script_exit_code;

const BOARD_VENDOR_PATH: &str = "/sys/class/dmi/id/board_vendor";
const BOARD_NAME_PATH: &str = "/sys/class/dmi/id/board_name";

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum HardwareVariant {
    Unknown,
    Jupiter,
    Galileo,
}

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum HardwareCurrentlySupported {
    UnsupportedFeature = 0,
    Unsupported = 1,
    Supported = 2,
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
            x if x == HardwareCurrentlySupported::UnsupportedFeature as u32 => {
                Ok(HardwareCurrentlySupported::UnsupportedFeature)
            }
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
            HardwareCurrentlySupported::UnsupportedFeature => write!(f, "Unsupported feature"),
            HardwareCurrentlySupported::Unsupported => write!(f, "Unsupported"),
            HardwareCurrentlySupported::Supported => write!(f, "Supported"),
        }
    }
}

pub async fn variant() -> Result<HardwareVariant> {
    let board_vendor = fs::read_to_string(path(BOARD_VENDOR_PATH)).await?;
    if board_vendor.trim_end() != "Valve" {
        return Ok(HardwareVariant::Unknown);
    }

    let board_name = fs::read_to_string(path(BOARD_NAME_PATH)).await?;
    HardwareVariant::from_str(board_name.trim_end())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use tokio::fs::{create_dir_all, write};

    #[tokio::test]
    async fn board_lookup() {
        let h = testing::start();

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

pub async fn check_support() -> Result<HardwareCurrentlySupported> {
    // Run jupiter-check-support note this script does exit 1 for "Support: No" case
    // so no need to parse output, etc.
    let res = script_exit_code("/usr/bin/jupiter-check-support", &[] as &[String; 0]).await?;

    Ok(match res {
        0 => HardwareCurrentlySupported::Supported,
        _ => HardwareCurrentlySupported::Unsupported,
    })
}
