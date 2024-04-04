/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{Error, Result};
use std::str::FromStr;
use tokio::fs;

use crate::path;

const BOARD_VENDOR_PATH: &str = "/sys/class/dmi/id/board_vendor";
const BOARD_NAME_PATH: &str = "/sys/class/dmi/id/board_name";

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum HardwareVariant {
    Unknown,
    Jupiter,
    Galileo,
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
