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
    Unknown = 0,
    UnsupportedPrototype = 1,
    Supported = 2,
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
    type Error = String;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == HardwareCurrentlySupported::Unknown as u32 => {
                Ok(HardwareCurrentlySupported::Unknown)
            }
            x if x == HardwareCurrentlySupported::UnsupportedPrototype as u32 => {
                Ok(HardwareCurrentlySupported::UnsupportedPrototype)
            }
            x if x == HardwareCurrentlySupported::Supported as u32 => {
                Ok(HardwareCurrentlySupported::Supported)
            }
            _ => Err(format!("No enum match for value {v}")),
        }
    }
}

impl fmt::Display for HardwareCurrentlySupported {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HardwareCurrentlySupported::Unknown => write!(f, "Unknown"),
            HardwareCurrentlySupported::UnsupportedPrototype => write!(f, "Unsupported Prototype"),
            HardwareCurrentlySupported::Supported => write!(f, "Supported"),
        }
    }
}

impl TryFrom<u32> for FanControlState {
    type Error = String;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == FanControlState::Bios as u32 => Ok(FanControlState::Bios),
            x if x == FanControlState::Os as u32 => Ok(FanControlState::Os),
            _ => Err(format!("No enum match for value {v}")),
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

pub(crate) async fn is_deck() -> Result<bool> {
    match variant().await {
        Ok(variant) => Ok(variant != HardwareVariant::Unknown),
        Err(e) => Err(e),
    }
}

pub(crate) async fn check_support() -> Result<HardwareCurrentlySupported> {
    // Run jupiter-check-support note this script does exit 1 for "Support: No" case
    // so no need to parse output, etc.
    let res = script_exit_code("/usr/bin/jupiter-check-support", &[] as &[String; 0]).await?;

    Ok(match res {
        0 => HardwareCurrentlySupported::Supported,
        _ => HardwareCurrentlySupported::UnsupportedPrototype,
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
pub mod test {
    use super::*;
    use crate::error::to_zbus_fdo_error;
    use crate::{enum_roundtrip, testing};
    use std::time::Duration;
    use tokio::fs::{create_dir_all, write};
    use tokio::time::sleep;
    use zbus::fdo;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath};

    pub(crate) async fn fake_model(model: HardwareVariant) -> Result<()> {
        create_dir_all(crate::path("/sys/class/dmi/id")).await?;
        match model {
            HardwareVariant::Unknown => write(crate::path(BOARD_VENDOR_PATH), "LENOVO\n").await?,
            HardwareVariant::Jupiter => {
                write(crate::path(BOARD_VENDOR_PATH), "Valve\n").await?;
                write(crate::path(BOARD_NAME_PATH), "Jupiter\n").await?;
            }
            HardwareVariant::Galileo => {
                write(crate::path(BOARD_VENDOR_PATH), "Valve\n").await?;
                write(crate::path(BOARD_NAME_PATH), "Galileo\n").await?;
            }
        }
        Ok(())
    }

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

    #[test]
    fn hardware_currently_supported_roundtrip() {
        enum_roundtrip!(HardwareCurrentlySupported {
            0: u32 = Unknown,
            1: u32 = UnsupportedPrototype,
            2: u32 = Supported,
        });
        assert!(HardwareCurrentlySupported::try_from(3).is_err());
        assert_eq!(HardwareCurrentlySupported::Unknown.to_string(), "Unknown");
        assert_eq!(
            HardwareCurrentlySupported::UnsupportedPrototype.to_string(),
            "Unsupported Prototype"
        );
        assert_eq!(
            HardwareCurrentlySupported::Supported.to_string(),
            "Supported"
        );
    }

    #[test]
    fn fan_control_state_roundtrip() {
        enum_roundtrip!(FanControlState {
            0: u32 = Bios,
            1: u32 = Os,
            "BIOS": str = Bios,
            "OS": str = Os,
        });
        assert_eq!(
            FanControlState::from_str("os").unwrap(),
            FanControlState::Os
        );
        assert_eq!(
            FanControlState::from_str("bios").unwrap(),
            FanControlState::Bios
        );
        assert!(FanControlState::try_from(2).is_err());
        assert!(FanControlState::from_str("on").is_err());
    }

    #[derive(Default)]
    struct MockUnit {
        active: bool,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl MockUnit {
        #[zbus(property)]
        fn active_state(&self) -> fdo::Result<String> {
            if self.active {
                Ok(String::from("active"))
            } else {
                Ok(String::from("inactive"))
            }
        }

        async fn start(&mut self, mode: &str) -> fdo::Result<OwnedObjectPath> {
            if mode != "fail" {
                return Err(to_zbus_fdo_error("Invalid mode"));
            }
            self.active = true;
            let path = ObjectPath::try_from("/start/0").map_err(to_zbus_fdo_error)?;
            Ok(path.into())
        }

        async fn stop(&mut self, mode: &str) -> fdo::Result<OwnedObjectPath> {
            if mode != "fail" {
                return Err(to_zbus_fdo_error("Invalid mode"));
            }
            self.active = false;
            let path = ObjectPath::try_from("/stop/0").map_err(to_zbus_fdo_error)?;
            Ok(path.into())
        }
    }

    #[tokio::test]
    async fn test_fan_control() {
        let mut h = testing::start();
        let unit = MockUnit::default();
        let connection = h.new_dbus().await.expect("dbus");
        connection
            .request_name("org.freedesktop.systemd1")
            .await
            .expect("request_name");
        connection
            .object_server()
            .at(
                "/org/freedesktop/systemd1/unit/jupiter_2dfan_2dcontrol_2eservice",
                unit,
            )
            .await
            .expect("at");

        sleep(Duration::from_millis(10)).await;

        let fan_control = FanControl::new(connection);
        assert_eq!(
            fan_control.get_state().await.unwrap(),
            FanControlState::Bios
        );
        assert!(fan_control.set_state(FanControlState::Os).await.is_ok());
        assert_eq!(fan_control.get_state().await.unwrap(), FanControlState::Os);
        assert!(fan_control.set_state(FanControlState::Bios).await.is_ok());
        assert_eq!(
            fan_control.get_state().await.unwrap(),
            FanControlState::Bios
        );
    }
}
